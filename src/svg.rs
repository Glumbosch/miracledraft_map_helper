use crate::{
    Error, Result, Value,
    assets::Resolver,
    fonts, godot_text,
    images::{self, Images},
};
use quick_xml::{Reader, events::Event};
use std::{
    collections::HashMap,
    fs,
    io::Cursor,
    path::{Path, PathBuf},
};

const WD: &str = "urn:wonderdraft-map-editor";
#[derive(Clone, Debug, Default)]
pub struct Summary {
    pub boxes: usize,
    pub labels: usize,
    pub symbols: usize,
    pub paths: usize,
    pub territories: usize,
    pub missing_symbols: usize,
    pub background: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    pub background: bool,
    pub boxes: bool,
    pub paths: bool,
    pub symbols: bool,
    pub labels: bool,
    pub territories: bool,
    pub embed_background: bool,
    pub embed_boxes: bool,
    pub embed_symbols: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            background: true,
            boxes: true,
            paths: true,
            symbols: true,
            labels: true,
            territories: true,
            embed_background: false,
            embed_boxes: false,
            embed_symbols: false,
        }
    }
}

type Matrix = [f64; 6];
const IDENTITY: Matrix = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];

fn mat_mul(left: Matrix, right: Matrix) -> Matrix {
    let [a, b, c, d, e, f] = left;
    let [g, h, i, j, k, l] = right;
    [
        a * g + c * h,
        b * g + d * h,
        a * i + c * j,
        b * i + d * j,
        a * k + c * l + e,
        b * k + d * l + f,
    ]
}
fn mat_apply(m: Matrix, x: f64, y: f64) -> (f64, f64) {
    (m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5])
}
fn matrix_scale_rotation(m: Matrix) -> (f64, f64, f64, bool) {
    let sx = m[0].hypot(m[1]);
    let sy = m[2].hypot(m[3]);
    (sx, sy, m[1].atan2(m[0]), m[0] * m[3] - m[1] * m[2] < 0.0)
}
fn normalize_angle(angle: f64) -> f64 {
    (angle + std::f64::consts::PI).rem_euclid(2.0 * std::f64::consts::PI) - std::f64::consts::PI
}
fn rotate_vector((x, y): (f64, f64), angle: f64) -> (f64, f64) {
    let (sin, cos) = angle.sin_cos();
    (x * cos - y * sin, x * sin + y * cos)
}
fn rotation_about(angle: f64, (x, y): (f64, f64)) -> Matrix {
    let (sin, cos) = angle.sin_cos();
    mat_mul(
        mat_mul([1., 0., 0., 1., x, y], [cos, sin, -sin, cos, 0., 0.]),
        [1., 0., 0., 1., -x, -y],
    )
}
fn mirror_about_y(y: f64) -> Matrix {
    [1., 0., 0., -1., 0., 2. * y]
}
fn matrix_text(m: Matrix) -> String {
    format!(
        "matrix({:.12} {:.12} {:.12} {:.12} {:.12} {:.12})",
        m[0], m[1], m[2], m[3], m[4], m[5]
    )
}

fn parse_transform(text: Option<&str>) -> Matrix {
    let Some(text) = text else { return IDENTITY };
    let mut result = IDENTITY;
    let mut rest = text.trim();
    while let Some(open) = rest.find('(') {
        let name = rest[..open].split_whitespace().last().unwrap_or("");
        let Some(close_rel) = rest[open + 1..].find(')') else {
            break;
        };
        let close = open + 1 + close_rel;
        let values: Vec<f64> = rest[open + 1..close]
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|part| !part.is_empty())
            .filter_map(|part| part.parse().ok())
            .collect();
        let transform = match name {
            "matrix" if values.len() >= 6 => [
                values[0], values[1], values[2], values[3], values[4], values[5],
            ],
            "translate" if !values.is_empty() => [
                1.,
                0.,
                0.,
                1.,
                values[0],
                values.get(1).copied().unwrap_or(0.),
            ],
            "scale" if !values.is_empty() => [
                values[0],
                0.,
                0.,
                values.get(1).copied().unwrap_or(values[0]),
                0.,
                0.,
            ],
            "rotate" if !values.is_empty() => {
                let angle = values[0].to_radians();
                if values.len() >= 3 {
                    rotation_about(angle, (values[1], values[2]))
                } else {
                    let (sin, cos) = angle.sin_cos();
                    [cos, sin, -sin, cos, 0., 0.]
                }
            }
            _ => IDENTITY,
        };
        result = mat_mul(result, transform);
        rest = &rest[close + 1..];
    }
    result
}
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn n(v: Option<&Value>, default: f64) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(default)
}
fn property<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    match value {
        Value::Object { properties, .. } => properties
            .iter()
            .find_map(|(key, value)| (key == name).then_some(value)),
        _ => value.get(name),
    }
}
fn vec2(v: Option<&Value>, default: (f64, f64)) -> (f64, f64) {
    match v {
        Some(Value::Vector { values, .. }) if values.len() >= 2 => {
            (values[0] as f64, values[1] as f64)
        }
        _ => default,
    }
}
fn color(v: Option<&Value>, default: [f64; 4]) -> (String, f64) {
    let c = match v {
        Some(Value::Vector { kind, values }) if kind == "Color" && values.len() >= 4 => [
            values[0] as f64,
            values[1] as f64,
            values[2] as f64,
            values[3] as f64,
        ],
        _ => default,
    };
    (
        format!(
            "#{:02x}{:02x}{:02x}",
            (c[0].clamp(0., 1.) * 255.).round() as u8,
            (c[1].clamp(0., 1.) * 255.).round() as u8,
            (c[2].clamp(0., 1.) * 255.).round() as u8
        ),
        c[3].clamp(0., 1.),
    )
}
fn b64(data: &[u8], url: bool) -> String {
    let alpha = if url {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"
    } else {
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/"
    };
    let mut o = String::new();
    for c in data.chunks(3) {
        let v = ((c[0] as u32) << 16)
            | ((c.get(1).copied().unwrap_or(0) as u32) << 8)
            | c.get(2).copied().unwrap_or(0) as u32;
        o.push(alpha[((v >> 18) & 63) as usize] as char);
        o.push(alpha[((v >> 12) & 63) as usize] as char);
        if c.len() > 1 {
            o.push(alpha[((v >> 6) & 63) as usize] as char);
        } else if !url {
            o.push('=');
        }
        if c.len() > 2 {
            o.push(alpha[(v & 63) as usize] as char);
        } else if !url {
            o.push('=');
        }
    }
    o
}
fn b64decode(s: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| *c != b'=') {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return Err(Error::format("invalid base64 metadata")),
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

fn symbol_path_key(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_owned())
}

fn png_data_uri(path: &Path) -> Result<String> {
    let png = if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"))
    {
        fs::read(path).map_err(|error| {
            Error::format(format!(
                "could not read symbol image {}: {error}",
                path.display()
            ))
        })?
    } else {
        let image = image::open(path).map_err(|error| {
            Error::format(format!(
                "could not decode symbol image {} for PNG embedding: {error}",
                path.display()
            ))
        })?;
        let mut png = Cursor::new(Vec::new());
        image
            .write_to(&mut png, image::ImageFormat::Png)
            .map_err(|error| {
                Error::format(format!(
                    "could not encode symbol image {} as PNG: {error}",
                    path.display()
                ))
            })?;
        png.into_inner()
    };
    Ok(format!("data:image/png;base64,{}", b64(&png, false)))
}

fn custom_color_matrix(record: &Value) -> Option<String> {
    if n(record.get("custom_color_mode"), 0.0) == 0.0 {
        return None;
    }
    let colors = record.get("custom_colors")?.as_array()?;
    let mut channels = [[0.0; 4]; 3];
    for (channel, color) in channels.iter_mut().zip(colors.iter().take(3)) {
        let Value::Vector { kind, values } = color else {
            return None;
        };
        if kind != "Color" || values.len() < 4 {
            return None;
        }
        for (output, value) in channel.iter_mut().zip(values.iter().take(4)) {
            let value = *value as f64;
            if !value.is_finite() {
                return None;
            }
            *output = value;
        }
    }
    if colors.len() < 3 {
        return None;
    }
    Some(format!(
        "{} {} {} 0 0  {} {} {} 0 0  {} {} {} 0 0  {} {} {} 0 0",
        channels[0][0],
        channels[1][0],
        channels[2][0],
        channels[0][1],
        channels[1][1],
        channels[2][1],
        channels[0][2],
        channels[1][2],
        channels[2][2],
        channels[0][3],
        channels[1][3],
        channels[2][3]
    ))
}

fn outline_style(record: &Value) -> Option<(String, f64, String, f64)> {
    let width = record
        .get("outline_width")
        .or_else(|| record.get("outline_size"))
        .and_then(Value::as_f64)
        .unwrap_or(0.0);
    if !width.is_finite() || width <= 0.0 {
        return None;
    }
    let outline_color = record.get("outline_color")?;
    let Value::Vector { kind, values } = outline_color else {
        return None;
    };
    if kind != "Color" || values.len() < 4 || values.iter().take(4).any(|value| !value.is_finite())
    {
        return None;
    }
    let (color, alpha) = color(Some(outline_color), [0.0, 0.0, 0.0, 1.0]);
    let key = format!("{width}|{color}|{alpha}");
    Some((key, width, color, alpha))
}
fn record(v: &Value) -> String {
    b64(godot_text::format(v).as_bytes(), true)
}
fn decode_record(s: &str) -> Result<Value> {
    let b = b64decode(s)?;
    godot_text::parse(std::str::from_utf8(&b).map_err(|_| Error::format("invalid record UTF-8"))?)
}
fn points(v: &Value) -> Option<Vec<(f64, f64)>> {
    match v {
        Value::PoolVectors {
            components: 2,
            values,
            ..
        } => Some(
            values
                .iter()
                .filter(|v| v.len() >= 2)
                .map(|v| (v[0] as f64, v[1] as f64))
                .collect(),
        ),
        Value::Array(a) => Some(
            a.iter()
                .filter_map(|v| match v {
                    Value::Vector { values, .. } if values.len() >= 2 => {
                        Some((values[0] as f64, values[1] as f64))
                    }
                    _ => None,
                })
                .collect(),
        ),
        Value::String(s) => godot_text::parse(s).ok().and_then(|v| points(&v)),
        Value::Dictionary(d) => d.iter().find_map(|(_, v)| points(v)),
        _ => None,
    }
}

fn svg_path_data(points: &[(f64, f64)], position: (f64, f64), closed: bool) -> String {
    let mut data = String::new();
    for (index, (x, y)) in points.iter().enumerate() {
        let command = if index == 0 { 'M' } else { 'L' };
        data.push_str(&format!(
            "{command} {:.6},{:.6} ",
            x + position.0,
            y + position.1
        ));
    }
    if closed {
        data.push('Z');
    } else {
        data.pop();
    }
    data
}

fn layer_open(id: &str) -> String {
    format!("  <g inkscape:groupmode=\"layer\" inkscape:label=\"{id}\" id=\"{id}\">\n")
}

fn positioned_points(points: &[(f64, f64)], position: (f64, f64)) -> Vec<(f64, f64)> {
    points
        .iter()
        .map(|(x, y)| (x + position.0, y + position.1))
        .collect()
}

fn polygon_data(points: &[(f64, f64)]) -> String {
    svg_path_data(points, (0.0, 0.0), true)
}

fn local_point(origin: (f64, f64), tangent: (f64, f64), along: f64, across: f64) -> (f64, f64) {
    (
        origin.0 + tangent.0 * along - tangent.1 * across,
        origin.1 + tangent.1 * along + tangent.0 * across,
    )
}

fn path_pattern_data(points: &[(f64, f64)], requested_width: f64, style: &str) -> String {
    let requested_width = requested_width.max(0.0);
    if requested_width == 0.0 {
        return String::new();
    }
    let mut data = String::new();
    for segment in points.windows(2) {
        let start = segment[0];
        let dx = segment[1].0 - start.0;
        let dy = segment[1].1 - start.1;
        let length = dx.hypot(dy);
        if length <= f64::EPSILON {
            continue;
        }
        let tangent = (dx / length, dy / length);
        if style.ends_with("path_directional") {
            // Scale the 50 px-high source motif so its rendered height equals
            // the requested Wonderdraft path width.
            let scale = requested_width / 50.0;
            let pattern_height = 50.0 * scale;
            let pattern_length = 65.0 * scale;
            let repeat = 63.0 * scale;
            let mut offset = 0.0;
            while offset < length {
                let available = (length - offset).min(pattern_length);
                if available >= pattern_length * 0.45 {
                    let half = pattern_height / 2.0;
                    let notch = 17.0 * scale;
                    let points = [
                        local_point(start, tangent, offset, -half),
                        local_point(start, tangent, offset + available - notch, -half),
                        local_point(start, tangent, offset + available, 0.0),
                        local_point(start, tangent, offset + available - notch, half),
                        local_point(start, tangent, offset, half),
                        local_point(start, tangent, offset + notch, 0.0),
                    ];
                    data.push_str(&polygon_data(&points));
                    data.push(' ');
                }
                offset += repeat;
            }
        } else if style.ends_with("path_double_paired") {
            let scale = requested_width / 50.0;
            let pattern_height = 50.0 * scale;
            let strip = (pattern_height * 0.18).max(0.1);
            let center_offset = (pattern_height - strip) / 2.0;
            for center in [-center_offset, center_offset] {
                let points = [
                    local_point(start, tangent, 0.0, center - strip / 2.0),
                    local_point(start, tangent, length, center - strip / 2.0),
                    local_point(start, tangent, length, center + strip / 2.0),
                    local_point(start, tangent, 0.0, center + strip / 2.0),
                ];
                data.push_str(&polygon_data(&points));
                data.push(' ');
            }
        } else if style.ends_with("path_hash_marks") {
            let scale = requested_width / 100.0;
            let pattern_height = 100.0 * scale;
            let mark_width = (pattern_height * 0.08).max(0.1);
            let repeat = pattern_height * 2.3;
            let mut offset = 0.0;
            while offset < length {
                for (along, height) in [
                    (offset, pattern_height),
                    (offset + repeat * 0.52, pattern_height * 0.36),
                ] {
                    if along >= length {
                        continue;
                    }
                    let half_w = mark_width / 2.0;
                    let half_h = height / 2.0;
                    let points = [
                        local_point(start, tangent, along - half_w, -half_h),
                        local_point(start, tangent, along + half_w, -half_h),
                        local_point(start, tangent, along + half_w, half_h),
                        local_point(start, tangent, along - half_w, half_h),
                    ];
                    data.push_str(&polygon_data(&points));
                    data.push(' ');
                }
                offset += repeat;
            }
        }
    }
    data.trim_end().to_owned()
}

fn label_glow(record: &Value) -> Option<(String, f64, String, f64)> {
    let size = n(record.get("glow_size"), 0.0);
    if !size.is_finite() || size <= 0.0 {
        return None;
    }
    let glow_color = record.get("glow_color")?;
    let (color, alpha) = color(Some(glow_color), [1.0, 1.0, 1.0, 1.0]);
    Some((format!("{size}|{color}|{alpha}"), size, color, alpha))
}

pub fn export(
    root: &Value,
    imageset: &Images,
    dest: &Path,
    resolver: &Resolver,
    options: ExportOptions,
) -> Result<Summary> {
    let width = n(root.get("map_width"), 512.);
    let height = n(root.get("map_height"), 512.);
    let mut summary = Summary::default();
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:inkscape=\"http://www.inkscape.org/namespaces/inkscape\" xmlns:wd=\"{WD}\" width=\"{width}px\" height=\"{height}px\" viewBox=\"0 0 {width} {height}\" wd:format-version=\"2\" wd:map-width=\"{width}\" wd:map-height=\"{height}\">\n  <metadata>Wonderdraft Map Editor SVG interchange file</metadata>\n"
    );
    if options.background {
        xml.push_str(&layer_open("wonderdraft-mask-background"));
        if let Some((_, mask)) = imageset
            .iter()
            .find(|(k, _)| k.split('.').next_back() == Some("mask"))
            && let Some(info) = crate::value::image_info(mask)
        {
            let png_path = dest.with_file_name(format!(
                "{}.mask.png",
                dest.file_stem().unwrap_or_default().to_string_lossy()
            ));
            images::export_png(&png_path, &info)?;
            let href = if options.embed_background {
                let raw = fs::read(&png_path).map_err(|e| Error::format(e.to_string()))?;
                let _ = fs::remove_file(&png_path);
                format!("data:image/png;base64,{}", b64(&raw, false))
            } else {
                png_path.file_name().unwrap().to_string_lossy().into_owned()
            };
            summary.background = if options.embed_background {
                "embedded".into()
            } else {
                href.clone()
            };
            xml.push_str(&format!("    <image x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\" preserveAspectRatio=\"none\" xlink:href=\"{}\" wd:kind=\"background\" wd:image-key=\"mask\"/>\n",esc(&href)));
        }
        xml.push_str("  </g>\n");
    } else {
        summary.background = "excluded".into();
    }
    if options.boxes {
        xml.push_str(&layer_open("wonderdraft-boxes"));
        if let Some(boxes) = root.get("boxes").and_then(Value::as_array) {
            for (index, box_record) in boxes.iter().enumerate() {
                if matches!(property(box_record, "visible"), Some(Value::Bool(false))) {
                    continue;
                }
                let left = n(property(box_record, "margin_left"), 0.0);
                let top = n(property(box_record, "margin_top"), 0.0);
                let right = n(property(box_record, "margin_right"), left);
                let bottom = n(property(box_record, "margin_bottom"), top);
                let box_width = (right - left).abs();
                let box_height = (bottom - top).abs();
                if box_width <= 0.0 || box_height <= 0.0 {
                    summary.warnings.push(format!(
                        "Skipped box {index}: its margin rectangle has no area"
                    ));
                    continue;
                }
                let image_key = format!("boxes.{index}.properties.texture.properties.image");
                let Some((_, image)) = imageset.iter().find(|(key, _)| key == &image_key) else {
                    summary
                        .warnings
                        .push(format!("Skipped box {index}: embedded image is missing"));
                    continue;
                };
                let Some(info) = crate::value::image_info(image) else {
                    summary
                        .warnings
                        .push(format!("Skipped box {index}: embedded image is invalid"));
                    continue;
                };
                let png_path = dest.with_file_name(format!(
                    "{}.box-{index}.png",
                    dest.file_stem().unwrap_or_default().to_string_lossy()
                ));
                images::export_png(&png_path, &info)?;
                let href = if options.embed_boxes {
                    let raw = fs::read(&png_path).map_err(|error| {
                        Error::format(format!(
                            "could not read exported box image {}: {error}",
                            png_path.display()
                        ))
                    })?;
                    let _ = fs::remove_file(&png_path);
                    format!("data:image/png;base64,{}", b64(&raw, false))
                } else {
                    png_path
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned()
                };
                let opacity = color(property(box_record, "modulate"), [1.0, 1.0, 1.0, 1.0]).1
                    * color(property(box_record, "self_modulate"), [1.0, 1.0, 1.0, 1.0]).1;
                xml.push_str(&format!(
                    "    <image id=\"wonderdraft-box-{index}\" x=\"{}\" y=\"{}\" width=\"{box_width}\" height=\"{box_height}\" preserveAspectRatio=\"none\" opacity=\"{opacity}\" xlink:href=\"{}\" wd:kind=\"box\" wd:image-key=\"{image_key}\" wd:record=\"{}\"/>\n",
                    left.min(right),
                    top.min(bottom),
                    esc(&href),
                    record(box_record)
                ));
                summary.boxes += 1;
            }
        }
        xml.push_str("  </g>\n");
    }
    if options.paths {
        xml.push_str(&layer_open("wonderdraft-paths"));
        if let Some(a) = root.get("paths").and_then(Value::as_array) {
            for (i, r) in a.iter().enumerate() {
                let Some(p) = points(r) else { continue };
                if p.len() < 2 {
                    continue;
                }
                let pos = vec2(r.get("position"), (0., 0.));
                let (c, op) = color(r.get("color"), [0.2, 0.1, 0.05, 1.]);
                let width = n(r.get("width"), 3.).max(0.0);
                let style = r.get("style").and_then(Value::as_str).unwrap_or("");
                let data = svg_path_data(&p, pos, false);
                let metadata = format!(
                    "id=\"wonderdraft-path-{i}\" d=\"{data}\" wd:kind=\"path\" wd:style=\"{}\" wd:record=\"{}\"",
                    esc(style),
                    record(r)
                );
                if matches!(
                    style.rsplit('/').next(),
                    Some("path_directional" | "path_double_paired" | "path_hash_marks")
                ) {
                    let visual = path_pattern_data(&positioned_points(&p, pos), width, style);
                    xml.push_str(&format!(
                        "    <path {metadata} fill=\"none\" stroke=\"none\"/>\n    <path d=\"{visual}\" fill=\"{c}\" fill-opacity=\"{op}\" stroke=\"none\" wd:role=\"path-style\"/>\n"
                    ));
                } else if style.ends_with("path_solid_outlined") {
                    xml.push_str(&format!(
                        "    <path d=\"{data}\" fill=\"none\" stroke=\"#000000\" stroke-opacity=\"{op}\" stroke-width=\"{}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" wd:role=\"path-style\"/>\n    <path {metadata} fill=\"none\" stroke=\"{c}\" stroke-opacity=\"{op}\" stroke-width=\"{width}\" stroke-linecap=\"round\" stroke-linejoin=\"round\"/>\n",
                        width * 1.5
                    ));
                } else {
                    let extra = if style.ends_with("path_circle") {
                        format!(
                            " stroke-linecap=\"round\" stroke-dasharray=\"0 {}\"",
                            width * 2.0
                        )
                    } else if style.ends_with("path_dash_dot_dot") {
                        format!(
                            " stroke-linecap=\"round\" stroke-dasharray=\"{} {} {} {} {} {}\"",
                            width * 1.5,
                            width * 1.5,
                            width * 0.01,
                            width * 1.5,
                            width * 0.01,
                            width * 1.5
                        )
                    } else if style.ends_with("path_dash_dot") {
                        format!(
                            " stroke-linecap=\"round\" stroke-dasharray=\"{width} {} {} {}\"",
                            width * 2.0,
                            width * 0.01,
                            width * 2.0
                        )
                    } else if style.ends_with("path_dash") {
                        format!(" stroke-dasharray=\"{} {}\"", width * 2.0, width * 2.0)
                    } else {
                        String::new()
                    };
                    xml.push_str(&format!(
                        "    <path {metadata} fill=\"none\" stroke=\"{c}\" stroke-opacity=\"{op}\" stroke-width=\"{width}\"{extra}/>\n"
                    ));
                }
                summary.paths += 1;
            }
        }
        xml.push_str("  </g>\n");
    }
    if options.territories {
        let territories = root
            .get("territories")
            .and_then(|territories| territories.get("territories"))
            .and_then(Value::as_array);
        if territories.is_some_and(|territories| {
            territories.iter().any(|territory| {
                territory
                    .get("style")
                    .and_then(Value::as_str)
                    .is_some_and(|style| style.ends_with("border_gradient"))
            })
        }) {
            xml.push_str(
                "  <defs id=\"wonderdraft-territory-filters\">\n    <filter id=\"wonderdraft-territory-gradient-blur\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\" color-interpolation-filters=\"sRGB\">\n      <feGaussianBlur stdDeviation=\"10\"/>\n    </filter>\n  </defs>\n",
            );
        }
        xml.push_str(&layer_open("wonderdraft-territories"));
        if let Some(territories) = territories {
            for (index, territory) in territories.iter().enumerate() {
                let Some(points) = points(territory) else {
                    continue;
                };
                if points.len() < 2 {
                    continue;
                }
                let position = vec2(territory.get("position"), (0.0, 0.0));
                let data = svg_path_data(&points, position, true);
                let (territory_color, _) = color(territory.get("color"), [0.0, 0.0, 0.0, 1.0]);
                let opacity = n(territory.get("opacity"), 1.0).clamp(0.0, 1.0);
                let width = n(territory.get("width"), 1.0).max(0.0);
                let style = territory.get("style").and_then(Value::as_str).unwrap_or("");
                let metadata = format!(
                    "id=\"wonderdraft-territory-{index}\" d=\"{data}\" fill=\"{territory_color}\" fill-opacity=\"{opacity}\" wd:kind=\"territory\" wd:style=\"{}\" wd:record=\"{}\"",
                    esc(style),
                    record(territory)
                );
                if style.ends_with("border_gradient") {
                    xml.push_str(&format!(
                        "    <path {metadata} stroke=\"none\"/>\n    <path d=\"{data}\" fill=\"none\" stroke=\"{territory_color}\" stroke-opacity=\"1\" stroke-width=\"{}\" stroke-linejoin=\"round\" filter=\"url(#wonderdraft-territory-gradient-blur)\" wd:role=\"territory-border\"/>\n",
                        width * 2.0
                    ));
                } else if style.ends_with("border_dash") {
                    xml.push_str(&format!(
                        "    <path {metadata} stroke=\"{territory_color}\" stroke-opacity=\"1\" stroke-width=\"{width}\" stroke-linejoin=\"round\" stroke-dasharray=\"{} {}\"/>\n",
                        width * 2.0,
                        width
                    ));
                } else if style.ends_with("border_dark_dot") {
                    let dotted_width = width * 0.42;
                    xml.push_str(&format!(
                        "    <path {metadata} stroke=\"#000000\" stroke-opacity=\"1\" stroke-width=\"{dotted_width}\" stroke-linecap=\"round\" stroke-linejoin=\"round\" stroke-dasharray=\"0 {}\"/>\n",
                        dotted_width * 2.0
                    ));
                } else {
                    xml.push_str(&format!(
                        "    <path {metadata} stroke=\"{territory_color}\" stroke-opacity=\"1\" stroke-width=\"{width}\" stroke-linejoin=\"round\"/>\n"
                    ));
                }
                summary.territories += 1;
            }
        }
        xml.push_str("  </g>\n");
    }
    if options.symbols {
        let mut outline_filters = HashMap::new();
        if let Some(symbols) = root.get("symbols").and_then(Value::as_array) {
            let mut definitions = String::new();
            for record in symbols {
                let texture = record.get("texture").and_then(Value::as_str).unwrap_or("");
                if resolver.asset_info(texture).is_none() {
                    continue;
                }
                let Some((key, width, color, alpha)) = outline_style(record) else {
                    continue;
                };
                if outline_filters.contains_key(&key) {
                    continue;
                }
                let id = format!(
                    "wonderdraft-symbol-outline-filter-{}",
                    outline_filters.len()
                );
                definitions.push_str(&format!(
                    "    <filter id=\"{id}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\" color-interpolation-filters=\"sRGB\">\n      <feMorphology in=\"SourceAlpha\" operator=\"dilate\" radius=\"{width}\" result=\"wonderdraft-outline-mask-outer\"/>\n      <feComposite in=\"wonderdraft-outline-mask-outer\" in2=\"SourceAlpha\" operator=\"out\" result=\"wonderdraft-outline-mask\"/>\n      <feFlood flood-color=\"{color}\" flood-opacity=\"{alpha}\" result=\"wonderdraft-outline-color\"/>\n      <feComposite in=\"wonderdraft-outline-color\" in2=\"wonderdraft-outline-mask\" operator=\"in\" result=\"wonderdraft-outline\"/>\n      <feMerge>\n        <feMergeNode in=\"wonderdraft-outline\"/>\n        <feMergeNode in=\"SourceGraphic\"/>\n      </feMerge>\n    </filter>\n"
                ));
                outline_filters.insert(key, id);
            }
            if !definitions.is_empty() {
                xml.push_str("  <defs id=\"wonderdraft-symbol-outline-filters\">\n");
                xml.push_str(&definitions);
                xml.push_str("  </defs>\n");
            }
        }
        let mut custom_color_filters = HashMap::new();
        if let Some(symbols) = root.get("symbols").and_then(Value::as_array) {
            let mut definitions = String::new();
            for record in symbols {
                let texture = record.get("texture").and_then(Value::as_str).unwrap_or("");
                if resolver.asset_info(texture).is_none() {
                    continue;
                }
                let Some(matrix) = custom_color_matrix(record) else {
                    continue;
                };
                if custom_color_filters.contains_key(&matrix) {
                    continue;
                }
                let id = format!(
                    "wonderdraft-symbol-color-filter-{}",
                    custom_color_filters.len()
                );
                definitions.push_str(&format!(
                    "    <filter id=\"{id}\" color-interpolation-filters=\"sRGB\">\n      <feColorMatrix type=\"matrix\" values=\"{matrix}\" result=\"wonderdraft-recolored\"/>\n      <feComposite in=\"wonderdraft-recolored\" in2=\"SourceGraphic\" operator=\"in\"/>\n    </filter>\n"
                ));
                custom_color_filters.insert(matrix, id);
            }
            if !definitions.is_empty() {
                xml.push_str("  <defs id=\"wonderdraft-symbol-color-filters\">\n");
                xml.push_str(&definitions);
                xml.push_str("  </defs>\n");
            }
        }
        let mut embedded_symbols = HashMap::new();
        if options.embed_symbols
            && let Some(symbols) = root.get("symbols").and_then(Value::as_array)
        {
            let mut definitions = String::new();
            for record in symbols {
                let texture = record.get("texture").and_then(Value::as_str).unwrap_or("");
                let Some(asset) = resolver.asset_info(texture) else {
                    continue;
                };
                let key = symbol_path_key(&asset.path);
                if embedded_symbols.contains_key(&key) {
                    continue;
                }
                let id = format!("wonderdraft-symbol-definition-{}", embedded_symbols.len());
                let href = png_data_uri(&asset.path)?;
                definitions.push_str(&format!(
                    "    <symbol id=\"{id}\" viewBox=\"0 0 {} {}\" preserveAspectRatio=\"none\" wd:texture=\"{}\">\n      <image x=\"0\" y=\"0\" width=\"{}\" height=\"{}\" preserveAspectRatio=\"none\" xlink:href=\"{}\"/>\n    </symbol>\n",
                    asset.width,
                    asset.height,
                    esc(texture),
                    asset.width,
                    asset.height,
                    esc(&href)
                ));
                embedded_symbols.insert(key, id);
            }
            if !definitions.is_empty() {
                xml.push_str("  <defs id=\"wonderdraft-symbol-definitions\">\n");
                xml.push_str(&definitions);
                xml.push_str("  </defs>\n");
            }
        }
        xml.push_str(&layer_open("wonderdraft-symbols"));
        if let Some(a) = root.get("symbols").and_then(Value::as_array) {
            for (i, r) in a.iter().enumerate() {
                let texture = r.get("texture").and_then(Value::as_str).unwrap_or("");
                let (x, y) = vec2(r.get("position"), (0., 0.));
                let radius = n(r.get("radius"), 16.);
                let (sample, alpha) = color(r.get("sample"), [1., 1., 1., 1.]);
                let scale = vec2(r.get("scale"), (1., 1.));
                let rotation = n(r.get("rotation"), 0.0);
                let mirrored = matches!(r.get("mirror"), Some(Value::Bool(true)));
                if let Some(asset) = resolver.asset_info(texture) {
                    let w = (asset.width * scale.0).abs().max(0.001);
                    let h = (asset.height * scale.1).abs().max(0.001);
                    let offset = vec2(r.get("offset"), (asset.offset_x, asset.offset_y));
                    let visual_center = (x + offset.0 * scale.0, y + offset.1 * scale.1);
                    let image_x = visual_center.0 - w / 2.0;
                    let image_y = visual_center.1 - h / 2.0;
                    let mut transform = IDENTITY;
                    if mirrored {
                        transform = mat_mul(transform, mirror_about_y(visual_center.1));
                    }
                    if rotation != 0.0 {
                        transform = mat_mul(rotation_about(rotation, (x, y)), transform);
                    }
                    let transform_attr = if transform != IDENTITY {
                        format!(" transform=\"{}\"", matrix_text(transform))
                    } else {
                        String::new()
                    };
                    let filter_attr = custom_color_matrix(r)
                        .and_then(|matrix| custom_color_filters.get(&matrix))
                        .map(|id| format!(" filter=\"url(#{id})\""))
                        .unwrap_or_default();
                    let outline_filter =
                        outline_style(r).and_then(|(key, _, _, _)| outline_filters.get(&key));
                    let common = format!(
                        "id=\"wonderdraft-symbol-{i}\" x=\"{image_x}\" y=\"{image_y}\" width=\"{w}\" height=\"{h}\" opacity=\"{alpha}\" wd:kind=\"symbol\" wd:texture=\"{}\" wd:record=\"{}\" wd:source-width=\"{}\" wd:source-height=\"{}\" wd:base-radius=\"{}\" wd:offset-x=\"{}\" wd:offset-y=\"{}\" wd:export-width=\"{w}\" wd:export-height=\"{h}\"{transform_attr}{filter_attr}",
                        esc(texture),
                        record(r),
                        asset.width,
                        asset.height,
                        asset.base_radius,
                        offset.0,
                        offset.1
                    );
                    let element = if let Some(definition) =
                        embedded_symbols.get(&symbol_path_key(&asset.path))
                    {
                        format!(
                            "    <use {common} href=\"#{definition}\" xlink:href=\"#{definition}\"/>\n"
                        )
                    } else {
                        let href = url::Url::from_file_path(&asset.path)
                            .map(|url| url.to_string())
                            .unwrap_or_else(|_| asset.path.to_string_lossy().into_owned());
                        format!("    <image {common} xlink:href=\"{}\"/>\n", esc(&href))
                    };
                    if let Some(filter) = outline_filter {
                        xml.push_str(&format!(
                            "    <g id=\"wonderdraft-symbol-outline-{i}\" filter=\"url(#{filter})\">\n  {element}    </g>\n"
                        ));
                    } else {
                        xml.push_str(&element);
                    }
                } else {
                    summary.missing_symbols += 1;
                    let offset = vec2(r.get("offset"), (0.0, 0.0));
                    let rotated = rotate_vector((offset.0 * scale.0, offset.1 * scale.1), rotation);
                    let visual = (x + rotated.0, y + rotated.1);
                    let display_radius = (radius * scale.0.abs().max(scale.1.abs())).max(1.0);
                    xml.push_str(&format!("    <circle id=\"wonderdraft-symbol-{i}\" cx=\"{}\" cy=\"{}\" r=\"{display_radius}\" fill=\"{sample}\" fill-opacity=\"{alpha}\" stroke=\"#ff00ff\" wd:kind=\"symbol\" wd:texture=\"{}\" wd:record=\"{}\" wd:offset-x=\"{}\" wd:offset-y=\"{}\"/>\n",visual.0,visual.1,esc(texture),record(r),offset.0,offset.1));
                }
                summary.symbols += 1;
            }
        }
        xml.push_str("  </g>\n");
    }
    if options.labels {
        let font_mapping = match fonts::load_name_mapping() {
            Ok(mapping) => mapping,
            Err(error) => {
                summary
                    .warnings
                    .push(format!("Could not read the font-name map: {error}"));
                HashMap::new()
            }
        };
        let mut glow_filters = HashMap::new();
        if let Some(labels) = root.get("labels").and_then(Value::as_array) {
            let mut definitions = String::new();
            for record in labels {
                let Some((key, size, color, alpha)) = label_glow(record) else {
                    continue;
                };
                if glow_filters.contains_key(&key) {
                    continue;
                }
                let id = format!("wonderdraft-label-glow-{}", glow_filters.len());
                definitions.push_str(&format!(
                    "    <filter id=\"{id}\" x=\"-50%\" y=\"-50%\" width=\"200%\" height=\"200%\" color-interpolation-filters=\"sRGB\" inkscape:label=\"Drop Shadow\">\n      <feFlood result=\"flood\" in=\"SourceGraphic\" flood-opacity=\"{alpha}\" flood-color=\"{color}\"/>\n      <feGaussianBlur result=\"blur\" in=\"SourceGraphic\" stdDeviation=\"{size}\"/>\n      <feOffset result=\"offset\" in=\"blur\" dx=\"0\" dy=\"0\"/>\n      <feComposite result=\"comp1\" operator=\"in\" in=\"flood\" in2=\"offset\"/>\n      <feComposite result=\"comp2\" operator=\"over\" in=\"SourceGraphic\" in2=\"comp1\"/>\n    </filter>\n"
                ));
                glow_filters.insert(key, id);
            }
            if !definitions.is_empty() {
                xml.push_str("  <defs id=\"wonderdraft-label-glow-filters\">\n");
                xml.push_str(&definitions);
                xml.push_str("  </defs>\n");
            }
        }
        xml.push_str(&layer_open("wonderdraft-labels"));
        if let Some(a) = root.get("labels").and_then(Value::as_array) {
            for (i, r) in a.iter().enumerate() {
                let (x, y) = vec2(r.get("position"), (0., 0.));
                let size = n(r.get("size"), 24.);
                let text = r.get("text").and_then(Value::as_str).unwrap_or("");
                let font_label = r
                    .get("font")
                    .and_then(Value::as_str)
                    .unwrap_or("sans-serif");
                let mapped_font = fonts::mapped_name(&font_mapping, font_label);
                let family = mapped_font
                    .map(|font| font.family.as_str())
                    .unwrap_or(font_label);
                let font_style = mapped_font
                    .map(|font| font.style.as_str())
                    .unwrap_or("normal");
                let font_weight = mapped_font
                    .map(|font| font.weight.as_str())
                    .unwrap_or("normal");
                let specification = if font_style == "normal" && font_weight == "normal" {
                    family.to_owned()
                } else {
                    let variants = [font_weight, font_style]
                        .into_iter()
                        .filter(|value| *value != "normal")
                        .map(|value| {
                            value
                                .chars()
                                .next()
                                .map(char::to_uppercase)
                                .into_iter()
                                .flatten()
                                .chain(value.chars().skip(1))
                                .collect::<String>()
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("{family} {variants}")
                };
                let (fill, op) = color(r.get("color"), [0., 0., 0., 1.]);
                let anchor = match r.get("align").and_then(Value::as_f64).unwrap_or(1.) as i32 {
                    0 => "start",
                    2 => "end",
                    _ => "middle",
                };
                let rotation = n(r.get("rotation"), 0.0);
                let transform = if rotation != 0.0 {
                    format!(" transform=\"rotate({} {x} {y})\"", rotation.to_degrees())
                } else {
                    String::new()
                };
                let outline = outline_style(r)
                    .map(|(_, width, color, alpha)| {
                        format!(" stroke=\"{color}\" stroke-opacity=\"{alpha}\" stroke-width=\"{width}\" stroke-linecap=\"round\" paint-order=\"markers stroke fill\"")
                    })
                    .unwrap_or_else(|| " stroke=\"none\"".into());
                let glow = label_glow(r)
                    .and_then(|(key, _, _, _)| glow_filters.get(&key))
                    .map(|id| format!(" filter=\"url(#{id})\""))
                    .unwrap_or_default();
                let css_family = specification.replace('\\', "\\\\").replace('\'', "\\'");
                xml.push_str(&format!("    <text id=\"wonderdraft-label-{i}\" x=\"{x}\" y=\"{y}\" font-family=\"{}\" font-style=\"{font_style}\" font-weight=\"{font_weight}\" font-size=\"{size}px\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" fill=\"{fill}\" fill-opacity=\"{op}\"{outline}{glow} style=\"font-style:{font_style};font-weight:{font_weight};-inkscape-font-specification:'{}';paint-order:markers stroke fill\" wd:kind=\"label\" wd:font-label=\"{}\" wd:record=\"{}\"{transform}>{}</text>\n",esc(family),esc(&css_family),esc(font_label),record(r),esc(text)));
                summary.labels += 1;
            }
        }
        xml.push_str("  </g>\n");
    }
    xml.push_str("</svg>\n");
    fs::write(dest, xml).map_err(|e| Error::format(e.to_string()))?;
    Ok(summary)
}

struct El {
    tag: String,
    attrs: Vec<(String, String)>,
    text: String,
    matrix: Matrix,
    layer: Option<String>,
}
impl Default for El {
    fn default() -> Self {
        Self {
            tag: String::new(),
            attrs: Vec::new(),
            text: String::new(),
            matrix: IDENTITY,
            layer: None,
        }
    }
}
fn attr<'a>(e: &'a El, name: &str) -> Option<&'a str> {
    e.attrs
        .iter()
        .find_map(|(k, v)| (k == name || k.ends_with(&format!(":{name}"))).then_some(v.as_str()))
}
fn presentation<'a>(e: &'a El, name: &str) -> Option<&'a str> {
    attr(e, "style")
        .and_then(|style| {
            style.split(';').find_map(|declaration| {
                let (property, value) = declaration.split_once(':')?;
                (property.trim() == name).then_some(value.trim())
            })
        })
        .or_else(|| attr(e, name))
}

fn svg_color(value: &str) -> Option<[f32; 4]> {
    let value = value.trim();
    if value.eq_ignore_ascii_case("none") {
        return None;
    }
    if value.eq_ignore_ascii_case("transparent") {
        return Some([0.0, 0.0, 0.0, 0.0]);
    }
    let hex = value.strip_prefix('#')?;
    let digit = |index: usize| u8::from_str_radix(&hex[index..index + 1], 16).ok();
    let pair = |index: usize| u8::from_str_radix(&hex[index..index + 2], 16).ok();
    let (red, green, blue, alpha) = match hex.len() {
        3 => (digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255),
        4 => (
            digit(0)? * 17,
            digit(1)? * 17,
            digit(2)? * 17,
            digit(3)? * 17,
        ),
        6 => (pair(0)?, pair(2)?, pair(4)?, 255),
        8 => (pair(0)?, pair(2)?, pair(4)?, pair(6)?),
        _ => return None,
    };
    Some([
        red as f32 / 255.0,
        green as f32 / 255.0,
        blue as f32 / 255.0,
        alpha as f32 / 255.0,
    ])
}

fn presentation_opacity(element: &El, name: &str) -> f32 {
    presentation(element, name)
        .and_then(|value| value.parse::<f32>().ok())
        .unwrap_or(1.0)
        .clamp(0.0, 1.0)
}

fn presentation_color(element: &El, name: &str, opacity_name: &str) -> Option<[f32; 4]> {
    let mut color = svg_color(presentation(element, name)?)?;
    color[3] *=
        presentation_opacity(element, opacity_name) * presentation_opacity(element, "opacity");
    Some(color)
}

fn set_record_color(record: &mut Value, color: [f32; 4]) {
    record.set(
        "color",
        Value::Vector {
            kind: "Color".into(),
            values: color.to_vec(),
        },
    );
}

fn transformed_stroke_width(element: &El, default: f64) -> f64 {
    let width = f(element, "stroke-width", default);
    let (scale_x, scale_y, _, _) = matrix_scale_rotation(element.matrix);
    width * (scale_x * scale_y).sqrt()
}

fn territory_width_from_stroke(element: &El, record: &Value) -> Option<f64> {
    let stroke = presentation(element, "stroke")?;
    if stroke.eq_ignore_ascii_case("none") {
        return None;
    }
    let old_width = n(record.get("width"), 1.0);
    let visible_width = transformed_stroke_width(element, old_width);
    let style = attr(element, "style")
        .filter(|style| style.starts_with("res://"))
        .or_else(|| record.get("style").and_then(Value::as_str))
        .unwrap_or("");
    Some(if style.ends_with("border_dark_dot") {
        visible_width / 0.42
    } else {
        visible_width
    })
}
fn f(e: &El, name: &str, d: f64) -> f64 {
    presentation(e, name)
        .and_then(|v| v.trim_end_matches("px").parse().ok())
        .unwrap_or(d)
}
fn known_layer(value: &str) -> bool {
    matches!(
        value,
        "wonderdraft-labels"
            | "wonderdraft-symbols"
            | "wonderdraft-paths"
            | "wonderdraft-territories"
            | "wonderdraft-boxes"
            | "wonderdraft-mask-background"
    )
}

fn element_from(
    event: &quick_xml::events::BytesStart<'_>,
    parent: Matrix,
    parent_layer: Option<&str>,
) -> El {
    let mut element = El {
        tag: String::from_utf8_lossy(event.local_name().as_ref()).into_owned(),
        ..Default::default()
    };
    for attribute in event.attributes().flatten() {
        element.attrs.push((
            String::from_utf8_lossy(attribute.key.as_ref()).into_owned(),
            attribute.unescape_value().unwrap_or_default().into_owned(),
        ));
    }
    element.matrix = mat_mul(parent, parse_transform(attr(&element, "transform")));
    let layer = [attr(&element, "label"), attr(&element, "id")]
        .into_iter()
        .flatten()
        .find(|value| known_layer(value))
        .map(str::to_owned)
        .or_else(|| parent_layer.map(str::to_owned));
    element.layer = layer;
    element
}

fn inferred_kind(element: &El) -> Option<&'static str> {
    if attr(element, "role").is_some() {
        return None;
    }
    match (element.layer.as_deref(), element.tag.as_str()) {
        (Some("wonderdraft-labels"), "text") => Some("label"),
        (Some("wonderdraft-symbols"), "image" | "use" | "circle") => Some("symbol"),
        (Some("wonderdraft-paths"), "path" | "polyline") => Some("path"),
        (Some("wonderdraft-territories"), "path" | "polygon") => Some("territory"),
        _ => None,
    }
}

fn theme_color(value: Option<&Value>) -> Option<Value> {
    match value {
        Some(Value::Vector { kind, values }) if kind == "Color" && values.len() >= 4 => {
            Some(Value::Vector {
                kind: "Color".into(),
                values: values.clone(),
            })
        }
        Some(Value::String(value)) => {
            let values = value
                .split(',')
                .filter_map(|part| part.trim().parse::<f32>().ok())
                .collect::<Vec<_>>();
            (values.len() >= 4).then(|| Value::Vector {
                kind: "Color".into(),
                values,
            })
        }
        _ => None,
    }
}

fn town_label_record(root: &Value) -> Value {
    let preset = root
        .get("theme")
        .and_then(|theme| theme.get("label_presets"))
        .and_then(|presets| presets.get("Town"));
    let mut record = Value::dict();
    record.set("align", Value::Int(1));
    record.set("curve", Value::Real(0.0));
    record.set("extra_spacing_char", Value::Int(0));
    record.set("glow_size", Value::Int(0));
    record.set("rotation", Value::Real(0.0));
    record.set("z_index", Value::Int(0));
    if let Some(preset) = preset {
        if let Some(font) = preset.get("font_name").and_then(Value::as_str) {
            record.set("font", Value::String(font.to_owned()));
        }
        if let Some(size) = preset.get("font_size").and_then(Value::as_f64) {
            record.set("size", Value::Int(size.round() as i64));
        }
        if let Some(color) = theme_color(preset.get("font_color")) {
            record.set("color", color);
        }
        if let Some(width) = preset.get("font_outline_width").and_then(Value::as_f64) {
            record.set("outline_size", Value::Real(width));
        }
        if let Some(color) = theme_color(preset.get("font_outline_color")) {
            record.set("outline_color", color);
        }
    }
    record
}

fn transformed_rect(element: &El) -> ((f64, f64), f64, f64, f64, bool) {
    let x = f(element, "x", 0.);
    let y = f(element, "y", 0.);
    let w = f(element, "width", 0.);
    let h = f(element, "height", 0.);
    let p0 = mat_apply(element.matrix, x, y);
    let px = mat_apply(element.matrix, x + w, y);
    let py = mat_apply(element.matrix, x, y + h);
    let center = mat_apply(element.matrix, x + w / 2., y + h / 2.);
    let width = (px.0 - p0.0).hypot(px.1 - p0.1);
    let height = (py.0 - p0.0).hypot(py.1 - p0.1);
    let (_, _, raw, mirrored) = matrix_scale_rotation(element.matrix);
    let angle = normalize_angle(raw);
    (center, width, height, angle, mirrored)
}

#[derive(Clone, Copy, Debug)]
enum PathToken {
    Command(char),
    Number(f64),
}

fn path_tokens(data: &str) -> Vec<PathToken> {
    let bytes = data.as_bytes();
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let character = bytes[index] as char;
        if character.is_ascii_alphabetic() {
            tokens.push(PathToken::Command(character));
            index += 1;
            continue;
        }
        if character.is_ascii_whitespace() || character == ',' {
            index += 1;
            continue;
        }
        let start = index;
        if matches!(bytes[index], b'+' | b'-') {
            index += 1;
        }
        while index < bytes.len() && bytes[index].is_ascii_digit() {
            index += 1;
        }
        if index < bytes.len() && bytes[index] == b'.' {
            index += 1;
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        }
        if index < bytes.len() && matches!(bytes[index], b'e' | b'E') {
            index += 1;
            if index < bytes.len() && matches!(bytes[index], b'+' | b'-') {
                index += 1;
            }
            while index < bytes.len() && bytes[index].is_ascii_digit() {
                index += 1;
            }
        }
        if start == index {
            index += 1;
        } else if let Ok(number) = data[start..index].parse() {
            tokens.push(PathToken::Number(number));
        }
    }
    tokens
}

fn path_endpoints(data: &str) -> Vec<(f64, f64)> {
    let tokens = path_tokens(data);
    let mut points = Vec::new();
    let mut index = 0;
    let mut command = 'M';
    let mut current = (0.0, 0.0);
    let mut start = current;
    while index < tokens.len() {
        if let PathToken::Command(next) = tokens[index] {
            command = next;
            index += 1;
            if matches!(command, 'Z' | 'z') {
                current = start;
                continue;
            }
        }
        let count = match command.to_ascii_uppercase() {
            'M' | 'L' | 'T' => 2,
            'H' | 'V' => 1,
            'S' | 'Q' => 4,
            'C' => 6,
            'A' => 7,
            _ => {
                index += 1;
                continue;
            }
        };
        if index + count > tokens.len() {
            break;
        }
        if tokens[index..index + count]
            .iter()
            .any(|token| matches!(token, PathToken::Command(_)))
        {
            continue;
        }
        let values = tokens[index..index + count]
            .iter()
            .filter_map(|token| match token {
                PathToken::Number(value) => Some(*value),
                PathToken::Command(_) => None,
            })
            .collect::<Vec<_>>();
        index += count;
        let relative = command.is_ascii_lowercase();
        let endpoint = match command.to_ascii_uppercase() {
            'H' => (
                if relative {
                    current.0 + values[0]
                } else {
                    values[0]
                },
                current.1,
            ),
            'V' => (
                current.0,
                if relative {
                    current.1 + values[0]
                } else {
                    values[0]
                },
            ),
            'M' | 'L' | 'T' => {
                let offset = values.len() - 2;
                if relative {
                    (current.0 + values[offset], current.1 + values[offset + 1])
                } else {
                    (values[offset], values[offset + 1])
                }
            }
            'S' | 'Q' | 'C' | 'A' => {
                let offset = values.len() - 2;
                if relative {
                    (current.0 + values[offset], current.1 + values[offset + 1])
                } else {
                    (values[offset], values[offset + 1])
                }
            }
            _ => current,
        };
        current = endpoint;
        if matches!(command, 'M' | 'm') {
            start = current;
            command = if relative { 'l' } else { 'L' };
        }
        points.push(current);
    }
    points
}

fn transformed_points(element: &El, position: (f64, f64)) -> Vec<Vec<f32>> {
    let raw_points = if let Some(points) = attr(element, "points") {
        points
            .split_whitespace()
            .filter_map(|point| {
                let mut numbers = point.split(',').filter_map(|value| value.parse().ok());
                Some((numbers.next()?, numbers.next()?))
            })
            .collect()
    } else {
        path_endpoints(attr(element, "d").unwrap_or(""))
    };
    raw_points
        .into_iter()
        .map(|(x, y)| {
            let point = mat_apply(element.matrix, x, y);
            vec![(point.0 - position.0) as f32, (point.1 - position.1) as f32]
        })
        .collect()
}

fn replace_record_points(record: &mut Value, points: Vec<Vec<f32>>) {
    let replacement = match record.get("points") {
        Some(Value::String(_)) => Value::String(godot_text::format(&Value::Array(
            points
                .iter()
                .map(|point| Value::Vector {
                    kind: "Vector2".into(),
                    values: point.clone(),
                })
                .collect(),
        ))),
        Some(Value::Array(_)) => Value::Array(
            points
                .iter()
                .map(|point| Value::Vector {
                    kind: "Vector2".into(),
                    values: point.clone(),
                })
                .collect(),
        ),
        _ => Value::PoolVectors {
            kind: "PoolVector2Array".into(),
            components: 2,
            values: points,
        },
    };
    record.set("points", replacement);
}

pub fn import(root: &mut Value, source: &Path, resolver: &Resolver) -> Result<Summary> {
    let raw = fs::read_to_string(source).map_err(|e| Error::format(e.to_string()))?;
    let mut reader = Reader::from_str(&raw);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<El> = Vec::new();
    let mut found = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let parent = stack
                    .last()
                    .map(|element| element.matrix)
                    .unwrap_or(IDENTITY);
                let parent_layer = stack.last().and_then(|element| element.layer.as_deref());
                let el = element_from(&e, parent, parent_layer);
                stack.push(el)
            }
            Ok(Event::Empty(e)) => {
                let parent = stack
                    .last()
                    .map(|element| element.matrix)
                    .unwrap_or(IDENTITY);
                let parent_layer = stack.last().and_then(|element| element.layer.as_deref());
                let el = element_from(&e, parent, parent_layer);
                found.push(el)
            }
            Ok(Event::Text(t)) => {
                if let Some(e) = stack.last_mut() {
                    e.text.push_str(&t.decode().unwrap_or_default())
                }
            }
            Ok(Event::End(_)) => {
                if let Some(e) = stack.pop() {
                    if let Some(parent) = stack.last_mut() {
                        parent.text.push_str(&e.text);
                    }
                    found.push(e)
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => return Err(e.into()),
            _ => {}
        }
    }
    let mut labels = Vec::new();
    let mut symbols = Vec::new();
    let mut paths = Vec::new();
    let mut territories = Vec::new();
    let mut summary = Summary::default();
    let font_mapping = match fonts::load_name_mapping() {
        Ok(mapping) => mapping,
        Err(error) => {
            summary
                .warnings
                .push(format!("Could not read the font-name map: {error}"));
            HashMap::new()
        }
    };
    for e in found {
        match attr(&e, "kind").or_else(|| inferred_kind(&e)) {
            Some("label") => {
                let mut r = match attr(&e, "record") {
                    Some(record) => decode_record(record)?,
                    None => town_label_record(root),
                };
                let position = mat_apply(e.matrix, f(&e, "x", 0.), f(&e, "y", 0.));
                let (sx, sy, angle, _) = matrix_scale_rotation(e.matrix);
                r.set(
                    "position",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![position.0 as f32, position.1 as f32],
                    },
                );
                r.set(
                    "size",
                    Value::Int((f(&e, "font-size", 24.) * (sx * sy).sqrt()).round() as i64),
                );
                r.set("rotation", Value::Real(normalize_angle(angle)));
                r.set("text", Value::String(e.text.trim().to_owned()));
                if let Some(color) = presentation_color(&e, "fill", "fill-opacity") {
                    set_record_color(&mut r, color);
                }
                if let Some(font) = presentation(&e, "font-family") {
                    let family = font.trim_matches(['\'', '"']);
                    let style = presentation(&e, "font-style").unwrap_or("normal");
                    let weight = presentation(&e, "font-weight").unwrap_or("normal");
                    let original_label = r
                        .get("font")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned();
                    let original_still_matches = fonts::mapped_name(&font_mapping, &original_label)
                        .is_some_and(|mapped| {
                            mapped.family.eq_ignore_ascii_case(family)
                                && mapped.style.eq_ignore_ascii_case(style)
                                && mapped.weight.eq_ignore_ascii_case(weight)
                        });
                    let label = if original_still_matches {
                        original_label
                    } else {
                        fonts::wonderdraft_label_for_name(&font_mapping, family, style, weight)
                            .or_else(|| {
                                fonts::mapped_name(&font_mapping, family).map(|_| family.to_owned())
                            })
                            .or_else(|| r.get("font").and_then(Value::as_str).map(str::to_owned))
                            .unwrap_or_else(|| "sans-serif".to_owned())
                    };
                    r.set("font", Value::String(label));
                }
                labels.push(r);
                summary.labels += 1
            }
            Some("symbol") => {
                let mut r = attr(&e, "record")
                    .map(decode_record)
                    .transpose()?
                    .unwrap_or_else(Value::dict);
                let (visual_center, rendered_width, rendered_height, angle, mirrored) =
                    if e.tag == "circle" {
                        let center = mat_apply(e.matrix, f(&e, "cx", 0.), f(&e, "cy", 0.));
                        let (sx, sy, raw, mirrored) = matrix_scale_rotation(e.matrix);
                        let angle = normalize_angle(raw);
                        let radius = f(&e, "r", 1.);
                        (center, 2. * radius * sx, 2. * radius * sy, angle, mirrored)
                    } else {
                        transformed_rect(&e)
                    };
                let original_texture = r
                    .get("texture")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned();
                let mut texture = original_texture.clone();
                if let Some(href) = attr(&e, "href")
                    && let Ok(url) = url::Url::parse(href)
                    && let Ok(p) = url.to_file_path()
                    && let Some(t) = resolver.texture_for_path(&p)
                {
                    texture = t;
                }
                if texture.is_empty() {
                    texture = attr(&e, "texture")
                        .unwrap_or("res://sprites/symbols/custom_colors/s2_capital")
                        .to_owned();
                }
                let asset = resolver.asset_info(&texture);
                let source_width = f(
                    &e,
                    "source-width",
                    asset.as_ref().map(|a| a.width).unwrap_or(0.),
                );
                let source_height = f(
                    &e,
                    "source-height",
                    asset.as_ref().map(|a| a.height).unwrap_or(0.),
                );
                let old_scale = vec2(r.get("scale"), (1., 1.));
                let old_radius = n(r.get("radius"), 16.).max(0.000001);
                let texture_changed = !original_texture.is_empty() && texture != original_texture;
                let offset = if texture_changed {
                    asset
                        .as_ref()
                        .map(|a| (a.offset_x, a.offset_y))
                        .unwrap_or((0., 0.))
                } else {
                    vec2(
                        r.get("offset"),
                        asset
                            .as_ref()
                            .map(|a| (a.offset_x, a.offset_y))
                            .unwrap_or((0., 0.)),
                    )
                };
                let symbol_scale = if e.tag == "circle" {
                    (
                        rendered_width / (2. * old_radius),
                        rendered_height / (2. * old_radius),
                    )
                } else if source_width > 0. && source_height > 0. {
                    (
                        rendered_width / source_width,
                        rendered_height / source_height,
                    )
                } else {
                    let old_w = f(&e, "export-width", rendered_width);
                    let old_h = f(&e, "export-height", rendered_height);
                    (
                        old_scale.0 * rendered_width / old_w.max(0.000001),
                        old_scale.1 * rendered_height / old_h.max(0.000001),
                    )
                };
                let rotated = rotate_vector(
                    (offset.0 * symbol_scale.0, offset.1 * symbol_scale.1),
                    angle,
                );
                let position = (visual_center.0 - rotated.0, visual_center.1 - rotated.1);
                r.set("texture", Value::String(texture));
                r.set(
                    "position",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![position.0 as f32, position.1 as f32],
                    },
                );
                r.set(
                    "scale",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![symbol_scale.0 as f32, symbol_scale.1 as f32],
                    },
                );
                r.set(
                    "offset",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![offset.0 as f32, offset.1 as f32],
                    },
                );
                if texture_changed && let Some(asset) = asset {
                    r.set("radius", Value::Real(asset.base_radius));
                }
                r.set("rotation", Value::Real(angle));
                r.set("mirror", Value::Bool(mirrored));
                symbols.push(r);
                summary.symbols += 1
            }
            Some("path") => {
                let mut r = attr(&e, "record")
                    .map(decode_record)
                    .transpose()?
                    .unwrap_or_else(Value::dict);
                if attr(&e, "kind").is_none() {
                    r.set(
                        "style",
                        Value::String("res://textures/paths/path_blended".into()),
                    );
                }
                let position = vec2(r.get("position"), (0., 0.));
                let points = transformed_points(&e, position);
                replace_record_points(&mut r, points);
                if let Some(color) = presentation_color(&e, "stroke", "stroke-opacity") {
                    set_record_color(&mut r, color);
                }
                if presentation(&e, "stroke-width").is_some() {
                    let old_width = n(r.get("width"), 1.0);
                    r.set(
                        "width",
                        Value::Real(transformed_stroke_width(&e, old_width)),
                    );
                }
                paths.push(r);
                summary.paths += 1
            }
            Some("territory") => {
                let mut record = attr(&e, "record")
                    .map(decode_record)
                    .transpose()?
                    .unwrap_or_else(Value::dict);
                if attr(&e, "kind").is_none() {
                    record.set(
                        "style",
                        Value::String("res://textures/borders/border_solid".into()),
                    );
                }
                let position = vec2(record.get("position"), (0.0, 0.0));
                let points = transformed_points(&e, position);
                replace_record_points(&mut record, points);
                if let Some(mut color) = svg_color(presentation(&e, "fill").unwrap_or("none")) {
                    let fill_opacity = presentation_opacity(&e, "fill-opacity")
                        * presentation_opacity(&e, "opacity")
                        * color[3];
                    color[3] = 1.0;
                    set_record_color(&mut record, color);
                    record.set("opacity", Value::Real(fill_opacity as f64));
                }
                if let Some(width) = territory_width_from_stroke(&e, &record) {
                    record.set("width", Value::Real(width));
                }
                territories.push(record);
                summary.territories += 1;
            }
            _ => {}
        }
    }
    if !labels.is_empty() {
        root.set("labels", Value::Array(labels));
    }
    if !symbols.is_empty() {
        root.set("symbols", Value::Array(symbols));
    }
    if !paths.is_empty() {
        root.set("paths", Value::Array(paths));
    }
    if !territories.is_empty() {
        if root.get("territories").is_none() {
            root.set("territories", Value::dict());
        }
        if let Some(territory_data) = root.get_mut("territories") {
            territory_data.set("territories", Value::Array(territories));
        }
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::Settings;

    fn dict(entries: Vec<(&str, Value)>) -> Value {
        Value::Dictionary(
            entries
                .into_iter()
                .map(|(k, v)| (Value::String(k.into()), v))
                .collect(),
        )
    }

    #[test]
    fn export_options_exclude_layers_and_use_v2_symbol_geometry() {
        let base =
            std::env::temp_dir().join(format!("wonderdraft-svg-options-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let asset = base.join("icon.png");
        image::RgbaImage::new(10, 20).save(&asset).unwrap();
        fs::write(
            base.join(".wonderdraft_symbols"),
            r#"{"icon":{"radius":7,"offset_x":4,"offset_y":-2}}"#,
        )
        .unwrap();
        let symbol = dict(vec![
            ("texture", Value::String("user://assets/icon".into())),
            (
                "position",
                Value::Vector {
                    kind: "Vector2".into(),
                    values: vec![100., 200.],
                },
            ),
            (
                "scale",
                Value::Vector {
                    kind: "Vector2".into(),
                    values: vec![2., 3.],
                },
            ),
            ("radius", Value::Real(99.)),
            ("rotation", Value::Real(0.5)),
            ("mirror", Value::Bool(true)),
        ]);
        let root = dict(vec![
            ("map_width", Value::Int(512)),
            ("map_height", Value::Int(512)),
            ("symbols", Value::Array(vec![symbol])),
            (
                "labels",
                Value::Array(vec![dict(vec![("text", Value::String("Hidden".into()))])]),
            ),
            ("paths", Value::Array(vec![])),
        ]);
        let resolver = Resolver::new(&Settings {
            custom_asset_folder: base.to_string_lossy().into_owned(),
            ..Settings::default()
        });
        let destination = base.join("map.svg");
        let summary = export(
            &root,
            &Vec::new(),
            &destination,
            &resolver,
            ExportOptions {
                background: false,
                boxes: false,
                paths: false,
                symbols: true,
                labels: false,
                territories: false,
                embed_background: false,
                embed_boxes: false,
                embed_symbols: false,
            },
        )
        .unwrap();
        let xml = fs::read_to_string(destination).unwrap();
        assert!(xml.contains("wd:format-version=\"2\""));
        assert!(xml.contains("width=\"20\" height=\"60\""));
        assert!(xml.contains("x=\"98\" y=\"164\""));
        assert!(xml.contains("wd:base-radius=\"7\""));
        assert!(xml.contains("transform=\"matrix("));
        assert!(xml.contains("inkscape:groupmode=\"layer\""));
        assert!(xml.contains("inkscape:label=\"wonderdraft-symbols\""));
        assert!(!xml.contains("wonderdraft-labels"));
        assert!(!xml.contains("wonderdraft-paths"));
        assert!(!xml.contains("wonderdraft-mask-background"));
        assert!(!xml.contains("feColorMatrix"));
        assert_eq!(summary.symbols, 1);
        assert_eq!(summary.labels, 0);
        let mut imported = root.clone();
        let imported_summary = import(&mut imported, &base.join("map.svg"), &resolver).unwrap();
        let imported_symbol = &imported.get("symbols").unwrap().as_array().unwrap()[0];
        let position = vec2(imported_symbol.get("position"), (0.0, 0.0));
        let scale = vec2(imported_symbol.get("scale"), (0.0, 0.0));
        assert!((position.0 - 100.0).abs() < 0.001 && (position.1 - 200.0).abs() < 0.001);
        assert!((scale.0 - 2.0).abs() < 0.001 && (scale.1 - 3.0).abs() < 0.001);
        assert!((n(imported_symbol.get("rotation"), 0.0) - 0.5).abs() < 0.001);
        assert!(matches!(
            imported_symbol.get("mirror"),
            Some(Value::Bool(true))
        ));
        assert_eq!(imported_summary.symbols, 1);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn boxes_export_as_linked_or_embedded_stretched_images() {
        let base =
            std::env::temp_dir().join(format!("wonderdraft-svg-boxes-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let box_record = Value::Object {
            class: "NinePatchRect".into(),
            properties: vec![
                ("margin_left".into(), Value::Real(10.0)),
                ("margin_top".into(), Value::Real(20.0)),
                ("margin_right".into(), Value::Real(310.0)),
                ("margin_bottom".into(), Value::Real(120.0)),
                ("visible".into(), Value::Bool(true)),
                (
                    "modulate".into(),
                    Value::Vector {
                        kind: "Color".into(),
                        values: vec![1.0, 1.0, 1.0, 0.5],
                    },
                ),
            ],
        };
        let image = Value::Object {
            class: "Image".into(),
            properties: vec![(
                "data".into(),
                dict(vec![
                    ("width", Value::Int(2)),
                    ("height", Value::Int(2)),
                    ("format", Value::String("RGBA8".into())),
                    ("mipmaps", Value::Bool(false)),
                    (
                        "data",
                        Value::PoolByteArray(crate::ByteSource::Memory(vec![255; 16])),
                    ),
                ]),
            )],
        };
        let root = dict(vec![
            ("map_width", Value::Int(512)),
            ("map_height", Value::Int(512)),
            ("boxes", Value::Array(vec![box_record])),
        ]);
        let images = vec![("boxes.0.properties.texture.properties.image".into(), image)];
        let linked_svg = base.join("linked.svg");
        let linked = export(
            &root,
            &images,
            &linked_svg,
            &Resolver::new(&Settings::default()),
            ExportOptions {
                background: false,
                boxes: true,
                paths: false,
                symbols: false,
                labels: false,
                territories: false,
                embed_background: false,
                embed_boxes: false,
                embed_symbols: false,
            },
        )
        .unwrap();
        let xml = fs::read_to_string(&linked_svg).unwrap();
        assert_eq!(linked.boxes, 1);
        assert!(xml.contains("inkscape:label=\"wonderdraft-boxes\""));
        assert!(xml.contains("x=\"10\" y=\"20\" width=\"300\" height=\"100\""));
        assert!(xml.contains("preserveAspectRatio=\"none\" opacity=\"0.5\""));
        assert!(xml.contains("xlink:href=\"linked.box-0.png\""));
        assert!(base.join("linked.box-0.png").is_file());

        let embedded_svg = base.join("embedded.svg");
        let embedded = export(
            &root,
            &images,
            &embedded_svg,
            &Resolver::new(&Settings::default()),
            ExportOptions {
                background: false,
                boxes: true,
                paths: false,
                symbols: false,
                labels: false,
                territories: false,
                embed_background: false,
                embed_boxes: true,
                embed_symbols: false,
            },
        )
        .unwrap();
        let xml = fs::read_to_string(&embedded_svg).unwrap();
        assert_eq!(embedded.boxes, 1);
        assert!(xml.contains("xlink:href=\"data:image/png;base64,"));
        assert!(!base.join("embedded.box-0.png").exists());
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn rotations_are_clockwise_and_vertical_mirroring_happens_first() {
        let clockwise = rotation_about(std::f64::consts::FRAC_PI_2, (0.0, 0.0));
        let counter_clockwise = rotation_about(-std::f64::consts::FRAC_PI_2, (0.0, 0.0));
        let mirrored = mirror_about_y(0.0);
        let combined = mat_mul(clockwise, mirrored);

        let positive = mat_apply(clockwise, 2.0, 0.0);
        let negative = mat_apply(counter_clockwise, 2.0, 0.0);
        let flipped = mat_apply(mirrored, 2.0, 3.0);
        let flipped_then_rotated = mat_apply(combined, 2.0, 3.0);
        assert!(positive.0.abs() < 1e-9 && (positive.1 - 2.0).abs() < 1e-9);
        assert!(negative.0.abs() < 1e-9 && (negative.1 + 2.0).abs() < 1e-9);
        assert!((flipped.0 - 2.0).abs() < 1e-9 && (flipped.1 + 3.0).abs() < 1e-9);
        assert!(
            (flipped_then_rotated.0 - 3.0).abs() < 1e-9
                && (flipped_then_rotated.1 - 2.0).abs() < 1e-9
        );
        let (_, _, angle, is_mirrored) = matrix_scale_rotation(combined);
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-9);
        assert!(is_mirrored);
    }

    #[test]
    fn embedded_symbols_use_one_png_definition_and_cloned_instances() {
        let base = std::env::temp_dir().join(format!(
            "wonderdraft-svg-embedded-symbols-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        image::RgbaImage::new(10, 20)
            .save(base.join("shared.png"))
            .unwrap();
        let make_symbol = |x| {
            dict(vec![
                ("texture", Value::String("user://assets/shared".into())),
                (
                    "position",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![x, 50.],
                    },
                ),
                ("custom_color_mode", Value::Int(1)),
                (
                    "custom_colors",
                    Value::Array(vec![
                        Value::Vector {
                            kind: "Color".into(),
                            values: vec![1., 1., 0., 1.],
                        },
                        Value::Vector {
                            kind: "Color".into(),
                            values: vec![0., 0., 1., 1.],
                        },
                        Value::Vector {
                            kind: "Color".into(),
                            values: vec![0., 0., 0., 1.],
                        },
                    ]),
                ),
                ("outline_width", Value::Real(4.0)),
                (
                    "outline_color",
                    Value::Vector {
                        kind: "Color".into(),
                        values: vec![0., 0., 0., 0.5],
                    },
                ),
            ])
        };
        let root = dict(vec![
            ("map_width", Value::Int(512)),
            ("map_height", Value::Int(512)),
            (
                "symbols",
                Value::Array(vec![make_symbol(25.), make_symbol(75.)]),
            ),
        ]);
        let resolver = Resolver::new(&Settings {
            custom_asset_folder: base.to_string_lossy().into_owned(),
            ..Settings::default()
        });
        let destination = base.join("embedded.svg");

        let summary = export(
            &root,
            &Vec::new(),
            &destination,
            &resolver,
            ExportOptions {
                background: false,
                boxes: false,
                paths: false,
                symbols: true,
                labels: false,
                territories: false,
                embed_background: false,
                embed_boxes: false,
                embed_symbols: true,
            },
        )
        .unwrap();
        let xml = fs::read_to_string(&destination).unwrap();
        assert_eq!(xml.matches("data:image/png;base64,").count(), 1);
        assert_eq!(
            xml.matches("<symbol id=\"wonderdraft-symbol-definition-")
                .count(),
            1
        );
        assert_eq!(xml.matches("<use ").count(), 2);
        assert_eq!(
            xml.matches("<filter id=\"wonderdraft-symbol-color-filter-")
                .count(),
            1
        );
        assert!(xml.contains("values=\"1 0 0 0 0  1 0 0 0 0  0 1 0 0 0  1 1 1 0 0\""));
        assert!(xml.contains(
            "<feComposite in=\"wonderdraft-recolored\" in2=\"SourceGraphic\" operator=\"in\"/>"
        ));
        assert_eq!(
            xml.matches("filter=\"url(#wonderdraft-symbol-color-filter-")
                .count(),
            2
        );
        assert_eq!(
            xml.matches("<filter id=\"wonderdraft-symbol-outline-filter-")
                .count(),
            1
        );
        assert!(xml.contains("<feMorphology in=\"SourceAlpha\" operator=\"dilate\" radius=\"4\""));
        assert!(xml.contains("flood-color=\"#000000\" flood-opacity=\"0.5\""));
        assert_eq!(
            xml.matches("<g id=\"wonderdraft-symbol-outline-").count(),
            2
        );
        assert!(!xml.contains("file://"));
        assert_eq!(summary.symbols, 2);

        let mut imported = root.clone();
        let imported_summary = import(&mut imported, &destination, &resolver).unwrap();
        let imported_symbols = imported.get("symbols").unwrap().as_array().unwrap();
        assert_eq!(imported_symbols.len(), 2);
        assert!(imported_symbols.iter().all(|symbol| {
            symbol.get("texture").and_then(Value::as_str) == Some("user://assets/shared")
                && symbol.get("custom_color_mode").and_then(Value::as_f64) == Some(1.0)
        }));
        assert_eq!(imported_summary.symbols, 2);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn territories_export_requested_borders_and_round_trip_points() {
        let base = std::env::temp_dir().join(format!(
            "wonderdraft-svg-territories-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let make_territory = |style: &str, opacity: f64| {
            dict(vec![
                (
                    "color",
                    Value::Vector {
                        kind: "Color".into(),
                        values: vec![0.25, 0.5, 0.75, 0.4],
                    },
                ),
                ("opacity", Value::Real(opacity)),
                (
                    "points",
                    Value::String(
                        "[ Vector2( 10, 20 ), Vector2( 50, 20 ), Vector2( 50, 60 ) ]".into(),
                    ),
                ),
                (
                    "position",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![5.0, 7.0],
                    },
                ),
                ("style", Value::String(style.into())),
                ("width", Value::Real(10.0)),
            ])
        };
        let root = dict(vec![
            ("map_width", Value::Int(512)),
            ("map_height", Value::Int(512)),
            (
                "territories",
                dict(vec![(
                    "territories",
                    Value::Array(vec![
                        make_territory("res://textures/borders/border_gradient", 0.05),
                        make_territory("res://textures/borders/border_dash", 0.4),
                        make_territory("res://textures/borders/border_dark_dot", 0.8),
                    ]),
                )]),
            ),
        ]);
        let destination = base.join("territories.svg");
        let resolver = Resolver::new(&Settings::default());

        let summary = export(
            &root,
            &Vec::new(),
            &destination,
            &resolver,
            ExportOptions {
                background: false,
                boxes: false,
                paths: false,
                symbols: false,
                labels: false,
                territories: true,
                embed_background: false,
                embed_boxes: false,
                embed_symbols: false,
            },
        )
        .unwrap();
        let mut xml = fs::read_to_string(&destination).unwrap();
        assert_eq!(summary.territories, 3);
        assert!(xml.contains("<feGaussianBlur stdDeviation=\"10\"/>"));
        assert!(xml.contains("stroke-width=\"20\""));
        assert!(xml.contains("stroke-dasharray=\"20 10\""));
        assert!(xml.contains("stroke=\"#000000\""));
        assert!(xml.contains("stroke-width=\"4.2\""));
        assert!(xml.contains("stroke-dasharray=\"0 8.4\""));
        assert!(xml.contains("fill-opacity=\"0.05\""));
        assert!(xml.contains("stroke-opacity=\"1\""));
        assert!(xml.contains("<path id=\"wonderdraft-territory-"));
        assert!(!xml.contains("<polygon"));
        xml = xml.replacen("15.000000,27.000000", "25.000000,37.000000", 1);
        fs::write(&destination, xml).unwrap();

        let mut imported = root.clone();
        let imported_summary = import(&mut imported, &destination, &resolver).unwrap();
        assert_eq!(imported_summary.territories, 3);
        let territories = imported
            .get("territories")
            .and_then(|value| value.get("territories"))
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(territories.len(), 3);
        assert!(
            territories[0]
                .get("points")
                .and_then(Value::as_str)
                .unwrap()
                .contains("Vector2( 20, 30 )")
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn imports_css_path_color_width_and_territory_fill_width() {
        let base = std::env::temp_dir().join(format!(
            "wonderdraft-svg-presentation-import-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let path_record = dict(vec![
            (
                "color",
                Value::Vector {
                    kind: "Color".into(),
                    values: vec![0.1, 0.2, 0.3, 1.0],
                },
            ),
            (
                "points",
                Value::String("[ Vector2( 10, 20 ), Vector2( 50, 60 ) ]".into()),
            ),
            (
                "position",
                Value::Vector {
                    kind: "Vector2".into(),
                    values: vec![0.0, 0.0],
                },
            ),
            (
                "style",
                Value::String("res://textures/paths/path_dash".into()),
            ),
            ("width", Value::Real(7.0)),
        ]);
        let territory_record = dict(vec![
            (
                "color",
                Value::Vector {
                    kind: "Color".into(),
                    values: vec![0.0, 0.34, 0.7, 1.0],
                },
            ),
            ("opacity", Value::Real(0.25)),
            (
                "points",
                Value::String(
                    "[ Vector2( 100, 100 ), Vector2( 200, 100 ), Vector2( 150, 200 ) ]".into(),
                ),
            ),
            (
                "position",
                Value::Vector {
                    kind: "Vector2".into(),
                    values: vec![0.0, 0.0],
                },
            ),
            (
                "style",
                Value::String("res://textures/borders/border_dash".into()),
            ),
            ("width", Value::Real(10.0)),
        ]);
        let root = dict(vec![
            ("map_width", Value::Int(512)),
            ("map_height", Value::Int(512)),
            ("paths", Value::Array(vec![path_record])),
            (
                "territories",
                dict(vec![("territories", Value::Array(vec![territory_record]))]),
            ),
        ]);
        let destination = base.join("edited.svg");
        let resolver = Resolver::new(&Settings::default());
        export(
            &root,
            &Vec::new(),
            &destination,
            &resolver,
            ExportOptions {
                background: false,
                boxes: false,
                paths: true,
                symbols: false,
                labels: false,
                territories: true,
                embed_background: false,
                embed_boxes: false,
                embed_symbols: false,
            },
        )
        .unwrap();
        let xml = fs::read_to_string(&destination)
            .unwrap()
            .replacen(
                "id=\"wonderdraft-path-0\"",
                "id=\"wonderdraft-path-0\" style=\"stroke:#ff0000;stroke-width:18\"",
                1,
            )
            .replacen(
                "id=\"wonderdraft-territory-0\"",
                "id=\"wonderdraft-territory-0\" style=\"fill:#ffff00;stroke-width:14\"",
                1,
            );
        fs::write(&destination, xml).unwrap();

        let mut imported = root.clone();
        let summary = import(&mut imported, &destination, &resolver).unwrap();
        assert_eq!(summary.paths, 1);
        assert_eq!(summary.territories, 1);
        let imported_path = &imported.get("paths").and_then(Value::as_array).unwrap()[0];
        assert_eq!(
            imported_path.get("width").and_then(Value::as_f64),
            Some(18.0)
        );
        let Some(Value::Vector { kind, values }) = imported_path.get("color") else {
            panic!("imported path has no color");
        };
        assert_eq!(kind, "Color");
        assert_eq!(values, &[1.0, 0.0, 0.0, 1.0]);
        let imported_territory = &imported
            .get("territories")
            .and_then(|value| value.get("territories"))
            .and_then(Value::as_array)
            .unwrap()[0];
        assert_eq!(
            imported_territory.get("width").and_then(Value::as_f64),
            Some(14.0)
        );
        let Some(Value::Vector { kind, values }) = imported_territory.get("color") else {
            panic!("imported territory has no color");
        };
        assert_eq!(kind, "Color");
        assert_eq!(values, &[1.0, 1.0, 0.0, 1.0]);
        assert_eq!(
            imported_territory.get("opacity").and_then(Value::as_f64),
            Some(0.25)
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn svg_path_commands_are_imported_as_wonderdraft_points() {
        assert_eq!(
            path_endpoints("M 10,20 L 30,40 l 5,-5 H 50 v 10 C 1,2 3,4 60,70 Z"),
            vec![
                (10.0, 20.0),
                (30.0, 40.0),
                (35.0, 35.0),
                (50.0, 35.0),
                (50.0, 45.0),
                (60.0, 70.0)
            ]
        );
    }

    fn pattern_height(data: &str) -> f64 {
        let points = path_endpoints(data);
        let min_y = points
            .iter()
            .map(|point| point.1)
            .fold(f64::INFINITY, f64::min);
        let max_y = points
            .iter()
            .map(|point| point.1)
            .fold(f64::NEG_INFINITY, f64::max);
        max_y - min_y
    }

    #[test]
    fn patterned_paths_scale_source_height_to_requested_width() {
        for style in ["path_directional", "path_double_paired", "path_hash_marks"] {
            let data = path_pattern_data(
                &[(0.0, 0.0), (300.0, 0.0)],
                25.0,
                &format!("res://textures/paths/{style}"),
            );
            assert!(
                (pattern_height(&data) - 25.0).abs() < 0.000001,
                "{style} did not render at the requested 25 px width"
            );
        }
    }

    #[test]
    fn path_styles_export_as_strokes_or_fill_only_pattern_geometry() {
        let make_path = |style: &str| {
            dict(vec![
                (
                    "points",
                    Value::String("[ Vector2( 0, 0 ), Vector2( 200, 0 ) ]".into()),
                ),
                (
                    "color",
                    Value::Vector {
                        kind: "Color".into(),
                        values: vec![0.0, 0.5, 1.0, 1.0],
                    },
                ),
                ("style", Value::String(style.into())),
                ("width", Value::Real(25.0)),
            ])
        };
        let root = dict(vec![
            ("map_width", Value::Int(200)),
            ("map_height", Value::Int(100)),
            (
                "paths",
                Value::Array(vec![
                    make_path("res://textures/paths/path_circle"),
                    make_path("res://textures/paths/path_dash"),
                    make_path("res://textures/paths/path_dash_dot"),
                    make_path("res://textures/paths/path_dash_dot_dot"),
                    make_path("res://textures/paths/path_directional"),
                    make_path("res://textures/paths/path_double_paired"),
                    make_path("res://textures/paths/path_hash_marks"),
                ]),
            ),
        ]);
        let destination = std::env::temp_dir().join(format!(
            "wonderdraft-svg-path-styles-{}.svg",
            std::process::id()
        ));
        export(
            &root,
            &Vec::new(),
            &destination,
            &Resolver::new(&Settings::default()),
            ExportOptions {
                background: false,
                boxes: false,
                paths: true,
                symbols: false,
                labels: false,
                territories: false,
                embed_background: false,
                embed_boxes: false,
                embed_symbols: false,
            },
        )
        .unwrap();
        let xml = fs::read_to_string(&destination).unwrap();
        assert!(xml.contains("stroke-dasharray=\"0 50\""));
        assert!(xml.contains("stroke-dasharray=\"50 50\""));
        assert!(xml.contains("wd:style=\"res://textures/paths/path_directional\""));
        assert_eq!(
            xml.matches(
                "fill=\"#0080ff\" fill-opacity=\"1\" stroke=\"none\" wd:role=\"path-style\""
            )
            .count(),
            3
        );
        let _ = fs::remove_file(destination);
    }

    #[test]
    fn imports_untagged_elements_from_inkscape_layers_with_defaults() {
        let base = std::env::temp_dir().join(format!(
            "wonderdraft-svg-layer-import-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let source = base.join("layers.svg");
        fs::write(
            &source,
            r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape">
  <g inkscape:groupmode="layer" inkscape:label="wonderdraft-labels" id="labels">
    <text x="12" y="34" font-family="Definitely Missing" font-size="31" fill="#ff0080">New Town</text>
  </g>
  <g inkscape:groupmode="layer" inkscape:label="wonderdraft-paths" id="roads">
    <path d="M 1,2 L 30,40" fill="none" stroke="#123456" stroke-width="7"/>
  </g>
  <g inkscape:groupmode="layer" inkscape:label="wonderdraft-territories" id="areas">
    <path d="M 5,6 L 25,6 L 15,20 Z" fill="#abcdef" stroke="#abcdef" stroke-width="4"/>
  </g>
</svg>"##,
        )
        .unwrap();
        let town = dict(vec![
            ("font_name", Value::String("Lancelot".into())),
            ("font_size", Value::Int(24)),
            ("font_color", Value::String("0.1,0.2,0.3,1".into())),
            ("font_outline_width", Value::Int(3)),
            ("font_outline_color", Value::String("0.9,0.8,0.7,1".into())),
        ]);
        let mut root = dict(vec![(
            "theme",
            dict(vec![("label_presets", dict(vec![("Town", town)]))]),
        )]);

        let summary = import(&mut root, &source, &Resolver::new(&Settings::default())).unwrap();
        assert_eq!(summary.labels, 1);
        assert_eq!(summary.paths, 1);
        assert_eq!(summary.territories, 1);
        let label = &root.get("labels").and_then(Value::as_array).unwrap()[0];
        assert_eq!(label.get("font").and_then(Value::as_str), Some("Lancelot"));
        assert_eq!(label.get("size").and_then(Value::as_f64), Some(31.0));
        let Value::Vector { values, .. } = label.get("color").unwrap() else {
            panic!("label color was not imported");
        };
        assert_eq!(values, &[1.0, 0.0, 128.0 / 255.0, 1.0]);
        let road = &root.get("paths").and_then(Value::as_array).unwrap()[0];
        assert_eq!(
            road.get("style").and_then(Value::as_str),
            Some("res://textures/paths/path_blended")
        );
        let territory = &root
            .get("territories")
            .and_then(|value| value.get("territories"))
            .and_then(Value::as_array)
            .unwrap()[0];
        assert_eq!(
            territory.get("style").and_then(Value::as_str),
            Some("res://textures/borders/border_solid")
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn labels_use_font_mapping_outline_paint_order_and_nonzero_glow() {
        let base =
            std::env::temp_dir().join(format!("wonderdraft-svg-labels-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let color = |values: [f32; 4]| Value::Vector {
            kind: "Color".into(),
            values: values.into(),
        };
        let label = |text: &str, glow_size: f64| {
            dict(vec![
                ("text", Value::String(text.into())),
                ("font", Value::String("IM Fell English Italic".into())),
                (
                    "position",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![10.0, 20.0],
                    },
                ),
                ("size", Value::Int(20)),
                ("color", color([0.0, 0.0, 0.7, 1.0])),
                ("outline_width", Value::Real(0.370417)),
                ("outline_color", color([0.0, 0.5, 0.0, 1.0])),
                ("glow_size", Value::Real(glow_size)),
                (
                    "glow_color",
                    color([225.0 / 255.0, 217.0 / 255.0, 41.0 / 255.0, 0.843137]),
                ),
            ])
        };
        let root = dict(vec![
            ("map_width", Value::Int(100)),
            ("map_height", Value::Int(100)),
            (
                "labels",
                Value::Array(vec![label("Glow", 3.0), label("No glow", 0.0)]),
            ),
        ]);
        let destination = base.join("labels.svg");

        export(
            &root,
            &Vec::new(),
            &destination,
            &Resolver::new(&Settings::default()),
            ExportOptions {
                background: false,
                boxes: false,
                paths: false,
                symbols: false,
                labels: true,
                territories: false,
                embed_background: false,
                embed_boxes: false,
                embed_symbols: false,
            },
        )
        .unwrap();
        let xml = fs::read_to_string(destination).unwrap();
        assert!(xml.contains("font-family=\"IM FELL English\""));
        assert!(xml.contains("font-style=\"italic\""));
        assert!(xml.contains("-inkscape-font-specification:'IM FELL English Italic'"));
        assert!(xml.contains("paint-order=\"markers stroke fill\""));
        assert!(xml.contains("stroke-width=\"0.370417\""));
        assert!(xml.contains("flood-color=\"#e1d929\""));
        assert!(xml.contains("stdDeviation=\"3\""));
        assert_eq!(
            xml.matches("<filter id=\"wonderdraft-label-glow-").count(),
            1
        );
        assert_eq!(
            xml.matches("filter=\"url(#wonderdraft-label-glow-").count(),
            1
        );
        let _ = fs::remove_dir_all(base);
    }
}

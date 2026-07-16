use crate::{
    Error, Result, Value,
    assets::Resolver,
    godot_text,
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
    pub labels: usize,
    pub symbols: usize,
    pub paths: usize,
    pub missing_symbols: usize,
    pub background: String,
    pub warnings: Vec<String>,
}

#[derive(Clone, Copy, Debug)]
pub struct ExportOptions {
    pub background: bool,
    pub paths: bool,
    pub symbols: bool,
    pub labels: bool,
    pub embed_background: bool,
    pub embed_symbols: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            background: true,
            paths: true,
            symbols: true,
            labels: true,
            embed_background: false,
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
    let width = n(record.get("outline_width"), 0.0);
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
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:wd=\"{WD}\" width=\"{width}px\" height=\"{height}px\" viewBox=\"0 0 {width} {height}\" wd:format-version=\"2\" wd:map-width=\"{width}\" wd:map-height=\"{height}\">\n  <metadata>Wonderdraft Map Editor SVG interchange file</metadata>\n"
    );
    if options.background {
        xml.push_str("  <g id=\"wonderdraft-mask-background\">\n");
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
    if options.paths {
        xml.push_str("  <g id=\"wonderdraft-paths\">\n");
        if let Some(a) = root.get("paths").and_then(Value::as_array) {
            for (i, r) in a.iter().enumerate() {
                let Some(p) = points(r) else { continue };
                if p.len() < 2 {
                    continue;
                }
                let pos = vec2(r.get("position"), (0., 0.));
                let (c, op) = color(r.get("color"), [0.2, 0.1, 0.05, 1.]);
                let width = n(r.get("width"), 3.);
                let ps = p
                    .iter()
                    .map(|(x, y)| format!("{:.6},{:.6}", x + pos.0, y + pos.1))
                    .collect::<Vec<_>>()
                    .join(" ");
                xml.push_str(&format!("    <polyline id=\"wonderdraft-path-{i}\" points=\"{ps}\" fill=\"none\" stroke=\"{c}\" stroke-opacity=\"{op}\" stroke-width=\"{width}\" wd:kind=\"path\" wd:record=\"{}\"/>\n",record(r)));
                summary.paths += 1;
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
        xml.push_str("  <g id=\"wonderdraft-symbols\">\n");
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
        xml.push_str("  <g id=\"wonderdraft-labels\">\n");
        if let Some(a) = root.get("labels").and_then(Value::as_array) {
            for (i, r) in a.iter().enumerate() {
                let (x, y) = vec2(r.get("position"), (0., 0.));
                let size = n(r.get("size"), 24.);
                let text = r.get("text").and_then(Value::as_str).unwrap_or("");
                let font = r
                    .get("font")
                    .and_then(Value::as_str)
                    .unwrap_or("sans-serif");
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
                xml.push_str(&format!("    <text id=\"wonderdraft-label-{i}\" x=\"{x}\" y=\"{y}\" font-family=\"{}\" font-size=\"{size}px\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" fill=\"{fill}\" fill-opacity=\"{op}\" wd:kind=\"label\" wd:record=\"{}\"{transform}>{}</text>\n",esc(font),record(r),esc(text)));
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
}
impl Default for El {
    fn default() -> Self {
        Self {
            tag: String::new(),
            attrs: Vec::new(),
            text: String::new(),
            matrix: IDENTITY,
        }
    }
}
fn attr<'a>(e: &'a El, name: &str) -> Option<&'a str> {
    e.attrs
        .iter()
        .find_map(|(k, v)| (k == name || k.ends_with(&format!(":{name}"))).then_some(v.as_str()))
}
fn f(e: &El, name: &str, d: f64) -> f64 {
    attr(e, name)
        .and_then(|v| v.trim_end_matches("px").parse().ok())
        .unwrap_or(d)
}
fn element_from(event: &quick_xml::events::BytesStart<'_>, parent: Matrix) -> El {
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
    element
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
                let el = element_from(&e, parent);
                stack.push(el)
            }
            Ok(Event::Empty(e)) => {
                let parent = stack
                    .last()
                    .map(|element| element.matrix)
                    .unwrap_or(IDENTITY);
                let el = element_from(&e, parent);
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
    let mut summary = Summary::default();
    for e in found {
        match attr(&e, "kind") {
            Some("label") => {
                let mut r = attr(&e, "record")
                    .map(decode_record)
                    .transpose()?
                    .unwrap_or_else(Value::dict);
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
                if let Some(font) = attr(&e, "font-family") {
                    r.set("font", Value::String(font.to_owned()));
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
                let position = vec2(r.get("position"), (0., 0.));
                let ps = attr(&e, "points")
                    .unwrap_or("")
                    .split_whitespace()
                    .filter_map(|p| {
                        let mut n = p.split(',').filter_map(|v| v.parse::<f32>().ok());
                        let point = mat_apply(e.matrix, n.next()? as f64, n.next()? as f64);
                        Some(vec![
                            (point.0 - position.0) as f32,
                            (point.1 - position.1) as f32,
                        ])
                    })
                    .collect::<Vec<_>>();
                let replacement = match r.get("points") {
                    Some(Value::String(_)) => Value::String(godot_text::format(&Value::Array(
                        ps.iter()
                            .map(|p| Value::Vector {
                                kind: "Vector2".into(),
                                values: p.clone(),
                            })
                            .collect(),
                    ))),
                    Some(Value::Array(_)) => Value::Array(
                        ps.iter()
                            .map(|p| Value::Vector {
                                kind: "Vector2".into(),
                                values: p.clone(),
                            })
                            .collect(),
                    ),
                    _ => Value::PoolVectors {
                        kind: "PoolVector2Array".into(),
                        components: 2,
                        values: ps,
                    },
                };
                r.set("points", replacement);
                paths.push(r);
                summary.paths += 1
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
                paths: false,
                symbols: true,
                labels: false,
                embed_background: false,
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
                paths: false,
                symbols: true,
                labels: false,
                embed_background: false,
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
}

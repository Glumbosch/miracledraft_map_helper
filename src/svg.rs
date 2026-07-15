use crate::{
    Error, Result, Value,
    assets::Resolver,
    godot_text,
    images::{self, Images},
};
use quick_xml::{Reader, events::Event};
use std::{fs, path::Path};

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
    embed_background: bool,
) -> Result<Summary> {
    let width = n(root.get("map_width"), 512.);
    let height = n(root.get("map_height"), 512.);
    let mut summary = Summary::default();
    let mut xml = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" xmlns:wd=\"{WD}\" width=\"{width}px\" height=\"{height}px\" viewBox=\"0 0 {width} {height}\" wd:format-version=\"1\" wd:map-width=\"{width}\" wd:map-height=\"{height}\">\n  <metadata>Wonderdraft Map Editor SVG interchange file</metadata>\n"
    );
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
        let href = if embed_background {
            let raw = fs::read(&png_path).map_err(|e| Error::format(e.to_string()))?;
            let _ = fs::remove_file(&png_path);
            format!("data:image/png;base64,{}", b64(&raw, false))
        } else {
            png_path.file_name().unwrap().to_string_lossy().into_owned()
        };
        summary.background = if embed_background {
            "embedded".into()
        } else {
            href.clone()
        };
        xml.push_str(&format!("    <image x=\"0\" y=\"0\" width=\"{width}\" height=\"{height}\" preserveAspectRatio=\"none\" xlink:href=\"{}\" wd:kind=\"background\" wd:image-key=\"mask\"/>\n",esc(&href)));
    }
    xml.push_str("  </g>\n  <g id=\"wonderdraft-paths\">\n");
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
    xml.push_str("  </g>\n  <g id=\"wonderdraft-symbols\">\n");
    if let Some(a) = root.get("symbols").and_then(Value::as_array) {
        for (i, r) in a.iter().enumerate() {
            let texture = r.get("texture").and_then(Value::as_str).unwrap_or("");
            let (x, y) = vec2(r.get("position"), (0., 0.));
            let radius = n(r.get("radius"), 16.);
            let (sample, alpha) = color(r.get("sample"), [1., 1., 1., 1.]);
            if let Some(asset) = resolver.resolve(texture) {
                let dims = image::image_dimensions(&asset).unwrap_or((32, 32));
                let base = (dims.0.max(dims.1) as f64 / 2.).max(1.);
                let scale = vec2(r.get("scale"), (1., 1.));
                let w = dims.0 as f64 * radius / base * scale.0.abs();
                let h = dims.1 as f64 * radius / base * scale.1.abs();
                let href = url::Url::from_file_path(&asset)
                    .map(|u| u.to_string())
                    .unwrap_or_else(|_| asset.to_string_lossy().into_owned());
                xml.push_str(&format!("    <image id=\"wonderdraft-symbol-{i}\" x=\"{}\" y=\"{}\" width=\"{w}\" height=\"{h}\" xlink:href=\"{}\" opacity=\"{alpha}\" wd:kind=\"symbol\" wd:texture=\"{}\" wd:record=\"{}\" wd:export-width=\"{w}\" wd:export-height=\"{h}\"/>\n",x-w/2.,y-h/2.,esc(&href),esc(texture),record(r)));
            } else {
                summary.missing_symbols += 1;
                xml.push_str(&format!("    <circle id=\"wonderdraft-symbol-{i}\" cx=\"{x}\" cy=\"{y}\" r=\"{radius}\" fill=\"{sample}\" fill-opacity=\"{alpha}\" stroke=\"#ff00ff\" wd:kind=\"symbol\" wd:texture=\"{}\" wd:record=\"{}\"/>\n",esc(texture),record(r)));
            }
            summary.symbols += 1;
        }
    }
    xml.push_str("  </g>\n  <g id=\"wonderdraft-labels\">\n");
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
            xml.push_str(&format!("    <text id=\"wonderdraft-label-{i}\" x=\"{x}\" y=\"{y}\" font-family=\"{}\" font-size=\"{size}px\" text-anchor=\"{anchor}\" dominant-baseline=\"central\" fill=\"{fill}\" fill-opacity=\"{op}\" wd:kind=\"label\" wd:record=\"{}\">{}</text>\n",esc(font),record(r),esc(text)));
            summary.labels += 1;
        }
    }
    xml.push_str("  </g>\n</svg>\n");
    fs::write(dest, xml).map_err(|e| Error::format(e.to_string()))?;
    Ok(summary)
}

#[derive(Default)]
struct El {
    tag: String,
    attrs: Vec<(String, String)>,
    text: String,
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
pub fn import(root: &mut Value, source: &Path, resolver: &Resolver) -> Result<Summary> {
    let raw = fs::read_to_string(source).map_err(|e| Error::format(e.to_string()))?;
    let mut reader = Reader::from_str(&raw);
    reader.config_mut().trim_text(true);
    let mut stack: Vec<El> = Vec::new();
    let mut found = Vec::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let mut el = El {
                    tag: String::from_utf8_lossy(e.local_name().as_ref()).into_owned(),
                    ..Default::default()
                };
                for a in e.attributes().flatten() {
                    el.attrs.push((
                        String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                        a.unescape_value().unwrap_or_default().into_owned(),
                    ));
                }
                stack.push(el)
            }
            Ok(Event::Empty(e)) => {
                let mut el = El {
                    tag: String::from_utf8_lossy(e.local_name().as_ref()).into_owned(),
                    ..Default::default()
                };
                for a in e.attributes().flatten() {
                    el.attrs.push((
                        String::from_utf8_lossy(a.key.as_ref()).into_owned(),
                        a.unescape_value().unwrap_or_default().into_owned(),
                    ));
                }
                found.push(el)
            }
            Ok(Event::Text(t)) => {
                if let Some(e) = stack.last_mut() {
                    e.text.push_str(&t.decode().unwrap_or_default())
                }
            }
            Ok(Event::End(_)) => {
                if let Some(e) = stack.pop() {
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
                r.set(
                    "position",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![f(&e, "x", 0.) as f32, f(&e, "y", 0.) as f32],
                    },
                );
                r.set("size", Value::Int(f(&e, "font-size", 24.).round() as i64));
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
                let (x, y) = if e.tag == "circle" {
                    (f(&e, "cx", 0.), f(&e, "cy", 0.))
                } else {
                    let w = f(&e, "width", 32.);
                    let h = f(&e, "height", 32.);
                    (f(&e, "x", 0.) + w / 2., f(&e, "y", 0.) + h / 2.)
                };
                r.set(
                    "position",
                    Value::Vector {
                        kind: "Vector2".into(),
                        values: vec![x as f32, y as f32],
                    },
                );
                if let Some(href) = attr(&e, "href")
                    && let Ok(url) = url::Url::parse(href)
                    && let Ok(p) = url.to_file_path()
                    && let Some(t) = resolver.texture_for_path(&p)
                {
                    r.set("texture", Value::String(t));
                }
                symbols.push(r);
                summary.symbols += 1
            }
            Some("path") => {
                let mut r = attr(&e, "record")
                    .map(decode_record)
                    .transpose()?
                    .unwrap_or_else(Value::dict);
                let ps = attr(&e, "points")
                    .unwrap_or("")
                    .split_whitespace()
                    .filter_map(|p| {
                        let mut n = p.split(',').filter_map(|v| v.parse::<f32>().ok());
                        Some(vec![n.next()?, n.next()?])
                    })
                    .collect::<Vec<_>>();
                r.set(
                    "points",
                    Value::PoolVectors {
                        kind: "PoolVector2Array".into(),
                        components: 2,
                        values: ps,
                    },
                );
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

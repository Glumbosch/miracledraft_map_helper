//! Convert the recoverable vector content of an Inkarnate v3 backup to SVG.

use crate::{Error, Result};
use serde_json::{Map, Value};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    path::Path,
};

pub fn convert(source: &Path, destination: &Path) -> Result<()> {
    let document: Value = serde_json::from_slice(&fs::read(source).map_err(io_error)?)
        .map_err(|error| Error::format(error.to_string()))?;
    let scene = object(&document, "scene")?;
    let size = scene
        .get("normSceneSize")
        .and_then(Value::as_object)
        .ok_or_else(|| Error::format("Inkarnate backup has no scene size"))?;
    let width = number(size, "w", 0.);
    let height = number(size, "h", 0.);
    if width <= 0. || height <= 0. {
        return Err(Error::format("Inkarnate backup has no valid scene size"));
    }
    let mut svg = vec![format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:inkscape="http://www.inkscape.org/namespaces/inkscape" width="{width}" height="{height}" viewBox="0 0 {width} {height}">"#
    )];
    let masks = mask_shapes(&document);
    svg.push("<defs>".into());
    svg.push(format!(r#"<mask id="land-mask" maskUnits="userSpaceOnUse" x="0" y="0" width="{width}" height="{height}"><rect width="{width}" height="{height}" fill="black"/>"#));
    for (mode, path, fill_rule) in masks {
        let fill = if mode == "add" { "white" } else { "black" };
        svg.push(format!(
            r#"<path d="{}" fill="{fill}" fill-rule="{}"/>"#,
            escape(&path),
            escape(&fill_rule)
        ));
    }
    svg.push("</mask></defs>".into());
    if let Some(preview) =
        string(&document, "preview").filter(|value| value.starts_with("data:image/"))
    {
        svg.extend(layer("Preview", &[format!(r#"<image width="{width}" height="{height}" href="{}" preserveAspectRatio="none"/>"#, escape(preview))], true));
    }
    svg.extend(layer(
        "Islands",
        &[format!(
            r##"<rect width="{width}" height="{height}" fill="#d8c99b" mask="url(#land-mask)"/>"##
        )],
        true,
    ));
    let mut paths = Vec::new();
    let mut text = Vec::new();
    let mut grid = Vec::new();
    for entity in entities(&document) {
        if entity.get("isVisible").and_then(Value::as_bool) == Some(false) {
            continue;
        }
        match string(&entity, "entityType") {
            Some("path-v2") if string(&entity, "paths").is_some() => {
                paths.push(render_path(&entity))
            }
            Some("text") => text.push(render_text(&entity)),
            Some("grid") => grid.extend(render_grid(&entity, width, height)),
            _ => {}
        }
    }
    svg.extend(layer("Paths", &paths, true));
    svg.extend(layer("Text", &text, true));
    svg.extend(layer("Grid", &grid, true));
    svg.push("</svg>".into());
    fs::write(destination, svg.join("\n")).map_err(io_error)
}

fn io_error(error: std::io::Error) -> Error {
    Error::format(error.to_string())
}
fn object<'a>(value: &'a Value, key: &str) -> Result<&'a Map<String, Value>> {
    object_value(value, key).ok_or_else(|| Error::format(format!("Inkarnate backup has no {key}")))
}
fn object_value<'a>(value: &'a Value, key: &str) -> Option<&'a Map<String, Value>> {
    value.get(key)?.as_object()
}
fn string<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value.get(key)?.as_str()
}
fn number(value: &Map<String, Value>, key: &str, fallback: f64) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(fallback)
}
fn value_number(value: &Value, key: &str, fallback: f64) -> f64 {
    value.get(key).and_then(Value::as_f64).unwrap_or(fallback)
}
fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn rgba(value: Option<&Value>) -> String {
    let Some(color) = value.and_then(Value::as_object) else {
        return "#000000".into();
    };
    let (r, g, b) = (
        number(color, "r", 0.),
        number(color, "g", 0.),
        number(color, "b", 0.),
    );
    match color.get("a").and_then(Value::as_f64) {
        Some(alpha) if alpha < 1. => format!("rgba({r},{g},{b},{alpha})"),
        _ => format!("rgb({r},{g},{b})"),
    }
}
fn layer(name: &str, body: &[String], visible: bool) -> Vec<String> {
    let hidden = if visible {
        ""
    } else {
        r#" style="display:none""#
    };
    let id = name
        .to_ascii_lowercase()
        .replace(|character: char| !character.is_ascii_alphanumeric(), "-");
    let mut result = vec![format!(
        r#"<g inkscape:groupmode="layer" inkscape:label="{}" id="layer-{id}"{hidden}>"#,
        escape(name)
    )];
    result.extend(body.iter().cloned());
    result.push("</g>".into());
    result
}
fn merge(target: &mut Value, patch: &Value) {
    if let (Some(target), Some(patch)) = (target.as_object_mut(), patch.as_object()) {
        for (key, value) in patch {
            merge(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    } else {
        *target = patch.clone();
    }
}
fn entities(document: &Value) -> Vec<Value> {
    let mut output = BTreeMap::<i64, Value>::new();
    for command in document
        .get("history")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        match string(command, "cmdType") {
            Some("cmd-entity-add") => {
                for item in command
                    .get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    let entity = item.get("entity").unwrap_or(item).clone();
                    if let Some(id) = entity.get("entityId").and_then(Value::as_i64) {
                        output.insert(id, entity);
                    }
                }
            }
            Some("cmd-entity-update") => {
                for item in command
                    .get("items")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(id) = item.get("entityId").and_then(Value::as_i64)
                        && let Some(entity) = output.get_mut(&id)
                    {
                        if let Some(update) = item.get("update") {
                            merge(entity, update);
                        }
                    }
                }
            }
            Some("cmd-entity-remove") => {
                for id in command
                    .get("entityIds")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_i64)
                {
                    output.remove(&id);
                }
            }
            _ => {}
        }
    }
    let mut values = output.into_values().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        value_number(left, "z", 0.).total_cmp(&value_number(right, "z", 0.))
    });
    values
}
fn arc_path(values: &[Value]) -> String {
    let values = values.iter().filter_map(Value::as_f64).collect::<Vec<_>>();
    if values.len() < 2 {
        return String::new();
    }
    let mut current = (values[0], values[1]);
    let mut output = vec![format!("M{},{}", current.0, current.1)];
    for index in (2..values.len().saturating_sub(4)).step_by(5) {
        let p1 = (values[index], values[index + 1]);
        let p2 = (values[index + 2], values[index + 3]);
        let radius = values[index + 4].max(0.);
        let a = (current.0 - p1.0, current.1 - p1.1);
        let b = (p2.0 - p1.0, p2.1 - p1.1);
        let (a_len, b_len) = (a.0.hypot(a.1), b.0.hypot(b.1));
        if radius == 0. || a_len == 0. || b_len == 0. {
            output.push(format!("L{},{}", p1.0, p1.1));
            current = p1;
            continue;
        }
        let (a, b) = ((a.0 / a_len, a.1 / a_len), (b.0 / b_len, b.1 / b_len));
        let angle = (a.0 * b.0 + a.1 * b.1).clamp(-1., 1.).acos();
        if angle < 1e-9 || (std::f64::consts::PI - angle).abs() < 1e-9 {
            output.push(format!("L{},{}", p1.0, p1.1));
            current = p1;
            continue;
        }
        let distance = (radius / (angle / 2.).tan()).min(a_len).min(b_len);
        let start = (p1.0 + a.0 * distance, p1.1 + a.1 * distance);
        let end = (p1.0 + b.0 * distance, p1.1 + b.1 * distance);
        let effective_radius = distance * (angle / 2.).tan();
        let sweep = i32::from(a.0 * b.1 - a.1 * b.0 < 0.);
        output.push(format!(
            "L{},{} A{},{} 0 0 {sweep} {},{}",
            start.0, start.1, effective_radius, effective_radius, end.0, end.1
        ));
        current = end;
    }
    output.push("Z".into());
    output.join(" ")
}
fn polygon_path(values: &[Value]) -> String {
    let numbers = values.iter().filter_map(Value::as_f64).collect::<Vec<_>>();
    if numbers.len() < 2 {
        return String::new();
    };
    let points = numbers
        .chunks_exact(2)
        .map(|point| format!("{},{}", point[0], point[1]))
        .collect::<Vec<_>>()
        .join(" ");
    format!("M{points} Z")
}
fn mask_shapes(document: &Value) -> Vec<(String, String, String)> {
    let mut shapes = Vec::new();
    for command in document
        .get("history")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if string(command, "cmdType") != Some("cmd-mask") {
            continue;
        }
        let Some(shape) = command.get("shape") else {
            continue;
        };
        let rule = string(shape, "fillRule").unwrap_or("nonzero").to_owned();
        let mode = string(command, "mode").unwrap_or("add").to_owned();
        let paths = match string(shape, "type") {
            Some("arc-to-paths-shape") => {
                let mut seen = HashSet::new();
                shape
                    .get("arcToPaths")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter(|path| seen.insert(path.to_string()))
                    .map(|path| arc_path(path.as_array().unwrap_or(&Vec::new())))
                    .collect()
            }
            Some("polygons-shape") => shape
                .get("polygons")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|path| polygon_path(path.as_array().unwrap_or(&Vec::new())))
                .collect(),
            _ => Vec::new(),
        };
        shapes.extend(
            paths
                .into_iter()
                .filter(|path| !path.is_empty())
                .map(|path| (mode.clone(), path, rule.clone())),
        );
    }
    shapes
}
fn render_path(entity: &Value) -> String {
    let fill = if entity
        .get("floorEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        value_number(entity, "floorOpacity", 0.)
    } else {
        0.
    };
    let width = if entity
        .get("wallEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        value_number(entity, "wallThickness", 1.)
    } else {
        0.
    };
    format!(
        r#"<path d="{}" transform="translate({} {}) scale({})" fill="{}" fill-opacity="{fill}" stroke="{}" stroke-width="{width}" opacity="{}"/>"#,
        escape(string(entity, "paths").unwrap_or("")),
        value_number(entity, "x", 0.),
        value_number(entity, "y", 0.),
        value_number(entity, "scale", 1.),
        rgba(entity.get("floorColor")),
        rgba(entity.get("wallColor")),
        value_number(entity, "opacity", 1.)
    )
}
fn render_text(entity: &Value) -> String {
    let style = entity.get("textStyle").and_then(Value::as_object);
    let x = value_number(entity, "x", 0.);
    let y = value_number(entity, "y", 0.);
    let font_size = style
        .map(|style| number(style, "fontSize", 16.))
        .unwrap_or(16.);
    let anchor = match style
        .and_then(|style| style.get("horizontalAlignment"))
        .and_then(Value::as_str)
    {
        Some("left") => "start",
        Some("right") => "end",
        _ => "middle",
    };
    let font = style
        .and_then(|style| style.get("fontFamily"))
        .and_then(Value::as_str)
        .unwrap_or("sans-serif");
    let mut attributes = format!(
        r#"x="{x}" y="{y}" fill="{}" opacity="{}" font-family="{}" font-size="{font_size}" text-anchor="{anchor}""#,
        rgba(style.and_then(|style| style.get("color"))),
        value_number(entity, "opacity", 1.),
        escape(font)
    );
    if style
        .and_then(|style| style.get("isBold"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        attributes.push_str(r#" font-weight="bold""#);
    }
    if style
        .and_then(|style| style.get("isItalic"))
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        attributes.push_str(r#" font-style="italic""#);
    }
    let lines = string(entity, "text")
        .unwrap_or("")
        .lines()
        .collect::<Vec<_>>();
    let body = if lines.is_empty() {
        format!(r#"<tspan x="{x}" dy="0"></tspan>"#)
    } else {
        lines
            .iter()
            .enumerate()
            .map(|(index, line)| {
                format!(
                    r#"<tspan x="{x}" dy="{}">{}</tspan>"#,
                    if index == 0 { 0. } else { font_size },
                    escape(line)
                )
            })
            .collect()
    };
    format!(
        r#"<text {attributes} transform="rotate({} {x} {y})">{body}</text>"#,
        value_number(entity, "angle", 0.)
    )
}
fn render_grid(entity: &Value, width: f64, height: f64) -> Vec<String> {
    let Some(style) = entity.get("style").and_then(Value::as_object) else {
        return Vec::new();
    };
    let step = number(style, "size", 100.).max(1.);
    let common = format!(
        r#"stroke="{}" stroke-opacity="{}" stroke-width="{}""#,
        rgba(style.get("color")),
        number(style, "opacity", 1.),
        number(style, "lineWidth", 1.)
    );
    let mut lines = Vec::new();
    let mut x = 0.;
    while x <= width {
        lines.push(format!(r#"<path d="M{x} 0V{height}" {common}/>"#));
        x += step;
    }
    let mut y = 0.;
    while y <= height {
        lines.push(format!(r#"<path d="M0 {y}H{width}" {common}/>"#));
        y += step;
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_basic_backup_without_external_runtime() {
        let dir = std::env::temp_dir().join(format!("inkarnate-test-{}", std::process::id()));
        let source = dir.with_extension("json");
        let destination = dir.with_extension("svg");
        fs::write(&source, r#"{"scene":{"normSceneSize":{"w":100,"h":80}},"history":[{"cmdType":"cmd-entity-add","items":[{"entity":{"entityId":1,"z":1,"entityType":"text","x":10,"y":20,"text":"Harbor","textStyle":{"fontFamily":"Lancelot","fontSize":30,"color":{"r":10,"g":20,"b":30}}}}]}]}"#).unwrap();
        convert(&source, &destination).unwrap();
        let svg = fs::read_to_string(&destination).unwrap();
        assert!(svg.contains("inkscape:label=\"Text\""));
        assert!(svg.contains("Harbor"));
        assert!(svg.contains("font-family=\"Lancelot\""));
        let _ = fs::remove_file(source);
        let _ = fs::remove_file(destination);
    }

    #[test]
    fn supplied_backup_matches_reference_island_mask_structure() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let source = root.join("testfiles/backup.json");
        let reference = fs::read_to_string(root.join("testfiles/map-vectors.svg")).unwrap();
        let destination =
            std::env::temp_dir().join(format!("inkarnate-islands-{}.svg", std::process::id()));
        convert(&source, &destination).unwrap();
        let actual = fs::read_to_string(&destination).unwrap();
        let mask_paths = |svg: &str| {
            svg.split_once("</mask>")
                .map_or(0, |(mask, _)| mask.matches("<path ").count())
        };
        assert_eq!(mask_paths(&actual), mask_paths(&reference));
        assert!(actual.contains(r#"inkscape:label="Islands""#));
        assert!(actual.contains(r##"mask="url(#land-mask)""##));
        assert!(!actual.contains(r#"id="layer-islands" style="display:none""#));
        let _ = fs::remove_file(destination);
    }
}

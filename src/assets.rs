use crate::settings::Settings;
use std::{
    fs,
    path::{Path, PathBuf},
};
const EXTS: &[&str] = &["png", "webp", "jpg", "jpeg", "svg"];
#[derive(Clone, Debug)]
pub struct Resolver {
    pub custom: Option<PathBuf>,
    pub default: Option<PathBuf>,
}

#[derive(Clone, Debug)]
pub struct AssetInfo {
    pub texture: String,
    pub path: PathBuf,
    pub width: f64,
    pub height: f64,
    pub base_radius: f64,
    pub offset_x: f64,
    pub offset_y: f64,
    pub draw_mode: String,
}
impl Resolver {
    pub fn new(s: &Settings) -> Self {
        Self {
            custom: nonempty(&s.custom_asset_folder),
            default: nonempty(&s.default_asset_folder),
        }
    }
    pub fn resolve(&self, texture: &str) -> Option<PathBuf> {
        let (root, rel) = self
            .custom
            .as_ref()
            .zip(texture.strip_prefix("user://assets/"))
            .or_else(|| {
                self.default
                    .as_ref()
                    .zip(texture.strip_prefix("res://sprites/"))
            })?;
        candidate(root, rel)
    }
    pub fn texture_for_path(&self, path: &Path) -> Option<String> {
        let p = path.canonicalize().ok()?;
        for (root, prefix) in [
            (&self.custom, "user://assets/"),
            (&self.default, "res://sprites/"),
        ] {
            if let Some(root) = root
                && let Ok(rel) = p.strip_prefix(root)
            {
                let mut rel = rel.to_owned();
                rel.set_extension("");
                return Some(format!(
                    "{prefix}{}",
                    rel.to_string_lossy().replace('\\', "/")
                ));
            }
        }
        None
    }

    pub fn asset_info(&self, texture: &str) -> Option<AssetInfo> {
        let path = self.resolve(texture)?;
        let (width, height) = dimensions(&path).unwrap_or((0.0, 0.0));
        let metadata = self.metadata_for(&path).unwrap_or_default();
        let fallback_radius = (width.max(height) / 2.0).max(1.0);
        let finite = |key: &str, default: f64| {
            metadata
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .filter(|value| value.is_finite())
                .unwrap_or(default)
        };
        let base_radius = finite("radius", fallback_radius);
        Some(AssetInfo {
            texture: texture.to_owned(),
            path,
            width: if width > 0.0 {
                width
            } else {
                fallback_radius * 2.0
            },
            height: if height > 0.0 {
                height
            } else {
                fallback_radius * 2.0
            },
            base_radius: if base_radius > 0.0 {
                base_radius
            } else {
                fallback_radius
            },
            offset_x: finite("offset_x", 0.0),
            offset_y: finite("offset_y", 0.0),
            draw_mode: metadata
                .get("draw_mode")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned(),
        })
    }

    fn metadata_for(&self, path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
        let roots = [self.custom.as_deref(), self.default.as_deref()];
        let stem = path.file_stem()?.to_string_lossy().to_lowercase();
        let parent_name = path.parent()?.file_name()?.to_string_lossy().to_lowercase();
        let mut current = path.parent()?;
        loop {
            let metadata_path = current.join(".wonderdraft_symbols");
            if let Ok(text) = fs::read_to_string(metadata_path)
                && let Ok(serde_json::Value::Object(entries)) =
                    serde_json::from_str(text.trim_start_matches('\u{feff}'))
            {
                for (key, value) in entries {
                    if (key.to_lowercase() == stem || key.to_lowercase() == parent_name)
                        && let serde_json::Value::Object(data) = value
                    {
                        return Some(data);
                    }
                }
            }
            if roots.iter().flatten().any(|root| *root == current)
                || current.parent() == Some(current)
            {
                break;
            }
            current = current.parent()?;
        }
        None
    }
}
fn nonempty(s: &str) -> Option<PathBuf> {
    (!s.trim().is_empty()).then(|| PathBuf::from(s))
}
fn candidate(root: &Path, rel: &str) -> Option<PathBuf> {
    let p = root.join(rel);
    if p.is_file() {
        return Some(p);
    }
    for ext in EXTS {
        let q = p.with_extension(ext);
        if q.is_file() {
            return Some(q);
        }
    }
    None
}

fn dimensions(path: &Path) -> Option<(f64, f64)> {
    if path
        .extension()
        .and_then(|v| v.to_str())
        .is_some_and(|v| v.eq_ignore_ascii_case("svg"))
    {
        let text = fs::read_to_string(path).ok()?;
        let mut reader = quick_xml::Reader::from_str(&text);
        loop {
            match reader.read_event().ok()? {
                quick_xml::events::Event::Start(event) | quick_xml::events::Event::Empty(event)
                    if event.local_name().as_ref() == b"svg" =>
                {
                    let mut width = None;
                    let mut height = None;
                    let mut view_box = None;
                    for attribute in event.attributes().flatten() {
                        let value = attribute.unescape_value().ok()?.into_owned();
                        match attribute.key.as_ref() {
                            b"width" => width = parse_length(&value),
                            b"height" => height = parse_length(&value),
                            b"viewBox" => view_box = Some(value),
                            _ => {}
                        }
                    }
                    if let (Some(width), Some(height)) = (width, height) {
                        return Some((width, height));
                    }
                    if let Some(view_box) = view_box {
                        let values: Vec<f64> = view_box
                            .split(|c: char| c.is_whitespace() || c == ',')
                            .filter(|v| !v.is_empty())
                            .filter_map(|v| v.parse().ok())
                            .collect();
                        if values.len() == 4 {
                            return Some((values[2].abs(), values[3].abs()));
                        }
                    }
                    return None;
                }
                quick_xml::events::Event::Eof => return None,
                _ => {}
            }
        }
    }
    image::image_dimensions(path)
        .ok()
        .map(|(w, h)| (w as f64, h as f64))
}

fn parse_length(value: &str) -> Option<f64> {
    value
        .trim()
        .trim_end_matches(|c: char| c.is_ascii_alphabetic() || c == '%')
        .parse()
        .ok()
}

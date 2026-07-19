use crate::{
    Error, Result,
    settings::{self, Settings},
};
use std::{
    fs,
    path::{Path, PathBuf},
};
const EXTS: &[&str] = &["png", "webp", "jpg", "jpeg", "svg"];
const SYMBOL_DATABASE_FORMAT_VERSION: u32 = 2;
#[derive(Clone, Debug)]
pub struct Resolver {
    pub custom: Option<PathBuf>,
    pub default: Option<PathBuf>,
    pub packs: Option<PathBuf>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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
        let default = nonempty(&s.default_asset_folder)
            .filter(|path| path.is_dir())
            .or_else(bundled_core_sprites);
        let packs = default
            .as_deref()
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.eq_ignore_ascii_case("sprites"))
            })
            .and_then(Path::parent)
            .map(|root| root.join("packs"));
        Self {
            custom: nonempty(&s.custom_asset_folder),
            default,
            packs,
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
            })
            .or_else(|| {
                self.packs
                    .as_ref()
                    .zip(texture.strip_prefix("res://packs/"))
            })?;
        candidate(root, rel)
    }
    pub fn texture_for_path(&self, path: &Path) -> Option<String> {
        let p = path.canonicalize().ok()?;
        for (root, prefix) in [
            (&self.custom, "user://assets/"),
            (&self.default, "res://sprites/"),
            (&self.packs, "res://packs/"),
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

    /// Enumerate every resolvable symbol image in the configured core, pack,
    /// and custom asset trees. The URI is the value Wonderdraft stores.
    pub fn all_assets(&self) -> Vec<AssetInfo> {
        let mut assets = Vec::new();
        for (root, prefix) in [
            (self.default.as_deref(), "res://sprites/"),
            (self.packs.as_deref(), "res://packs/"),
            (self.custom.as_deref(), "user://assets/"),
        ] {
            let Some(root) = root else { continue };
            collect_assets(self, root, root, prefix, &mut assets);
        }
        assets.sort_by(|left, right| {
            left.texture
                .to_lowercase()
                .cmp(&right.texture.to_lowercase())
        });
        assets.dedup_by(|left, right| left.texture == right.texture);
        assets
    }

    /// Load the persisted symbol index, rebuilding it on first use. Entries
    /// whose source files were removed are ignored.
    pub fn symbol_database(&self) -> Result<Vec<AssetInfo>> {
        let path = symbol_database_path();
        if let Ok(text) = fs::read_to_string(&path)
            && let Ok(database) = serde_json::from_str::<SymbolDatabase>(&text)
            && database.format_version == SYMBOL_DATABASE_FORMAT_VERSION
        {
            let assets = database
                .assets
                .into_iter()
                .filter(|asset| asset.path.is_file())
                .collect::<Vec<_>>();
            if !assets.is_empty() {
                return Ok(assets);
            }
        }
        self.rebuild_symbol_database()
    }

    /// Scan configured core, pack, and custom asset trees and persist the
    /// metadata that drives the symbol gallery and renderer controls.
    pub fn rebuild_symbol_database(&self) -> Result<Vec<AssetInfo>> {
        let assets = self.all_assets();
        let database = SymbolDatabase {
            format_version: SYMBOL_DATABASE_FORMAT_VERSION,
            assets: assets.clone(),
        };
        let path = symbol_database_path();
        let json = serde_json::to_vec_pretty(&database)
            .map_err(|error| Error::format(error.to_string()))?;
        fs::write(&path, json).map_err(|error| Error::format(error.to_string()))?;
        Ok(assets)
    }

    fn metadata_for(&self, path: &Path) -> Option<serde_json::Map<String, serde_json::Value>> {
        let roots = [
            self.custom.as_deref(),
            self.default.as_deref(),
            self.packs.as_deref(),
        ];
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

#[derive(serde::Serialize, serde::Deserialize)]
struct SymbolDatabase {
    format_version: u32,
    assets: Vec<AssetInfo>,
}

pub fn symbol_database_path() -> PathBuf {
    settings::config_path().with_file_name("miracledraft_symbol_database.json")
}

/// The core sprites extracted alongside this executable's source checkout.
/// This is a safe fallback when an older saved setting points at a moved or
/// deleted extraction directory.
pub fn bundled_core_sprites() -> Option<PathBuf> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("wonderdraft_files/sprites");
    path.is_dir().then_some(path)
}

fn collect_assets(
    resolver: &Resolver,
    root: &Path,
    directory: &Path,
    prefix: &str,
    out: &mut Vec<AssetInfo>,
) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_assets(resolver, root, &path, prefix, out);
            continue;
        }
        let supported = path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| EXTS.iter().any(|ext| value.eq_ignore_ascii_case(ext)));
        if !supported {
            continue;
        }
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let mut relative = relative.to_owned();
        relative.set_extension("");
        let texture = format!("{prefix}{}", relative.to_string_lossy().replace('\\', "/"));
        if let Some(info) = resolver.asset_info(&texture) {
            out.push(info);
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        env,
        time::{SystemTime, UNIX_EPOCH},
    };

    #[test]
    fn resolves_extracted_pack_textures_and_recreates_their_uri() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let extracted = env::temp_dir().join(format!("wonderdraft-pack-resolver-{stamp}"));
        let sprites = extracted.join("sprites");
        let pack_sprite =
            extracted.join("packs/Tang Dynasty by Chan/sprites/symbols/tang_dynasty_normal/5.png");
        fs::create_dir_all(&sprites).unwrap();
        fs::create_dir_all(pack_sprite.parent().unwrap()).unwrap();
        fs::write(&pack_sprite, b"test image placeholder").unwrap();

        let resolver = Resolver::new(&Settings {
            default_asset_folder: sprites.to_string_lossy().into_owned(),
            ..Settings::default()
        });
        let texture = "res://packs/Tang Dynasty by Chan/sprites/symbols/tang_dynasty_normal/5";

        assert_eq!(resolver.resolve(texture), Some(pack_sprite.clone()));
        assert_eq!(
            resolver.texture_for_path(&pack_sprite),
            Some(texture.to_owned())
        );
        let _ = fs::remove_dir_all(extracted);
    }

    #[test]
    fn lists_configured_core_and_custom_symbols() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = env::temp_dir().join(format!("wonderdraft-symbol-gallery-{stamp}"));
        let core = root.join("sprites/symbols/core/castle.png");
        let custom = root.join("assets/places/tower.webp");
        fs::create_dir_all(core.parent().unwrap()).unwrap();
        fs::create_dir_all(custom.parent().unwrap()).unwrap();
        fs::write(&core, b"placeholder").unwrap();
        fs::write(&custom, b"placeholder").unwrap();

        let resolver = Resolver::new(&Settings {
            default_asset_folder: root.join("sprites").to_string_lossy().into_owned(),
            custom_asset_folder: root.join("assets").to_string_lossy().into_owned(),
            ..Settings::default()
        });
        let textures = resolver
            .all_assets()
            .into_iter()
            .map(|asset| asset.texture)
            .collect::<Vec<_>>();

        assert!(textures.contains(&"res://sprites/symbols/core/castle".to_owned()));
        assert!(textures.contains(&"user://assets/places/tower".to_owned()));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn symbol_database_serializes_source_pixel_dimensions() {
        let root = env::temp_dir().join(format!(
            "wonderdraft-symbol-dimensions-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let image_path = root.join("wide.png");
        image::RgbaImage::new(37, 19).save(&image_path).unwrap();
        let resolver = Resolver {
            custom: Some(root.clone()),
            default: None,
            packs: None,
        };
        let asset = resolver.asset_info("user://assets/wide").unwrap();
        let json = serde_json::to_value(SymbolDatabase {
            format_version: SYMBOL_DATABASE_FORMAT_VERSION,
            assets: vec![asset],
        })
        .unwrap();

        assert_eq!(json["assets"][0]["width"], 37.0);
        assert_eq!(json["assets"][0]["height"], 19.0);
        let _ = fs::remove_dir_all(root);
    }
}

use crate::settings::Settings;
use std::path::{Path, PathBuf};
const EXTS: &[&str] = &["png", "webp", "jpg", "jpeg", "svg"];
#[derive(Clone, Debug)]
pub struct Resolver {
    pub custom: Option<PathBuf>,
    pub default: Option<PathBuf>,
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

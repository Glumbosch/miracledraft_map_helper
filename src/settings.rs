use crate::{Result, error::IoContext};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub custom_asset_folder: String,
    #[serde(default)]
    pub default_asset_folder: String,
    #[serde(default)]
    pub cache_folder: String,
}
pub fn config_path() -> PathBuf {
    let working = env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("wonderdraft_gui.config");
    if working.is_file() {
        return working;
    }
    env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_owned))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("wonderdraft_gui.config")
}
pub fn default_cache() -> PathBuf {
    if let Ok(p) = env::var("WONDERDRAFT_GUI_CACHE") {
        return PathBuf::from(p);
    }
    if cfg!(target_os = "windows") {
        PathBuf::from(env::var_os("LOCALAPPDATA").unwrap_or_else(|| ".".into()))
            .join("WonderdraftMapEditor/cache")
    } else if cfg!(target_os = "macos") {
        PathBuf::from(env::var_os("HOME").unwrap_or_else(|| ".".into()))
            .join("Library/Caches/WonderdraftMapEditor")
    } else {
        PathBuf::from(env::var_os("XDG_CACHE_HOME").unwrap_or_else(|| {
            env::var_os("HOME")
                .map(|p| PathBuf::from(p).join(".cache").into_os_string())
                .unwrap_or_else(|| "/tmp".into())
        }))
        .join("wonderdraft_gui")
    }
}
pub fn load() -> Settings {
    let mut s: Settings = fs::read_to_string(config_path())
        .ok()
        .and_then(|v| serde_json::from_str(&v).ok())
        .unwrap_or_default();
    if s.cache_folder.is_empty() {
        s.cache_folder = default_cache().to_string_lossy().into_owned();
    }
    s
}
pub fn save(s: &Settings) -> Result<()> {
    let p = config_path();
    fs::write(&p, serde_json::to_string_pretty(s)? + "\n").at(p)
}

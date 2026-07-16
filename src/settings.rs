use crate::{Result, error::IoContext};
use serde::{Deserialize, Serialize};
use std::{
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub custom_asset_folder: String,
    #[serde(default)]
    pub default_asset_folder: String,
    #[serde(default)]
    pub cache_folder: String,
    #[serde(default)]
    pub wonderdraft_folder: String,
    #[serde(default = "enabled")]
    pub auto_locate_custom_assets: bool,
    #[serde(default = "enabled")]
    pub clear_cache_on_exit: bool,
    #[serde(default)]
    pub setup_completed: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            custom_asset_folder: String::new(),
            default_asset_folder: String::new(),
            cache_folder: String::new(),
            wonderdraft_folder: String::new(),
            auto_locate_custom_assets: true,
            clear_cache_on_exit: true,
            setup_completed: false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WonderdraftConfig {
    pub recently_opened: Vec<PathBuf>,
    pub last_directory: Option<PathBuf>,
    pub custom_assets_directory: Option<PathBuf>,
}

fn enabled() -> bool {
    true
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

pub fn default_wonderdraft_folder() -> PathBuf {
    let home = PathBuf::from(env::var_os("HOME").unwrap_or_else(|| ".".into()));
    if cfg!(target_os = "windows") {
        PathBuf::from(
            env::var_os("APPDATA").unwrap_or_else(|| home.join("AppData/Roaming").into_os_string()),
        )
        .join("Wonderdraft")
    } else if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Wonderdraft")
    } else {
        home.join(".local/share/Wonderdraft")
    }
}

pub fn find_wonderdraft_folder(configured: &str) -> Option<PathBuf> {
    let folder = if configured.trim().is_empty() {
        default_wonderdraft_folder()
    } else {
        PathBuf::from(configured.trim())
    };
    folder.join("config.ini").is_file().then_some(folder)
}

pub fn read_wonderdraft_config(folder: &Path) -> Result<WonderdraftConfig> {
    let path = folder.join("config.ini");
    let text = fs::read_to_string(&path).at(&path)?;
    Ok(parse_wonderdraft_config(&text))
}

pub fn custom_assets_folder(folder: &Path, config: &WonderdraftConfig) -> PathBuf {
    let root = match config.custom_assets_directory.as_deref() {
        Some(path) if path.is_absolute() => path.to_owned(),
        Some(path) => folder.join(path),
        None => folder.to_owned(),
    };
    if root.file_name().is_some_and(|name| name == "assets") {
        root
    } else {
        root.join("assets")
    }
}

fn parse_wonderdraft_config(text: &str) -> WonderdraftConfig {
    let mut parsed = WonderdraftConfig::default();
    let mut section = String::new();
    for raw_line in text.lines() {
        let line = raw_line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_owned();
            continue;
        }
        let Some((raw_key, raw_value)) = line.split_once('=') else {
            continue;
        };
        let key = raw_key.trim();
        let value = raw_value.trim();
        if key == "custom_assets_directory" {
            parsed.custom_assets_directory = parse_quoted_value(value).map(PathBuf::from);
        } else if section == "Save" && key == "last_directory" {
            parsed.last_directory = parse_quoted_value(value).map(PathBuf::from);
        } else if section == "Save" && key == "recently_opened" {
            parsed.recently_opened = parse_quoted_values(value)
                .into_iter()
                .map(PathBuf::from)
                .collect();
        }
    }
    parsed
}

fn parse_quoted_value(value: &str) -> Option<String> {
    parse_quoted_values(value).into_iter().next()
}

fn parse_quoted_values(value: &str) -> Vec<String> {
    let mut values = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars().peekable();
    let mut quoted = false;
    while let Some(ch) = chars.next() {
        if !quoted {
            if ch == '"' {
                quoted = true;
                current.clear();
            }
            continue;
        }
        match ch {
            '"' => {
                quoted = false;
                values.push(std::mem::take(&mut current));
            }
            '\\' if matches!(chars.peek(), Some('"' | '\\')) => {
                current.push(chars.next().unwrap());
            }
            _ => current.push(ch),
        }
    }
    values
}

pub fn directory_size(path: &Path) -> std::io::Result<u64> {
    let mut total = 0u64;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            total = total.saturating_add(directory_size(&entry.path())?);
        } else if metadata.is_file() {
            total = total.saturating_add(metadata.len());
        }
    }
    Ok(total)
}

pub fn clear_cache(path: &Path, preserve: Option<&Path>) -> std::io::Result<()> {
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let entry = entry?;
        let entry_path = entry.path();
        let app_cache_entry = entry.file_name().to_str().is_some_and(|name| {
            name.starts_with("wonderdraft_rust_")
                || name.starts_with("wonderdraft_gui_")
                || name.starts_with("wonderdraft_verify_")
        });
        if !app_cache_entry {
            continue;
        }
        if preserve.is_some_and(|preserve| preserve == entry_path) {
            continue;
        }
        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            fs::remove_dir_all(entry_path)?;
        } else {
            fs::remove_file(entry_path)?;
        }
    }
    Ok(())
}

pub fn load() -> Settings {
    let mut settings: Settings = fs::read_to_string(config_path())
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
        .unwrap_or_default();
    if settings.cache_folder.is_empty() {
        settings.cache_folder = default_cache().to_string_lossy().into_owned();
    }
    settings
}

pub fn save(settings: &Settings) -> Result<()> {
    let path = config_path();
    fs::write(&path, serde_json::to_string_pretty(settings)? + "\n").at(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn old_settings_files_keep_safe_cache_defaults() {
        let settings: Settings =
            serde_json::from_str(r#"{"custom_asset_folder":"/assets","cache_folder":"/cache"}"#)
                .unwrap();
        assert!(settings.auto_locate_custom_assets);
        assert!(settings.clear_cache_on_exit);
        assert!(!settings.setup_completed);
    }

    #[test]
    fn parses_wonderdraft_save_and_asset_settings() {
        let parsed = parse_wonderdraft_config(
            r#"
[Assets]
custom_assets_directory="/home/test/Wonderdraft2"

[Save]
recently_opened=[ "/maps/one.wonderdraft_map", "/maps/two.wonderdraft_map" ]
last_directory="/maps"
"#,
        );
        assert_eq!(
            parsed.recently_opened,
            vec![
                PathBuf::from("/maps/one.wonderdraft_map"),
                PathBuf::from("/maps/two.wonderdraft_map")
            ]
        );
        assert_eq!(parsed.last_directory, Some(PathBuf::from("/maps")));
        assert_eq!(
            parsed.custom_assets_directory,
            Some(PathBuf::from("/home/test/Wonderdraft2"))
        );
        assert_eq!(
            custom_assets_folder(Path::new("/home/test/Wonderdraft"), &parsed),
            PathBuf::from("/home/test/Wonderdraft2/assets")
        );
    }

    #[test]
    fn cache_size_and_clear_preserve_the_active_map() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = env::temp_dir().join(format!("wonderdraft-settings-test-{stamp}"));
        let active = base.join("wonderdraft_rust_active");
        let stale = base.join("wonderdraft_rust_stale");
        let unrelated = base.join("unrelated");
        fs::create_dir_all(&active).unwrap();
        fs::create_dir_all(&stale).unwrap();
        fs::create_dir_all(&unrelated).unwrap();
        fs::write(active.join("map.variant"), [0; 7]).unwrap();
        fs::write(stale.join("map.variant"), [0; 11]).unwrap();
        fs::write(unrelated.join("keep.txt"), [0; 13]).unwrap();
        assert_eq!(directory_size(&base).unwrap(), 31);
        clear_cache(&base, Some(&active)).unwrap();
        assert!(active.is_dir());
        assert!(!stale.exists());
        assert!(unrelated.is_dir());
        assert_eq!(directory_size(&base).unwrap(), 20);
        let _ = fs::remove_dir_all(base);
    }
}

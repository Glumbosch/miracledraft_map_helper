use crate::{Error, Result, error::IoContext};
use std::{
    collections::HashMap,
    env, fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Installation {
    pub discovered: usize,
    pub installed: usize,
    pub already_installed: usize,
    pub conflicts: usize,
    pub destination: PathBuf,
    pub warnings: Vec<String>,
    font_files: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Core,
    Custom,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    pub path: PathBuf,
    pub origin: Origin,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FontName {
    pub family: String,
    pub style: String,
    pub weight: String,
}

pub fn source_dir() -> PathBuf {
    crate::pck::default_output_dir().join("fonts")
}

pub fn name_mapping_path() -> PathBuf {
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("wonderdraft_font_names.txt")
}

pub fn load_name_mapping() -> Result<HashMap<String, FontName>> {
    let path = name_mapping_path();
    if !path.is_file() {
        return Ok(HashMap::new());
    }
    let text = fs::read_to_string(&path).at(&path)?;
    let mut mapping = HashMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut columns = line.split('\t');
        let Some(label) = columns
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let Some(family) = columns
            .next()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        let style = columns.next().map(str::trim).unwrap_or("normal");
        let weight = columns.next().map(str::trim).unwrap_or("normal");
        mapping.insert(
            label.to_owned(),
            FontName {
                family: family.to_owned(),
                style: if style.is_empty() { "normal" } else { style }.to_owned(),
                weight: if weight.is_empty() { "normal" } else { weight }.to_owned(),
            },
        );
    }
    Ok(mapping)
}

pub fn mapped_name<'a>(
    mapping: &'a HashMap<String, FontName>,
    wonderdraft_label: &str,
) -> Option<&'a FontName> {
    let label = Path::new(wonderdraft_label)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or(wonderdraft_label);
    mapping.get(label).or_else(|| {
        mapping
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(label).then_some(value))
    })
}

pub fn wonderdraft_label_for_name(
    mapping: &HashMap<String, FontName>,
    family: &str,
    style: &str,
    weight: &str,
) -> Option<String> {
    mapping.iter().find_map(|(label, name)| {
        (name.family.eq_ignore_ascii_case(family)
            && name.style.eq_ignore_ascii_case(style)
            && name.weight.eq_ignore_ascii_case(weight))
        .then(|| label.clone())
    })
}

pub fn update_name_mapping(candidates: &[Candidate]) -> Result<usize> {
    let path = name_mapping_path();
    let existing = load_name_mapping()?;
    let mut additions = Vec::new();
    for candidate in candidates {
        let Some(label) = candidate
            .path
            .file_stem()
            .and_then(|value| value.to_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
        else {
            continue;
        };
        if existing.keys().any(|key| key.eq_ignore_ascii_case(label))
            || additions
                .iter()
                .any(|(known, _): &(String, FontName)| known.eq_ignore_ascii_case(label))
        {
            continue;
        }
        if let Some(name) = font_name(&candidate.path)? {
            additions.push((label.to_owned(), name));
        }
    }
    if additions.is_empty() && path.is_file() {
        return Ok(0);
    }
    let new_file = !path.is_file();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .at(&path)?;
    if new_file {
        writeln!(
            file,
            "# Wonderdraft font label<TAB>installed font family<TAB>SVG style<TAB>SVG weight"
        )
        .at(&path)?;
        writeln!(
            file,
            "# This file is editable. Existing rows are never replaced during font discovery."
        )
        .at(&path)?;
    }
    additions.sort_by(|left, right| left.0.cmp(&right.0));
    for (label, name) in &additions {
        writeln!(
            file,
            "{label}\t{}\t{}\t{}",
            name.family, name.style, name.weight
        )
        .at(&path)?;
    }
    Ok(additions.len())
}

fn be_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_be_bytes(
        data.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn be_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn decode_font_string(platform: u16, bytes: &[u8]) -> Option<String> {
    let decoded = if platform == 0 || platform == 3 {
        let words = bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&words).ok()?
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let decoded = decoded.trim_matches('\0').trim().to_owned();
    (!decoded.is_empty()).then_some(decoded)
}

fn font_name(path: &Path) -> Result<Option<FontName>> {
    let data = fs::read(path).at(path)?;
    let face_offset = if data.get(0..4) == Some(b"ttcf") {
        be_u32(&data, 12).map(|value| value as usize).unwrap_or(0)
    } else {
        0
    };
    let Some(table_count) = be_u16(&data, face_offset + 4).map(usize::from) else {
        return Ok(None);
    };
    let mut name_table = None;
    for index in 0..table_count {
        let record = face_offset + 12 + index * 16;
        if data.get(record..record + 4) == Some(b"name") {
            name_table = be_u32(&data, record + 8).map(|value| value as usize);
            break;
        }
    }
    let Some(table) = name_table else {
        return Ok(None);
    };
    let Some(count) = be_u16(&data, table + 2).map(usize::from) else {
        return Ok(None);
    };
    let Some(strings) = be_u16(&data, table + 4).map(|value| table + value as usize) else {
        return Ok(None);
    };
    let mut names: HashMap<u16, (i32, String)> = HashMap::new();
    for index in 0..count {
        let record = table + 6 + index * 12;
        let (Some(platform), Some(language), Some(name_id), Some(length), Some(offset)) = (
            be_u16(&data, record),
            be_u16(&data, record + 4),
            be_u16(&data, record + 6),
            be_u16(&data, record + 8).map(usize::from),
            be_u16(&data, record + 10).map(usize::from),
        ) else {
            continue;
        };
        if !matches!(name_id, 1 | 2 | 16 | 17) {
            continue;
        }
        let Some(value) = data
            .get(strings + offset..strings + offset + length)
            .and_then(|bytes| decode_font_string(platform, bytes))
        else {
            continue;
        };
        let score = match (platform, language) {
            (3, 0x0409) => 4,
            (0, _) => 3,
            (3, _) => 2,
            _ => 1,
        };
        if names
            .get(&name_id)
            .is_none_or(|(old_score, _)| score > *old_score)
        {
            names.insert(name_id, (score, value));
        }
    }
    let family = names
        .remove(&16)
        .or_else(|| names.remove(&1))
        .map(|(_, value)| value);
    let Some(family) = family else {
        return Ok(None);
    };
    let subfamily = names
        .remove(&17)
        .or_else(|| names.remove(&2))
        .map(|(_, value)| value)
        .unwrap_or_else(|| "Regular".into());
    let lowercase = subfamily.to_ascii_lowercase();
    let style = if lowercase.contains("italic") {
        "italic"
    } else if lowercase.contains("oblique") {
        "oblique"
    } else {
        "normal"
    };
    let weight = if lowercase.contains("bold") {
        "bold"
    } else {
        "normal"
    };
    Ok(Some(FontName {
        family,
        style: style.into(),
        weight: weight.into(),
    }))
}

pub fn user_fonts_dir() -> Result<PathBuf> {
    if cfg!(target_os = "windows") {
        let root = env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .ok_or_else(|| {
                Error::format("LOCALAPPDATA is not set; cannot locate the user Fonts folder")
            })?;
        Ok(root.join("Microsoft/Windows/Fonts"))
    } else if cfg!(target_os = "macos") {
        let home = env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| Error::format("HOME is not set; cannot locate the user Fonts folder"))?;
        Ok(home.join("Library/Fonts"))
    } else {
        Ok(env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
            .ok_or_else(|| {
                Error::format(
                    "Neither XDG_DATA_HOME nor HOME is set; cannot locate the user Fonts folder",
                )
            })?
            .join("fonts"))
    }
}

pub fn install_for_current_user(source: &Path) -> Result<Installation> {
    if !source.is_dir() {
        return Err(Error::format(format!(
            "The extracted Wonderdraft fonts folder does not exist: {}. Extract Wonderdraft.pck first.",
            source.display()
        )));
    }
    let mut paths = Vec::new();
    collect_fonts(source, &mut paths)?;
    install_selected(&paths)
}

pub fn discover(core: Option<&Path>, custom_assets: Option<&Path>) -> Result<Vec<Candidate>> {
    let mut candidates = Vec::new();
    if let Some(core) = core.filter(|directory| directory.is_dir()) {
        let mut paths = Vec::new();
        collect_fonts(core, &mut paths)?;
        candidates.extend(paths.into_iter().map(|path| Candidate {
            path,
            origin: Origin::Core,
        }));
    }
    if let Some(custom_assets) = custom_assets.filter(|directory| directory.is_dir()) {
        let mut paths = Vec::new();
        collect_custom_fonts(custom_assets, false, &mut paths)?;
        candidates.extend(paths.into_iter().map(|path| Candidate {
            path,
            origin: Origin::Custom,
        }));
    }
    candidates.sort_by(|left, right| left.path.cmp(&right.path));
    candidates.dedup_by(|left, right| left.path == right.path);
    Ok(candidates)
}

pub fn install_selected(fonts: &[PathBuf]) -> Result<Installation> {
    if fonts.is_empty() {
        return Err(Error::format("No fonts were selected for installation"));
    }
    let destination = user_fonts_dir()?;
    let mut installation = install_files_into(fonts, &destination)?;
    refresh_font_registry(&mut installation);
    Ok(installation)
}

fn is_font(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "ttf" | "otf" | "ttc" | "otc"
            )
        })
}

fn collect_fonts(directory: &Path, fonts: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(directory).at(directory)? {
        let entry = entry.at(directory)?;
        let path = entry.path();
        let file_type = entry.file_type().at(&path)?;
        if file_type.is_dir() {
            collect_fonts(&path, fonts)?;
        } else if file_type.is_file() && is_font(&path) {
            fonts.push(path);
        }
    }
    Ok(())
}

fn collect_custom_fonts(
    directory: &Path,
    inside_fonts_folder: bool,
    fonts: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(directory).at(directory)? {
        let entry = entry.at(directory)?;
        let path = entry.path();
        let file_type = entry.file_type().at(&path)?;
        if file_type.is_dir() {
            let is_fonts_folder = path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("fonts"));
            collect_custom_fonts(&path, inside_fonts_folder || is_fonts_folder, fonts)?;
        } else if inside_fonts_folder && file_type.is_file() && is_font(&path) {
            fonts.push(path);
        }
    }
    Ok(())
}

fn install_files_into(fonts: &[PathBuf], destination: &Path) -> Result<Installation> {
    fs::create_dir_all(destination).at(destination)?;
    let mut fonts = fonts.to_vec();
    fonts.sort();
    fonts.dedup();

    let mut installation = Installation {
        discovered: fonts.len(),
        destination: destination.to_owned(),
        ..Installation::default()
    };
    for font in fonts {
        if !font.is_file() || !is_font(&font) {
            installation.warnings.push(format!(
                "Skipped a missing or unsupported font: {}",
                font.display()
            ));
            continue;
        }
        let Some(file_name) = font.file_name() else {
            continue;
        };
        let target = destination.join(file_name);
        if target.exists() {
            if fs::read(&font).at(&font)? == fs::read(&target).at(&target)? {
                installation.already_installed += 1;
                installation.font_files.push(target);
            } else {
                installation.conflicts += 1;
                installation.warnings.push(format!(
                    "Kept the existing different font: {}",
                    target.display()
                ));
            }
            continue;
        }
        fs::copy(&font, &target).at(&target)?;
        installation.installed += 1;
        installation.font_files.push(target);
    }
    Ok(installation)
}

#[cfg(target_os = "linux")]
fn refresh_font_registry(installation: &mut Installation) {
    match std::process::Command::new("fc-cache")
        .arg("-f")
        .arg(&installation.destination)
        .status()
    {
        Ok(status) if status.success() => {}
        Ok(status) => installation.warnings.push(format!(
            "Fonts were copied, but fc-cache exited with status {status}"
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => installation
            .warnings
            .push("Fonts were copied; fc-cache is unavailable, so applications may see them after the next login".into()),
        Err(error) => installation
            .warnings
            .push(format!("Fonts were copied, but fc-cache could not run: {error}")),
    }
}

#[cfg(target_os = "windows")]
fn refresh_font_registry(installation: &mut Installation) {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt};

    #[link(name = "gdi32")]
    unsafe extern "system" {
        fn AddFontResourceExW(name: *const u16, flags: u32, reserved: *mut c_void) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn SendMessageTimeoutW(
            window: usize,
            message: u32,
            word: usize,
            long: isize,
            flags: u32,
            timeout: u32,
            result: *mut usize,
        ) -> isize;
    }

    let mut any_loaded = false;
    for path in &installation.font_files {
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let kind = match path
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str()
        {
            "otf" | "otc" => "OpenType",
            _ => "TrueType",
        };
        let value_name = format!("{file_name} ({kind})");
        let status = std::process::Command::new("reg.exe")
            .args([
                "add",
                r"HKCU\Software\Microsoft\Windows NT\CurrentVersion\Fonts",
                "/v",
                &value_name,
                "/t",
                "REG_SZ",
                "/d",
            ])
            .arg(&path)
            .arg("/f")
            .status();
        if !matches!(status, Ok(status) if status.success()) {
            installation.warnings.push(format!(
                "Could not register {file_name} with Windows; it may become available after the next login"
            ));
        }
        let wide_path = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        // The path is NUL-terminated and remains alive for the duration of the call.
        if unsafe { AddFontResourceExW(wide_path.as_ptr(), 0, std::ptr::null_mut()) } > 0 {
            any_loaded = true;
        } else {
            installation.warnings.push(format!(
                "Windows did not activate {file_name} immediately; it should be available after the next login"
            ));
        }
    }
    if any_loaded {
        let mut result = 0;
        // Notify running desktop applications that the session font table changed.
        unsafe {
            SendMessageTimeoutW(0xffff, 0x001d, 0, 0, 0x0002, 1_000, &mut result);
        }
    }
}

#[cfg(target_os = "macos")]
fn refresh_font_registry(_: &mut Installation) {
    // Font Services automatically observes additions to ~/Library/Fonts.
}

#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
fn refresh_font_registry(installation: &mut Installation) {
    installation
        .warnings
        .push("Fonts were copied, but this platform may require a login before they appear".into());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_font(family: &str, subfamily: &str) -> Vec<u8> {
        let encode = |value: &str| {
            value
                .encode_utf16()
                .flat_map(u16::to_be_bytes)
                .collect::<Vec<_>>()
        };
        let family = encode(family);
        let subfamily = encode(subfamily);
        let mut name = Vec::new();
        name.extend_from_slice(&0u16.to_be_bytes());
        name.extend_from_slice(&2u16.to_be_bytes());
        name.extend_from_slice(&30u16.to_be_bytes());
        for (name_id, length, offset) in [
            (1u16, family.len() as u16, 0u16),
            (2u16, subfamily.len() as u16, family.len() as u16),
        ] {
            name.extend_from_slice(&3u16.to_be_bytes());
            name.extend_from_slice(&1u16.to_be_bytes());
            name.extend_from_slice(&0x0409u16.to_be_bytes());
            name.extend_from_slice(&name_id.to_be_bytes());
            name.extend_from_slice(&length.to_be_bytes());
            name.extend_from_slice(&offset.to_be_bytes());
        }
        name.extend_from_slice(&family);
        name.extend_from_slice(&subfamily);

        let mut font = vec![0; 28];
        font[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
        font[4..6].copy_from_slice(&1u16.to_be_bytes());
        font[12..16].copy_from_slice(b"name");
        font[20..24].copy_from_slice(&28u32.to_be_bytes());
        font[24..28].copy_from_slice(&(name.len() as u32).to_be_bytes());
        font.extend_from_slice(&name);
        font
    }

    #[test]
    fn installation_skips_identical_fonts_and_preserves_conflicts() {
        let base = env::temp_dir().join(format!(
            "wonderdraft-font-install-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let source = base.join("source/nested");
        let destination = base.join("destination");
        fs::create_dir_all(&source).unwrap();
        fs::create_dir_all(&destination).unwrap();
        fs::write(source.join("new.ttf"), b"new font").unwrap();
        fs::write(source.join("same.otf"), b"same font").unwrap();
        fs::write(source.join("conflict.ttc"), b"new version").unwrap();
        fs::write(source.join("ignored.txt"), b"not a font").unwrap();
        fs::write(destination.join("same.otf"), b"same font").unwrap();
        fs::write(destination.join("conflict.ttc"), b"installed version").unwrap();

        let mut selected = Vec::new();
        collect_fonts(base.join("source").as_path(), &mut selected).unwrap();
        let result = install_files_into(&selected, &destination).unwrap();

        assert_eq!(result.discovered, 3);
        assert_eq!(result.installed, 1);
        assert_eq!(result.already_installed, 1);
        assert_eq!(result.conflicts, 1);
        assert_eq!(fs::read(destination.join("new.ttf")).unwrap(), b"new font");
        assert_eq!(
            fs::read(destination.join("conflict.ttc")).unwrap(),
            b"installed version"
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn discovery_only_uses_named_custom_fonts_folders() {
        let base = env::temp_dir().join(format!(
            "wonderdraft-font-discovery-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&base);
        let core = base.join("wonderdraft_files/fonts");
        let custom = base.join("assets");
        fs::create_dir_all(&core).unwrap();
        fs::create_dir_all(custom.join("Pack One/fonts/nested")).unwrap();
        fs::create_dir_all(custom.join("Pack Two/FONTS")).unwrap();
        fs::create_dir_all(custom.join("Pack Three/sprites")).unwrap();
        fs::write(core.join("core.ttf"), b"core").unwrap();
        fs::write(custom.join("Pack One/fonts/nested/custom.otf"), b"one").unwrap();
        fs::write(custom.join("Pack Two/FONTS/custom-two.ttc"), b"two").unwrap();
        fs::write(custom.join("Pack Three/sprites/not-a-font.ttf"), b"ignored").unwrap();

        let found = discover(Some(&core), Some(&custom)).unwrap();

        assert_eq!(found.len(), 3);
        assert_eq!(
            found
                .iter()
                .filter(|candidate| candidate.origin == Origin::Core)
                .count(),
            1
        );
        assert_eq!(
            found
                .iter()
                .filter(|candidate| candidate.origin == Origin::Custom)
                .count(),
            2
        );
        assert!(
            found
                .iter()
                .all(|candidate| !candidate.path.ends_with("not-a-font.ttf"))
        );
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn extracts_internal_family_style_and_weight_from_font_name_table() {
        let base =
            env::temp_dir().join(format!("wonderdraft-font-name-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let path = base.join("IM Fell English Italic.ttf");
        fs::write(&path, test_font("IM FELL English", "Bold Italic")).unwrap();

        let name = font_name(&path).unwrap().unwrap();

        assert_eq!(name.family, "IM FELL English");
        assert_eq!(name.style, "italic");
        assert_eq!(name.weight, "bold");
        let _ = fs::remove_dir_all(base);
    }
}

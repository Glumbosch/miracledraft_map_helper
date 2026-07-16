use crate::{Error, Result, error::IoContext};
use std::{
    env, fs,
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Component, Path, PathBuf},
};

const PCK_MAGIC: u32 = 0x4350_4447; // "GDPC" as a little-endian u32.

#[derive(Clone, Debug)]
struct Entry {
    path: String,
    offset: u64,
    size: u64,
}

#[derive(Clone, Debug)]
pub struct Extraction {
    pub output_dir: PathBuf,
    pub sprites_dir: PathBuf,
    pub file_count: u32,
    pub renamed_images: u32,
}

/// Find Wonderdraft's core pack in the standard location for this OS.
pub fn find_default_pack() -> Option<PathBuf> {
    default_pack_paths().into_iter().find(|path| path.is_file())
}

pub fn default_output_dir() -> PathBuf {
    env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("wonderdraft_files")
}

fn default_pack_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if cfg!(target_os = "windows") {
        paths.push(PathBuf::from(
            r"C:\Program Files\Wonderdraft\wonderdraft.pck",
        ));
        paths.push(PathBuf::from(
            r"C:\Program Files (x86)\Wonderdraft\wonderdraft.pck",
        ));
    } else if cfg!(target_os = "macos") {
        paths.push(PathBuf::from(
            "/Applications/Wonderdraft.app/Contents/Resources/wonderdraft.pck",
        ));
    } else {
        paths.push(PathBuf::from("/opt/Wonderdraft/wonderdraft.pck"));
        if let Some(home) = env::var_os("HOME") {
            paths.push(PathBuf::from(home).join("Games/Wonderdraft/wonderdraft.pck"));
        }
    }
    paths
}

/// Extract a Godot PCK v1/v2 and make Wonderdraft image resources usable as PNGs.
pub fn extract(pack_path: &Path, output_dir: &Path) -> Result<Extraction> {
    let mut file = File::open(pack_path).at(pack_path)?;
    if read_u32(&mut file, pack_path)? != PCK_MAGIC {
        return Err(Error::format(format!(
            "{} is not a valid Godot PCK file",
            pack_path.display()
        )));
    }

    let format_version = read_u32(&mut file, pack_path)?;
    let _godot_major = read_u32(&mut file, pack_path)?;
    let _godot_minor = read_u32(&mut file, pack_path)?;
    let _godot_patch = read_u32(&mut file, pack_path)?;
    match format_version {
        1 => {}
        2 => {
            let flags = read_u32(&mut file, pack_path)?;
            let _file_base = read_u64(&mut file, pack_path)?;
            if flags != 0 {
                return Err(Error::format(format!(
                    "encrypted or otherwise flagged PCK files are not supported (flags: {flags})"
                )));
            }
        }
        version => {
            return Err(Error::format(format!(
                "unsupported PCK format version {version}"
            )));
        }
    }

    for _ in 0..16 {
        read_u32(&mut file, pack_path)?;
    }
    let file_count = read_u32(&mut file, pack_path)?;
    let mut entries = Vec::with_capacity(file_count as usize);
    for _ in 0..file_count {
        let path_len = read_u32(&mut file, pack_path)? as usize;
        let mut path_bytes = vec![0; path_len];
        file.read_exact(&mut path_bytes).at(pack_path)?;
        let valid_len = path_bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(path_bytes.len());
        let path = String::from_utf8_lossy(&path_bytes[..valid_len]).into_owned();
        let offset = read_u64(&mut file, pack_path)?;
        let size = read_u64(&mut file, pack_path)?;
        let mut md5 = [0; 16];
        file.read_exact(&mut md5).at(pack_path)?;
        if format_version == 2 {
            let file_flags = read_u32(&mut file, pack_path)?;
            if file_flags != 0 {
                return Err(Error::format(format!(
                    "encrypted PCK entry is not supported: {path}"
                )));
            }
        }
        entries.push(Entry { path, offset, size });
    }

    fs::create_dir_all(output_dir).at(output_dir)?;
    let mut renamed_images = 0;
    for entry in entries {
        let relative = safe_relative_path(&entry.path)?;
        let (relative, renamed) = png_path(relative);
        renamed_images += u32::from(renamed);
        let target = output_dir.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).at(parent)?;
        }
        file.seek(SeekFrom::Start(entry.offset)).at(pack_path)?;
        let mut output = File::create(&target).at(&target)?;
        let mut chunk = Read::by_ref(&mut file).take(entry.size);
        std::io::copy(&mut chunk, &mut output).at(&target)?;
        if chunk.limit() != 0 {
            return Err(Error::format(format!(
                "PCK entry ended early while extracting {}",
                entry.path
            )));
        }
    }

    let sprites_dir = output_dir.join("sprites");
    if !sprites_dir.is_dir() {
        return Err(Error::format(format!(
            "the pack was extracted, but it did not contain a sprites folder at {}",
            sprites_dir.display()
        )));
    }
    Ok(Extraction {
        output_dir: output_dir.to_owned(),
        sprites_dir,
        file_count,
        renamed_images,
    })
}

fn safe_relative_path(raw: &str) -> Result<PathBuf> {
    let raw = raw.strip_prefix("res://").unwrap_or(raw).replace('\\', "/");
    let path = Path::new(&raw);
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err(Error::format(format!("unsafe path in PCK: {raw}")));
    }
    Ok(path.to_owned())
}

fn png_path(mut path: PathBuf) -> (PathBuf, bool) {
    let rename = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            ext.eq_ignore_ascii_case("wonderdraft_image")
                || ext.eq_ignore_ascii_case("wonderdraft-image")
        });
    if rename {
        path.set_extension("png");
    }
    (path, rename)
}

fn read_u32(reader: &mut impl Read, path: &Path) -> Result<u32> {
    let mut bytes = [0; 4];
    reader.read_exact(&mut bytes).at(path)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(reader: &mut impl Read, path: &Path) -> Result<u64> {
    let mut bytes = [0; 8];
    reader.read_exact(&mut bytes).at(path)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn wonderdraft_images_become_pngs() {
        assert_eq!(
            png_path(PathBuf::from("sprites/bridge.wonderdraft_image")),
            (PathBuf::from("sprites/bridge.png"), true)
        );
    }

    #[test]
    fn archive_paths_cannot_escape_output() {
        assert!(safe_relative_path("res://sprites/bridge.png").is_ok());
        assert!(safe_relative_path("res://../outside.png").is_err());
        assert!(safe_relative_path("/absolute.png").is_err());
    }

    #[test]
    fn extracts_pack_and_configures_renamed_sprite_folder() {
        let base = env::temp_dir().join(format!("wonderdraft-pck-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let pack = base.join("wonderdraft.pck");
        let output = base.join("wonderdraft_files");
        let path = b"res://sprites/bridge_02_flat.wonderdraft_image\0";
        let payload = b"fake png payload";
        let header_size = 22 * 4;
        let table_size = 4 + path.len() + 8 + 8 + 16;
        let payload_offset = (header_size + table_size) as u64;

        let mut file = File::create(&pack).unwrap();
        file.write_all(&PCK_MAGIC.to_le_bytes()).unwrap();
        file.write_all(&1u32.to_le_bytes()).unwrap();
        file.write_all(&4u32.to_le_bytes()).unwrap();
        file.write_all(&0u32.to_le_bytes()).unwrap();
        file.write_all(&0u32.to_le_bytes()).unwrap();
        for _ in 0..16 {
            file.write_all(&0u32.to_le_bytes()).unwrap();
        }
        file.write_all(&1u32.to_le_bytes()).unwrap();
        file.write_all(&(path.len() as u32).to_le_bytes()).unwrap();
        file.write_all(path).unwrap();
        file.write_all(&payload_offset.to_le_bytes()).unwrap();
        file.write_all(&(payload.len() as u64).to_le_bytes())
            .unwrap();
        file.write_all(&[0; 16]).unwrap();
        file.write_all(payload).unwrap();
        drop(file);

        let extracted = extract(&pack, &output).unwrap();
        assert_eq!(extracted.file_count, 1);
        assert_eq!(extracted.renamed_images, 1);
        assert_eq!(extracted.sprites_dir, output.join("sprites"));
        assert_eq!(
            fs::read(output.join("sprites/bridge_02_flat.png")).unwrap(),
            payload
        );
        let _ = fs::remove_dir_all(base);
    }
}

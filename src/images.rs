use crate::{ByteSource, Error, ImageInfo, Result, Value, error::IoContext, value::image_info};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

pub type Images = Vec<(String, Value)>;

pub fn find(root: &Value) -> Images {
    let mut out = Vec::new();
    walk(root, &mut Vec::new(), &mut out);
    out
}
fn walk(v: &Value, path: &mut Vec<String>, out: &mut Images) {
    if image_info(v).is_some() {
        out.push((path.join("."), v.clone()));
        return;
    }
    match v {
        Value::Dictionary(d) => {
            for (k, v) in d {
                path.push(
                    k.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| crate::godot_text::format(k)),
                );
                walk(v, path, out);
                path.pop();
            }
        }
        Value::Array(a) => {
            for (i, v) in a.iter().enumerate() {
                path.push(i.to_string());
                walk(v, path, out);
                path.pop();
            }
        }
        Value::Object { properties, .. } => {
            for (k, v) in properties {
                path.push("properties".into());
                path.push(k.clone());
                walk(v, path, out);
                path.pop();
                path.pop();
            }
        }
        _ => {}
    }
}

pub fn placeholders(root: &Value, images: &Images) -> Value {
    replace(root, images, &mut Vec::new(), false)
}
pub fn restore(root: &Value, images: &Images) -> Value {
    replace(root, images, &mut Vec::new(), true)
}
fn replace(v: &Value, images: &Images, path: &mut Vec<String>, restore: bool) -> Value {
    let joined = path.join(".");
    if let Some((_, image)) = images.iter().find(|(p, _)| p == &joined) {
        if restore {
            return image.clone();
        }
        let leaf = path.last().map(String::as_str).unwrap_or("image");
        return Value::String(format!(".{leaf}.png"));
    }
    match v {
        Value::Dictionary(d) => Value::Dictionary(
            d.iter()
                .map(|(k, v)| {
                    path.push(
                        k.as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| crate::godot_text::format(k)),
                    );
                    let x = replace(v, images, path, restore);
                    path.pop();
                    (k.clone(), x)
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(
            a.iter()
                .enumerate()
                .map(|(i, v)| {
                    path.push(i.to_string());
                    let x = replace(v, images, path, restore);
                    path.pop();
                    x
                })
                .collect(),
        ),
        _ => v.clone(),
    }
}

fn png_color(format: &str) -> Result<png::ColorType> {
    match format {
        "L8" => Ok(png::ColorType::Grayscale),
        "LA8" => Ok(png::ColorType::GrayscaleAlpha),
        "RGB8" => Ok(png::ColorType::Rgb),
        "RGBA8" => Ok(png::ColorType::Rgba),
        _ => Err(Error::format(format!(
            "unsupported image format {format:?}"
        ))),
    }
}
fn channels(format: &str) -> Result<usize> {
    match format {
        "L8" => Ok(1),
        "LA8" => Ok(2),
        "RGB8" => Ok(3),
        "RGBA8" => Ok(4),
        _ => Err(Error::format(format!(
            "unsupported image format {format:?}"
        ))),
    }
}
pub fn export_png(path: &Path, info: &ImageInfo) -> Result<()> {
    let needed = info.width as u64 * info.height as u64 * channels(&info.format)? as u64;
    if info.pixels.len() < needed {
        return Err(Error::format("embedded image buffer is too short"));
    }
    let file = fs::File::create(path).at(path)?;
    let mut encoder = png::Encoder::new(file, info.width, info.height);
    encoder.set_color(png_color(&info.format)?);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let mut stream = writer.stream_writer_with_size(1024 * 1024)?;
    match &info.pixels {
        ByteSource::Memory(b) => stream.write_all(&b[..needed as usize]).at(path)?,
        ByteSource::File {
            path: source,
            offset,
            ..
        } => {
            use std::io::{Read, Seek};
            let mut input = fs::File::open(source).at(source)?;
            input.seek(std::io::SeekFrom::Start(*offset)).at(source)?;
            let mut limited = input.take(needed);
            std::io::copy(&mut limited, &mut stream).at(path)?;
        }
    }
    stream.finish()?;
    Ok(())
}

pub fn import_image(path: &Path, template: &Value, cache: &Path) -> Result<Value> {
    let old = image_info(template)
        .ok_or_else(|| Error::format("selected value is not an Image object"))?;
    let img = image::open(path)?.to_rgba8();
    if img.width() != old.width || img.height() != old.height {
        return Err(Error::format(format!(
            "image is {}x{}; slot requires {}x{}",
            img.width(),
            img.height(),
            old.width,
            old.height
        )));
    }
    fs::create_dir_all(cache).at(cache)?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let raw_path = cache.join(format!("replacement_{}_{}.rgba", std::process::id(), stamp));
    fs::write(&raw_path, img.as_raw()).at(&raw_path)?;
    let data = Value::Dictionary(vec![
        (
            Value::String("width".into()),
            Value::Int(img.width() as i64),
        ),
        (
            Value::String("height".into()),
            Value::Int(img.height() as i64),
        ),
        (Value::String("mipmaps".into()), Value::Bool(false)),
        (
            Value::String("format".into()),
            Value::String("RGBA8".into()),
        ),
        (
            Value::String("data".into()),
            Value::PoolByteArray(ByteSource::File {
                path: raw_path,
                offset: 0,
                len: (img.width() as u64) * (img.height() as u64) * 4,
            }),
        ),
    ]);
    Ok(Value::Object {
        class: "Image".into(),
        properties: vec![("data".into(), data)],
    })
}

pub fn thumbnail(info: &ImageInfo, max: usize) -> Result<(usize, usize, Vec<u8>)> {
    let c = channels(&info.format)?;
    let scale = (max as f64 / info.width.max(info.height) as f64).min(1.0);
    let w = (info.width as f64 * scale).round().max(1.0) as usize;
    let h = (info.height as f64 * scale).round().max(1.0) as usize;
    let mut rgba = vec![0; w * h * 4];
    for y in 0..h {
        let sy = (y * info.height as usize / h).min(info.height as usize - 1);
        let row = info.pixels.read_slice(
            (sy * info.width as usize * c) as u64,
            info.width as usize * c,
        )?;
        for x in 0..w {
            let sx = (x * info.width as usize / w).min(info.width as usize - 1);
            let src = &row[sx * c..sx * c + c];
            let dst = &mut rgba[(y * w + x) * 4..(y * w + x + 1) * 4];
            match c {
                1 => dst.copy_from_slice(&[src[0], src[0], src[0], 255]),
                2 => dst.copy_from_slice(&[src[0], src[0], src[0], src[1]]),
                3 => dst.copy_from_slice(&[src[0], src[1], src[2], 255]),
                4 => dst.copy_from_slice(src),
                _ => unreachable!(),
            }
        }
    }
    Ok((w, h, rgba))
}

pub fn temp_cache_dir(base: &Path) -> Result<PathBuf> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let p = base.join(format!("wonderdraft_rust_{}_{}", std::process::id(), stamp));
    fs::create_dir_all(&p).at(&p)?;
    Ok(p)
}

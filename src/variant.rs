use crate::{ByteSource, Error, Result, Value, error::IoContext, gcpf};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};

const FLAG_64: u32 = 1 << 16;

fn type_name(id: u8) -> Option<&'static str> {
    Some(match id {
        0 => "Nil",
        1 => "Bool",
        2 => "Int",
        3 => "Real",
        4 => "String",
        5 => "Vector2",
        6 => "Rect2",
        7 => "Vector3",
        8 => "Transform2D",
        9 => "Plane",
        10 => "Quat",
        11 => "AABB",
        12 => "Basis",
        13 => "Transform",
        14 => "Color",
        15 => "NodePath",
        16 => "RID",
        17 => "Object",
        18 => "Dictionary",
        19 => "Array",
        20 => "PoolByteArray",
        21 => "PoolIntArray",
        22 => "PoolRealArray",
        23 => "PoolStringArray",
        24 => "PoolVector2Array",
        25 => "PoolVector3Array",
        26 => "PoolColorArray",
        _ => return None,
    })
}
fn type_id(name: &str) -> Option<u32> {
    (0..=26)
        .find(|&id| type_name(id).is_some_and(|n| n == name))
        .map(u32::from)
}

struct Parser {
    file: File,
    path: std::path::PathBuf,
    payload_len: u64,
    position: u64,
}
impl Parser {
    fn read<const N: usize>(&mut self) -> Result<[u8; N]> {
        if self.position + N as u64 > self.payload_len {
            return Err(Error::format("unexpected end of Godot Variant data"));
        }
        let mut b = [0; N];
        self.file.read_exact(&mut b).at(&self.path)?;
        self.position += N as u64;
        Ok(b)
    }
    fn skip(&mut self, n: u64) -> Result<()> {
        if self.position + n > self.payload_len {
            return Err(Error::format("unexpected end of Godot Variant data"));
        }
        self.file.seek(SeekFrom::Current(n as i64)).at(&self.path)?;
        self.position += n;
        Ok(())
    }
    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.read()?))
    }
    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.read()?))
    }
    fn i64(&mut self) -> Result<i64> {
        Ok(i64::from_le_bytes(self.read()?))
    }
    fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.read()?))
    }
    fn f64(&mut self) -> Result<f64> {
        Ok(f64::from_le_bytes(self.read()?))
    }
    fn string(&mut self) -> Result<String> {
        let n = self.u32()? as usize;
        let mut b = vec![0; n];
        if self.position + n as u64 > self.payload_len {
            return Err(Error::format("truncated string"));
        }
        self.file.read_exact(&mut b).at(&self.path)?;
        self.position += n as u64;
        self.skip(((4 - n % 4) % 4) as u64)?;
        String::from_utf8(b).map_err(|_| Error::format("invalid UTF-8 in Godot Variant"))
    }
    fn parse(&mut self, depth: usize) -> Result<Value> {
        if depth > 100 {
            return Err(Error::format("Godot Variant nesting is unreasonably deep"));
        }
        let header = self.u32()?;
        let id = (header & 255) as u8;
        let flags = header & !255;
        let name = type_name(id)
            .ok_or_else(|| Error::format(format!("unknown Godot Variant type {id}")))?;
        Ok(match id {
            0 => Value::Nil,
            1 => Value::Bool(self.u32()? != 0),
            2 => Value::Int(if flags & FLAG_64 != 0 {
                self.i64()?
            } else {
                self.i32()? as i64
            }),
            3 => Value::Real(if flags & FLAG_64 != 0 {
                self.f64()?
            } else {
                self.f32()? as f64
            }),
            4 => Value::String(self.string()?),
            5..=14 => {
                let n = match id {
                    5 => 2,
                    6 => 4,
                    7 => 3,
                    8 => 6,
                    9 | 10 | 14 => 4,
                    11 => 6,
                    12 => 9,
                    13 => 12,
                    _ => unreachable!(),
                };
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(self.f32()?);
                }
                Value::Vector {
                    kind: name.into(),
                    values: v,
                }
            }
            15 => {
                let nc = self.u32()?;
                if nc & 0x8000_0000 == 0 {
                    return Err(Error::format("old-format NodePath is unsupported"));
                }
                let nn = (nc & 0x7fff_ffff) as usize;
                let mut sn = self.u32()? as usize;
                let nf = self.u32()?;
                if nf & 2 != 0 {
                    sn += 1;
                }
                let mut names = Vec::with_capacity(nn);
                for _ in 0..nn {
                    names.push(self.string()?);
                }
                let mut subnames = Vec::with_capacity(sn);
                for _ in 0..sn {
                    subnames.push(self.string()?);
                }
                Value::NodePath {
                    names,
                    subnames,
                    absolute: nf & 1 != 0,
                }
            }
            16 => Value::Rid,
            17 => {
                if flags & FLAG_64 != 0 {
                    Value::ObjectId(self.i64()?)
                } else {
                    let class = self.string()?;
                    if class.is_empty() {
                        Value::Nil
                    } else {
                        let n = self.u32()? as usize;
                        let mut properties = Vec::with_capacity(n);
                        for _ in 0..n {
                            let key = self.string()?;
                            let value = self.parse(depth + 1)?;
                            properties.push((key, value));
                        }
                        Value::Object { class, properties }
                    }
                }
            }
            18 => {
                let n = (self.u32()? & 0x7fff_ffff) as usize;
                let mut d = Vec::with_capacity(n);
                for _ in 0..n {
                    let k = self.parse(depth + 1)?;
                    let v = self.parse(depth + 1)?;
                    d.push((k, v));
                }
                Value::Dictionary(d)
            }
            19 => {
                let n = (self.u32()? & 0x7fff_ffff) as usize;
                let mut a = Vec::with_capacity(n);
                for _ in 0..n {
                    a.push(self.parse(depth + 1)?);
                }
                Value::Array(a)
            }
            20 => {
                let n = self.u32()? as u64;
                let offset = self.file.stream_position().at(&self.path)?;
                self.skip(n)?;
                self.skip((4 - n % 4) % 4)?;
                Value::PoolByteArray(ByteSource::File {
                    path: self.path.clone(),
                    offset,
                    len: n,
                })
            }
            21 => {
                let n = self.u32()? as usize;
                let mut a = Vec::with_capacity(n);
                for _ in 0..n {
                    a.push(self.i32()?);
                }
                Value::PoolIntArray(a)
            }
            22 => {
                let n = self.u32()? as usize;
                let mut a = Vec::with_capacity(n);
                for _ in 0..n {
                    a.push(self.f32()?);
                }
                Value::PoolRealArray(a)
            }
            23 => {
                let n = self.u32()? as usize;
                let mut a = Vec::with_capacity(n);
                for _ in 0..n {
                    a.push(self.string()?);
                }
                Value::PoolStringArray(a)
            }
            24..=26 => {
                let components = match id {
                    24 => 2,
                    25 => 3,
                    _ => 4,
                };
                let n = self.u32()? as usize;
                let mut values = Vec::with_capacity(n);
                for _ in 0..n {
                    let mut v = Vec::with_capacity(components);
                    for _ in 0..components {
                        v.push(self.f32()?);
                    }
                    values.push(v);
                }
                Value::PoolVectors {
                    kind: name.into(),
                    components,
                    values,
                }
            }
            _ => unreachable!(),
        })
    }
}

pub fn decode_file(path: &Path) -> Result<Value> {
    let mut file = File::open(path).at(path)?;
    let size = file.metadata().at(path)?.len();
    let mut p = [0; 4];
    file.read_exact(&mut p).at(path)?;
    let len = u32::from_le_bytes(p) as u64;
    if len + 4 != size {
        return Err(Error::format(format!(
            "Variant length prefix says {len}, but {} bytes follow",
            size - 4
        )));
    }
    let mut parser = Parser {
        file,
        path: path.to_owned(),
        payload_len: len,
        position: 0,
    };
    let value = parser.parse(0)?;
    if parser.position != len {
        return Err(Error::format(
            "Variant decoder did not consume the entire payload",
        ));
    }
    Ok(value)
}

fn raw_string_size(s: &str) -> u64 {
    let n = s.len() as u64;
    4 + n + (4 - n % 4) % 4
}
pub fn encoded_size(v: &Value) -> Result<u64> {
    Ok(match v {
        Value::Nil | Value::Rid => 4,
        Value::Bool(_) => 8,
        Value::Int(n) => {
            if i32::try_from(*n).is_ok() {
                8
            } else {
                12
            }
        }
        Value::Real(_) => 8,
        Value::String(s) => 4 + raw_string_size(s),
        Value::Dictionary(d) => {
            8 + d
                .iter()
                .map(|(k, v)| Ok(encoded_size(k)? + encoded_size(v)?))
                .collect::<Result<Vec<u64>>>()?
                .into_iter()
                .sum::<u64>()
        }
        Value::Array(a) => {
            8 + a
                .iter()
                .map(encoded_size)
                .collect::<Result<Vec<_>>>()?
                .into_iter()
                .sum::<u64>()
        }
        Value::Vector { values, .. } => 4 + 4 * values.len() as u64,
        Value::Object { class, properties } => {
            4 + raw_string_size(class)
                + 4
                + properties
                    .iter()
                    .map(|(k, v)| Ok(raw_string_size(k) + encoded_size(v)?))
                    .collect::<Result<Vec<u64>>>()?
                    .into_iter()
                    .sum::<u64>()
        }
        Value::NodePath {
            names, subnames, ..
        } => {
            16 + names
                .iter()
                .chain(subnames)
                .map(|s| raw_string_size(s))
                .sum::<u64>()
        }
        Value::ObjectId(_) => 12,
        Value::PoolByteArray(b) => 8 + b.len() + (4 - b.len() % 4) % 4,
        Value::PoolIntArray(a) => 8 + 4 * a.len() as u64,
        Value::PoolRealArray(a) => 8 + 4 * a.len() as u64,
        Value::PoolStringArray(a) => 8 + a.iter().map(|s| raw_string_size(s)).sum::<u64>(),
        Value::PoolVectors {
            components, values, ..
        } => 8 + 4 * (*components * values.len()) as u64,
    })
}

fn header(w: &mut impl Write, name: &str, flags: u32) -> Result<()> {
    let id = type_id(name).ok_or_else(|| Error::format(format!("unsupported type {name}")))?;
    w.write_all(&(id | flags).to_le_bytes())
        .map_err(|e| Error::format(e.to_string()))
}
fn write_string(w: &mut impl Write, s: &str) -> Result<()> {
    let n: u32 = s
        .len()
        .try_into()
        .map_err(|_| Error::format("string too large"))?;
    w.write_all(&n.to_le_bytes())
        .map_err(|e| Error::format(e.to_string()))?;
    w.write_all(s.as_bytes())
        .map_err(|e| Error::format(e.to_string()))?;
    let pad = (4 - s.len() % 4) % 4;
    w.write_all(&[0; 3][..pad])
        .map_err(|e| Error::format(e.to_string()))
}
pub fn write_value(w: &mut impl Write, v: &Value) -> Result<()> {
    match v {
        Value::Nil => header(w, "Nil", 0)?,
        Value::Bool(v) => {
            header(w, "Bool", 0)?;
            w.write_all(&(*v as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?
        }
        Value::Int(v) => {
            if let Ok(n) = i32::try_from(*v) {
                header(w, "Int", 0)?;
                w.write_all(&n.to_le_bytes())
                    .map_err(|e| Error::format(e.to_string()))?
            } else {
                header(w, "Int", FLAG_64)?;
                w.write_all(&v.to_le_bytes())
                    .map_err(|e| Error::format(e.to_string()))?
            }
        }
        Value::Real(v) => {
            header(w, "Real", 0)?;
            w.write_all(&(*v as f32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?
        }
        Value::String(s) => {
            header(w, "String", 0)?;
            write_string(w, s)?
        }
        Value::Dictionary(d) => {
            header(w, "Dictionary", 0)?;
            w.write_all(&(d.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            for (k, v) in d {
                write_value(w, k)?;
                write_value(w, v)?
            }
        }
        Value::Array(a) => {
            header(w, "Array", 0)?;
            w.write_all(&(a.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            for v in a {
                write_value(w, v)?
            }
        }
        Value::Vector { kind, values } => {
            header(w, kind, 0)?;
            for v in values {
                w.write_all(&v.to_le_bytes())
                    .map_err(|e| Error::format(e.to_string()))?
            }
        }
        Value::Object { class, properties } => {
            header(w, "Object", 0)?;
            write_string(w, class)?;
            w.write_all(&(properties.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            for (k, v) in properties {
                write_string(w, k)?;
                write_value(w, v)?
            }
        }
        Value::NodePath {
            names,
            subnames,
            absolute,
        } => {
            header(w, "NodePath", 0)?;
            w.write_all(&(0x8000_0000 | names.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            w.write_all(&(subnames.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            w.write_all(&(*absolute as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            for s in names.iter().chain(subnames) {
                write_string(w, s)?
            }
        }
        Value::Rid => header(w, "RID", 0)?,
        Value::ObjectId(id) => {
            header(w, "Object", FLAG_64)?;
            w.write_all(&id.to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?
        }
        Value::PoolByteArray(b) => {
            header(w, "PoolByteArray", 0)?;
            let n: u32 = b
                .len()
                .try_into()
                .map_err(|_| Error::format("byte array exceeds 4 GiB"))?;
            w.write_all(&n.to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            b.copy_to(w)?;
            let p = (4 - b.len() % 4) % 4;
            w.write_all(&[0; 3][..p as usize])
                .map_err(|e| Error::format(e.to_string()))?
        }
        Value::PoolIntArray(a) => {
            header(w, "PoolIntArray", 0)?;
            w.write_all(&(a.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            for n in a {
                w.write_all(&n.to_le_bytes())
                    .map_err(|e| Error::format(e.to_string()))?
            }
        }
        Value::PoolRealArray(a) => {
            header(w, "PoolRealArray", 0)?;
            w.write_all(&(a.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            for n in a {
                w.write_all(&n.to_le_bytes())
                    .map_err(|e| Error::format(e.to_string()))?
            }
        }
        Value::PoolStringArray(a) => {
            header(w, "PoolStringArray", 0)?;
            w.write_all(&(a.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            for s in a {
                write_string(w, s)?
            }
        }
        Value::PoolVectors { kind, values, .. } => {
            header(w, kind, 0)?;
            w.write_all(&(values.len() as u32).to_le_bytes())
                .map_err(|e| Error::format(e.to_string()))?;
            for row in values {
                for n in row {
                    w.write_all(&n.to_le_bytes())
                        .map_err(|e| Error::format(e.to_string()))?
                }
            }
        }
    }
    Ok(())
}

pub fn save_map(
    root: &Value,
    destination: &Path,
    block_size: u32,
    compressed: bool,
) -> Result<u64> {
    let payload = encoded_size(root)?;
    let total = payload + 4;
    if let Some(p) = destination.parent() {
        fs::create_dir_all(p).at(p)?;
    }
    let temp = destination.with_extension("wonderdraft_map.rusttmp");
    let mut writer = gcpf::Writer::new(&temp, total, block_size, compressed)?;
    writer
        .write_all(&(payload as u32).to_le_bytes())
        .map_err(|e| Error::format(e.to_string()))?;
    write_value(&mut writer, root)?;
    let size = writer.finish()?;
    fs::rename(&temp, destination).at(destination)?;
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Value {
        Value::Dictionary(vec![
            (
                Value::String("x".into()),
                Value::Array(vec![Value::Int(2), Value::Real(3.5), Value::Bool(true)]),
            ),
            (
                Value::String("point".into()),
                Value::Vector {
                    kind: "Vector2".into(),
                    values: vec![12.0, -5.25],
                },
            ),
        ])
    }

    #[test]
    fn sizes_match() {
        let value = sample();
        let mut bytes = Vec::new();
        write_value(&mut bytes, &value).unwrap();
        assert_eq!(bytes.len() as u64, encoded_size(&value).unwrap());
    }

    #[test]
    fn complete_map_container_round_trip() {
        let base =
            std::env::temp_dir().join(format!("wonderdraft-rust-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        let map = base.join("sample.wonderdraft_map");
        let raw = base.join("sample.variant");
        save_map(&sample(), &map, 64, true).unwrap();
        crate::gcpf::decompress_file(&map, &raw, |_, _| {}).unwrap();
        let decoded = decode_file(&raw).unwrap();
        assert!(matches!(decoded.get("x"), Some(Value::Array(values)) if values.len() == 3));
        assert!(
            matches!(decoded.get("point"), Some(Value::Vector { kind, values }) if kind == "Vector2" && values.len() == 2)
        );
        let _ = fs::remove_dir_all(base);
    }
}

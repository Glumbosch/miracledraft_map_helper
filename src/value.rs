use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
};

use crate::{Error, Result, error::IoContext};

#[derive(Clone, Debug)]
pub enum ByteSource {
    Memory(Vec<u8>),
    File {
        path: PathBuf,
        offset: u64,
        len: u64,
    },
}

impl ByteSource {
    pub fn len(&self) -> u64 {
        match self {
            Self::Memory(bytes) => bytes.len() as u64,
            Self::File { len, .. } => *len,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn read_slice(&self, start: u64, len: usize) -> Result<Vec<u8>> {
        if start.saturating_add(len as u64) > self.len() {
            return Err(Error::format(
                "byte-source slice is outside the stored range",
            ));
        }
        match self {
            Self::Memory(bytes) => Ok(bytes[start as usize..start as usize + len].to_vec()),
            Self::File { path, offset, .. } => {
                let mut file = File::open(path).at(path)?;
                file.seek(SeekFrom::Start(offset + start)).at(path)?;
                let mut out = vec![0; len];
                file.read_exact(&mut out).at(path)?;
                Ok(out)
            }
        }
    }

    pub fn copy_to(&self, output: &mut impl std::io::Write) -> Result<()> {
        match self {
            Self::Memory(bytes) => output
                .write_all(bytes)
                .map_err(|e| Error::format(e.to_string())),
            Self::File { path, offset, len } => {
                let mut file = File::open(path).at(path)?;
                file.seek(SeekFrom::Start(*offset)).at(path)?;
                let mut limited = file.take(*len);
                std::io::copy(&mut limited, output).map_err(|e| Error::format(e.to_string()))?;
                if limited.limit() != 0 {
                    return Err(Error::format("unexpected end of disk-backed byte array"));
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone, Debug)]
pub enum Value {
    Nil,
    Bool(bool),
    Int(i64),
    Real(f64),
    String(String),
    Dictionary(Vec<(Value, Value)>),
    Array(Vec<Value>),
    Vector {
        kind: String,
        values: Vec<f32>,
    },
    Object {
        class: String,
        properties: Vec<(String, Value)>,
    },
    NodePath {
        names: Vec<String>,
        subnames: Vec<String>,
        absolute: bool,
    },
    Rid,
    ObjectId(i64),
    PoolByteArray(ByteSource),
    /// Editable placeholder for a disk-backed PoolByteArray held separately.
    /// This value must be restored before binary encoding.
    PoolByteArrayRef {
        path: String,
        len: u64,
    },
    PoolIntArray(Vec<i32>),
    PoolRealArray(Vec<f32>),
    PoolStringArray(Vec<String>),
    PoolVectors {
        kind: String,
        components: usize,
        values: Vec<Vec<f32>>,
    },
}

impl Value {
    pub fn dict() -> Self {
        Self::Dictionary(Vec::new())
    }
    pub fn as_dict(&self) -> Option<&[(Value, Value)]> {
        if let Self::Dictionary(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_dict_mut(&mut self) -> Option<&mut Vec<(Value, Value)>> {
        if let Self::Dictionary(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_array(&self) -> Option<&[Value]> {
        if let Self::Array(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        if let Self::Array(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        if let Self::String(v) = self {
            Some(v)
        } else {
            None
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Real(v) => Some(*v),
            Self::Int(v) => Some(*v as f64),
            _ => None,
        }
    }
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.as_dict()?
            .iter()
            .find_map(|(k, v)| (k.as_str() == Some(key)).then_some(v))
    }
    pub fn get_mut(&mut self, key: &str) -> Option<&mut Value> {
        self.as_dict_mut()?
            .iter_mut()
            .find_map(|(k, v)| (k.as_str() == Some(key)).then_some(v))
    }
    pub fn set(&mut self, key: impl Into<String>, value: Value) {
        let key = key.into();
        if let Some(dict) = self.as_dict_mut() {
            if let Some((_, old)) = dict.iter_mut().find(|(k, _)| k.as_str() == Some(&key)) {
                *old = value;
            } else {
                dict.push((Value::String(key), value));
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImageInfo {
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub mipmaps: bool,
    pub pixels: ByteSource,
}

pub fn image_info(value: &Value) -> Option<ImageInfo> {
    let Value::Object { class, properties } = value else {
        return None;
    };
    if class != "Image" {
        return None;
    }
    let data = properties.iter().find(|(k, _)| k == "data")?.1.as_dict()?;
    let lookup = |name: &str| {
        data.iter()
            .find_map(|(k, v)| (k.as_str() == Some(name)).then_some(v))
    };
    let pixels = match lookup("data")? {
        Value::PoolByteArray(v) => v.clone(),
        _ => return None,
    };
    Some(ImageInfo {
        width: lookup("width")?.as_f64()? as u32,
        height: lookup("height")?.as_f64()? as u32,
        format: lookup("format")?.as_str()?.to_owned(),
        mipmaps: matches!(lookup("mipmaps"), Some(Value::Bool(true))),
        pixels,
    })
}

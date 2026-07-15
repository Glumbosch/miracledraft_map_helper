use crate::{Error, Result, error::IoContext, fastlz};
use std::{
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
};

const MAGIC: &[u8; 4] = b"GCPF";

#[derive(Clone, Debug)]
pub struct Metadata {
    pub block_size: u32,
    pub block_count: u32,
    pub uncompressed_size: u32,
    pub compressed_size: u64,
}

fn read_u32(reader: &mut impl Read) -> Result<u32> {
    let mut b = [0; 4];
    reader
        .read_exact(&mut b)
        .map_err(|e| Error::format(e.to_string()))?;
    Ok(u32::from_le_bytes(b))
}

pub fn decompress_file(
    source: &Path,
    destination: &Path,
    mut progress: impl FnMut(u64, u64),
) -> Result<Metadata> {
    let mut input = File::open(source).at(source)?;
    let compressed_size = input.metadata().at(source)?.len();
    let mut magic = [0; 4];
    input.read_exact(&mut magic).at(source)?;
    if &magic != MAGIC {
        return Err(Error::format("invalid GCPF magic"));
    }
    let mode = read_u32(&mut input)?;
    let block_size = read_u32(&mut input)?;
    let raw_size = read_u32(&mut input)?;
    if mode != 0 || block_size == 0 {
        return Err(Error::format("unsupported GCPF mode or block size"));
    }
    let block_count = raw_size / block_size + 1;
    let mut sizes = Vec::with_capacity(block_count as usize);
    for _ in 0..block_count {
        sizes.push(read_u32(&mut input)?);
    }
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).at(parent)?;
    }
    let mut output = File::create(destination).at(destination)?;
    let mut written = 0u64;
    for (index, size) in sizes.into_iter().enumerate() {
        let mut packed = vec![0; size as usize];
        input.read_exact(&mut packed).at(source)?;
        let expected = if index + 1 == block_count as usize {
            raw_size - block_size * (block_count - 1)
        } else {
            block_size
        };
        let block = fastlz::decompress(&packed, expected as usize)?;
        if block.len() != expected as usize {
            return Err(Error::format(format!(
                "GCPF block {index} has wrong decoded length"
            )));
        }
        output.write_all(&block).at(destination)?;
        written += block.len() as u64;
        progress(written, raw_size as u64);
    }
    input.read_exact(&mut magic).at(source)?;
    if &magic != MAGIC {
        return Err(Error::format("missing trailing GCPF magic"));
    }
    let mut extra = [0];
    if input.read(&mut extra).at(source)? != 0 {
        return Err(Error::format("unexpected bytes after GCPF trailer"));
    }
    Ok(Metadata {
        block_size,
        block_count,
        uncompressed_size: raw_size,
        compressed_size,
    })
}

pub struct Writer {
    path: PathBuf,
    file: File,
    raw_size: u32,
    block_size: usize,
    block_count: usize,
    compressed: bool,
    table_offset: u64,
    buffer: Vec<u8>,
    sizes: Vec<u32>,
    written: u64,
}

impl Writer {
    pub fn new(path: &Path, raw_size: u64, block_size: u32, compressed: bool) -> Result<Self> {
        let raw_size: u32 = raw_size
            .try_into()
            .map_err(|_| Error::format("Godot GCPF size exceeds 4 GiB format limit"))?;
        let block_size = block_size as usize;
        if block_size == 0 {
            return Err(Error::format("block size must be positive"));
        }
        let block_count = raw_size as usize / block_size + 1;
        let mut file = File::create(path).at(path)?;
        file.write_all(MAGIC).at(path)?;
        file.write_all(&0u32.to_le_bytes()).at(path)?;
        file.write_all(&(block_size as u32).to_le_bytes())
            .at(path)?;
        file.write_all(&raw_size.to_le_bytes()).at(path)?;
        let table_offset = file.stream_position().at(path)?;
        file.seek(SeekFrom::Current((block_count * 4) as i64))
            .at(path)?;
        Ok(Self {
            path: path.to_owned(),
            file,
            raw_size,
            block_size,
            block_count,
            compressed,
            table_offset,
            buffer: Vec::with_capacity(block_size),
            sizes: Vec::with_capacity(block_count),
            written: 0,
        })
    }
    fn emit(&mut self, block: &[u8]) -> Result<()> {
        let packed = if self.compressed {
            fastlz::compress(block)
        } else {
            fastlz::literal_block(block)
        };
        self.file.write_all(&packed).at(&self.path)?;
        self.sizes.push(packed.len() as u32);
        Ok(())
    }
    pub fn finish(mut self) -> Result<u64> {
        if self.written != self.raw_size as u64 {
            return Err(Error::format(format!(
                "wrote {} bytes; expected {}",
                self.written, self.raw_size
            )));
        }
        if !self.buffer.is_empty() {
            let block = std::mem::take(&mut self.buffer);
            self.emit(&block)?;
        }
        while self.sizes.len() < self.block_count {
            self.emit(&[])?;
        }
        self.file.write_all(MAGIC).at(&self.path)?;
        let end = self.file.stream_position().at(&self.path)?;
        self.file
            .seek(SeekFrom::Start(self.table_offset))
            .at(&self.path)?;
        for size in &self.sizes {
            self.file.write_all(&size.to_le_bytes()).at(&self.path)?;
        }
        self.file.sync_all().at(&self.path)?;
        Ok(end)
    }
}

impl Write for Writer {
    fn write(&mut self, mut data: &[u8]) -> std::io::Result<usize> {
        let total = data.len();
        while !data.is_empty() {
            let take = (self.block_size - self.buffer.len()).min(data.len());
            self.buffer.extend_from_slice(&data[..take]);
            data = &data[take..];
            self.written += take as u64;
            if self.buffer.len() == self.block_size {
                let block = std::mem::take(&mut self.buffer);
                self.emit(&block).map_err(std::io::Error::other)?;
                self.buffer = Vec::with_capacity(self.block_size);
            }
        }
        Ok(total)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

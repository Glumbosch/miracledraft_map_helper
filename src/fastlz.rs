use crate::{Error, Result};

pub fn decompress(source: &[u8], max_output: usize) -> Result<Vec<u8>> {
    if source.is_empty() {
        return Ok(Vec::new());
    }
    let level = (source[0] >> 5) + 1;
    if level != 1 && level != 2 {
        return Err(Error::format(format!("unsupported FastLZ level {level}")));
    }
    let mut ip = 1usize;
    let mut ctrl = source[0] & 31;
    let mut out = Vec::with_capacity(max_output);
    loop {
        if ctrl >= 32 {
            let mut len = ((ctrl >> 5) - 1) as usize;
            let mut offset = ((ctrl & 31) as usize) << 8;
            let mut reference = out
                .len()
                .checked_sub(offset + 1)
                .ok_or_else(|| Error::format("invalid FastLZ backward reference"))?;
            if level == 1 {
                if len == 6 {
                    len += *source
                        .get(ip)
                        .ok_or_else(|| Error::format("truncated FastLZ length"))?
                        as usize;
                    ip += 1;
                }
                reference = reference
                    .checked_sub(
                        *source
                            .get(ip)
                            .ok_or_else(|| Error::format("truncated FastLZ offset"))?
                            as usize,
                    )
                    .ok_or_else(|| Error::format("invalid FastLZ backward reference"))?;
                ip += 1;
                len += 3;
            } else {
                if len == 6 {
                    loop {
                        let code = *source
                            .get(ip)
                            .ok_or_else(|| Error::format("truncated FastLZ extended length"))?;
                        ip += 1;
                        len += code as usize;
                        if code != 255 {
                            break;
                        }
                    }
                }
                let code = *source
                    .get(ip)
                    .ok_or_else(|| Error::format("truncated FastLZ offset"))?
                    as usize;
                ip += 1;
                reference = reference
                    .checked_sub(code)
                    .ok_or_else(|| Error::format("invalid FastLZ backward reference"))?;
                len += 3;
                if code == 255 && offset == 31 << 8 {
                    let hi = *source
                        .get(ip)
                        .ok_or_else(|| Error::format("truncated FastLZ far-distance match"))?
                        as usize;
                    let lo = *source
                        .get(ip + 1)
                        .ok_or_else(|| Error::format("truncated FastLZ far-distance match"))?
                        as usize;
                    ip += 2;
                    offset = (hi << 8) + lo;
                    reference = out
                        .len()
                        .checked_sub(offset + 8192)
                        .ok_or_else(|| Error::format("invalid FastLZ far-distance reference"))?;
                }
            }
            if out.len() + len > max_output {
                return Err(Error::format("FastLZ block expands beyond declared size"));
            }
            for _ in 0..len {
                let byte = *out
                    .get(reference)
                    .ok_or_else(|| Error::format("invalid FastLZ match"))?;
                out.push(byte);
                reference += 1;
            }
        } else {
            let count = ctrl as usize + 1;
            let end = ip
                .checked_add(count)
                .ok_or_else(|| Error::format("FastLZ size overflow"))?;
            if end > source.len() || out.len() + count > max_output {
                return Err(Error::format("truncated or oversized FastLZ literal"));
            }
            out.extend_from_slice(&source[ip..end]);
            ip = end;
        }
        let done = if level == 1 {
            ip + 2 > source.len()
        } else {
            ip >= source.len()
        };
        if done {
            break;
        }
        ctrl = source[ip];
        ip += 1;
    }
    Ok(out)
}

pub fn literal_block(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + data.len() / 32 + 1);
    for chunk in data.chunks(32) {
        out.push((chunk.len() - 1) as u8);
        out.extend_from_slice(chunk);
    }
    out
}

pub fn compress(data: &[u8]) -> Vec<u8> {
    if data.is_empty() {
        return Vec::new();
    }
    use std::collections::HashMap;
    let mut out = Vec::new();
    let mut literals = Vec::new();
    let mut positions: HashMap<[u8; 3], Vec<usize>> = HashMap::new();
    let remember = |pos: usize, positions: &mut HashMap<[u8; 3], Vec<usize>>| {
        if pos + 2 >= data.len() {
            return;
        }
        let key = [data[pos], data[pos + 1], data[pos + 2]];
        let bucket = positions.entry(key).or_default();
        bucket.push(pos);
        if bucket.len() > 64 {
            bucket.drain(..bucket.len() - 64);
        }
    };
    let flush = |out: &mut Vec<u8>, literals: &mut Vec<u8>| {
        for chunk in literals.chunks(32) {
            out.push((chunk.len() - 1) as u8);
            out.extend_from_slice(chunk);
        }
        literals.clear();
    };
    let mut i = 0;
    while i < data.len() {
        let (mut best_len, mut best_distance) = (0usize, 0usize);
        if i + 2 < data.len() {
            let key = [data[i], data[i + 1], data[i + 2]];
            let max_len = 264.min(data.len() - i);
            if let Some(candidates) = positions.get(&key) {
                for &candidate in candidates.iter().rev() {
                    let distance = i - candidate;
                    if distance > 8192 {
                        break;
                    }
                    let mut len = 3;
                    while len < max_len && data[candidate + len] == data[i + len] {
                        len += 1;
                    }
                    if len > best_len {
                        best_len = len;
                        best_distance = distance;
                        if len == max_len {
                            break;
                        }
                    }
                }
            }
        }
        if best_len >= 3 && (!out.is_empty() || !literals.is_empty()) {
            flush(&mut out, &mut literals);
            let distance = best_distance - 1;
            if best_len <= 8 {
                out.push((((best_len - 2) << 5) | (distance >> 8)) as u8);
            } else {
                out.push(((7 << 5) | (distance >> 8)) as u8);
                out.push((best_len - 9) as u8);
            }
            out.push(distance as u8);
            for pos in i..i + best_len {
                remember(pos, &mut positions);
            }
            i += best_len;
        } else {
            literals.push(data[i]);
            remember(i, &mut positions);
            i += 1;
            if literals.len() == 32 {
                flush(&mut out, &mut literals);
            }
        }
    }
    flush(&mut out, &mut literals);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn round_trip_patterns() {
        for data in [b"hello".as_slice(), &[7; 4096], b"abcabcabcabc0123456789"] {
            let packed = compress(data);
            assert_eq!(decompress(&packed, data.len()).unwrap(), data);
        }
    }
}

//! Little-endian binary reading helpers with bounds checking.
//!
//! Every read returns a `Result` so malformed / truncated files never panic.

use std::fmt;

#[derive(Debug, Clone)]
pub struct FormatError(pub String);

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for FormatError {}

impl From<std::io::Error> for FormatError {
    fn from(e: std::io::Error) -> Self {
        FormatError(e.to_string())
    }
}

pub type FResult<T> = Result<T, FormatError>;

pub fn err<T>(msg: impl Into<String>) -> FResult<T> {
    Err(FormatError(msg.into()))
}

/// A bounds-checked view over a byte buffer.
#[derive(Clone, Copy)]
pub struct Bin<'a> {
    pub data: &'a [u8],
}

impl<'a> Bin<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    fn slice(&self, off: usize, n: usize) -> FResult<&'a [u8]> {
        off.checked_add(n)
            .and_then(|end| self.data.get(off..end))
            .ok_or_else(|| FormatError(format!("read of {n} bytes at offset {off} is out of bounds (file is {} bytes)", self.data.len())))
    }

    pub fn bytes(&self, off: usize, n: usize) -> FResult<&'a [u8]> {
        self.slice(off, n)
    }

    pub fn u8(&self, off: usize) -> FResult<u8> {
        Ok(self.slice(off, 1)?[0])
    }

    pub fn i8(&self, off: usize) -> FResult<i8> {
        Ok(self.slice(off, 1)?[0] as i8)
    }

    pub fn u16(&self, off: usize) -> FResult<u16> {
        let b = self.slice(off, 2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    pub fn i16(&self, off: usize) -> FResult<i16> {
        Ok(self.u16(off)? as i16)
    }

    pub fn u32(&self, off: usize) -> FResult<u32> {
        let b = self.slice(off, 4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn i32(&self, off: usize) -> FResult<i32> {
        Ok(self.u32(off)? as i32)
    }

    pub fn u64(&self, off: usize) -> FResult<u64> {
        let b = self.slice(off, 8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_le_bytes(a))
    }

    pub fn f32(&self, off: usize) -> FResult<f32> {
        Ok(f32::from_bits(self.u32(off)?))
    }

    pub fn vec3(&self, off: usize) -> FResult<[f32; 3]> {
        Ok([self.f32(off)?, self.f32(off + 4)?, self.f32(off + 8)?])
    }

    pub fn vec4(&self, off: usize) -> FResult<[f32; 4]> {
        Ok([self.f32(off)?, self.f32(off + 4)?, self.f32(off + 8)?, self.f32(off + 12)?])
    }

    /// matrix3x4_t (row-major, 3 rows of 4 floats)
    pub fn mat3x4(&self, off: usize) -> FResult<[[f32; 4]; 3]> {
        let mut m = [[0.0; 4]; 3];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, v) in row.iter_mut().enumerate() {
                *v = self.f32(off + (r * 4 + c) * 4)?;
            }
        }
        Ok(m)
    }

    /// Null terminated string starting at `off`. Invalid offsets give an empty string.
    pub fn cstr(&self, off: usize) -> String {
        if off >= self.data.len() {
            return String::new();
        }
        let rest = &self.data[off..];
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        String::from_utf8_lossy(&rest[..end]).into_owned()
    }

    /// Fixed size char array.
    pub fn fixed_str(&self, off: usize, n: usize) -> FResult<String> {
        let b = self.slice(off, n)?;
        let end = b.iter().position(|&c| c == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&b[..end]).into_owned())
    }

    /// String referenced by a relative `int` offset stored at `base + field`.
    pub fn rel_str(&self, base: usize, field_off: usize) -> FResult<String> {
        let rel = self.i32(base + field_off)?;
        if rel == 0 {
            return Ok(String::new());
        }
        let abs = base as i64 + rel as i64;
        if abs < 0 {
            return Ok(String::new());
        }
        Ok(self.cstr(abs as usize))
    }
}

pub fn add_off(base: usize, rel: i32) -> FResult<usize> {
    let v = base as i64 + rel as i64;
    if v < 0 {
        return err(format!("negative offset {base} + {rel}"));
    }
    Ok(v as usize)
}

/// IEEE 754 half -> f32
pub fn f16_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1f) as u32;
    let mant = (h & 0x3ff) as u32;
    let bits = if exp == 0 {
        if mant == 0 {
            sign << 31
        } else {
            // subnormal
            let mut e: i32 = -14;
            let mut m = mant;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x3ff;
            (sign << 31) | (((e + 127) as u32) << 23) | (m << 13)
        }
    } else if exp == 31 {
        // Source's float16 treats max exponent as a regular number (no inf/nan).
        (sign << 31) | ((31 - 15 + 127) << 23) | (mant << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (mant << 13)
    };
    f32::from_bits(bits)
}

/// Simple growable little-endian writer.
#[derive(Default)]
pub struct BinWriter {
    pub buf: Vec<u8>,
}

impl BinWriter {
    pub fn u8(&mut self, v: u8) {
        self.buf.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.buf.extend_from_slice(&v.to_le_bytes());
    }
    pub fn bytes(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn half() {
        assert_eq!(f16_to_f32(0x3c00), 1.0);
        assert_eq!(f16_to_f32(0xc000), -2.0);
        assert_eq!(f16_to_f32(0x0000), 0.0);
        assert!((f16_to_f32(0x3555) - 0.333).abs() < 0.001);
    }
}

//! VPK (Valve Pak) v1/v2 directory reader.
//!
//! Only the directory (`*_dir.vpk`) is parsed up front; file data is read on
//! demand from the numbered archives (`*_000.vpk`, ...), so mounting a large
//! game costs little memory.

use super::bin::{Bin, FResult, err};
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct VpkEntry {
    pub crc: u32,
    pub preload: Vec<u8>,
    pub archive_index: u16,
    pub offset: u32,
    pub length: u32,
}

#[derive(Debug)]
pub struct Vpk {
    /// Path of the `_dir.vpk` file.
    pub dir_path: PathBuf,
    /// Base name used for archives: `<base>_NNN.vpk`.
    base: String,
    /// Offset of embedded data (archive index 0x7fff) in the dir file.
    embedded_data_offset: u64,
    /// lowercase forward-slash path -> entry
    pub entries: HashMap<String, VpkEntry>,
}

impl Vpk {
    /// Open a vpk. Accepts either `foo_dir.vpk` or `foo.vpk` (which is mapped to `foo_dir.vpk`
    /// when that exists, as the engine does).
    pub fn open(path: &Path) -> FResult<Vpk> {
        let mut dir_path = path.to_path_buf();
        let fname = path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if !fname.to_ascii_lowercase().ends_with("_dir.vpk") {
            let stem = fname.trim_end_matches(".vpk").trim_end_matches(".VPK");
            let alt = path.with_file_name(format!("{stem}_dir.vpk"));
            if alt.exists() {
                dir_path = alt;
            }
        }
        let data = std::fs::read(&dir_path)?;
        let b = Bin::new(&data);
        let sig = b.u32(0)?;
        if sig != 0x55aa1234 {
            return err(format!("{} is not a VPK (bad signature)", dir_path.display()));
        }
        let version = b.u32(4)?;
        let tree_size = b.u32(8)? as usize;
        let header_size = match version {
            1 => 12,
            2 => 28,
            v => return err(format!("unsupported VPK version {v}")),
        };
        let mut p = header_size;
        let tree_end = header_size + tree_size;
        let mut entries = HashMap::new();
        let read_str = |p: &mut usize| -> FResult<String> {
            let s = b.cstr(*p);
            *p += s.len() + 1;
            if *p > data.len() {
                return err("VPK directory truncated");
            }
            Ok(s)
        };
        loop {
            let ext = read_str(&mut p)?;
            if ext.is_empty() {
                break;
            }
            loop {
                let dir = read_str(&mut p)?;
                if dir.is_empty() {
                    break;
                }
                loop {
                    let name = read_str(&mut p)?;
                    if name.is_empty() {
                        break;
                    }
                    let crc = b.u32(p)?;
                    let preload_len = b.u16(p + 4)? as usize;
                    let archive_index = b.u16(p + 6)?;
                    let offset = b.u32(p + 8)?;
                    let length = b.u32(p + 12)?;
                    // terminator 0xffff at p+16
                    p += 18;
                    let preload = b.bytes(p, preload_len)?.to_vec();
                    p += preload_len;
                    let mut full = String::new();
                    if dir.trim() != "" {
                        full.push_str(dir.trim());
                        full.push('/');
                    }
                    full.push_str(&name);
                    if ext.trim() != "" {
                        full.push('.');
                        full.push_str(ext.trim());
                    }
                    entries.insert(normalize(&full), VpkEntry { crc, preload, archive_index, offset, length });
                }
            }
        }
        let dir_name = dir_path.file_name().unwrap().to_string_lossy().to_string();
        let base = dir_name[..dir_name.len() - "_dir.vpk".len().min(dir_name.len())].to_string();
        Ok(Vpk { dir_path, base, embedded_data_offset: tree_end as u64, entries })
    }

    pub fn display_name(&self) -> String {
        self.dir_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.entries.contains_key(&normalize(path))
    }

    pub fn read(&self, path: &str) -> FResult<Vec<u8>> {
        let Some(e) = self.entries.get(&normalize(path)) else {
            return err(format!("{path} not found in {}", self.display_name()));
        };
        self.read_entry(e)
    }

    pub fn read_entry(&self, e: &VpkEntry) -> FResult<Vec<u8>> {
        let mut out = Vec::with_capacity(e.preload.len() + e.length as usize);
        out.extend_from_slice(&e.preload);
        if e.length > 0 {
            let (file_path, off) = if e.archive_index == 0x7fff {
                (self.dir_path.clone(), self.embedded_data_offset + e.offset as u64)
            } else {
                let parent = self.dir_path.parent().unwrap_or(Path::new("."));
                (parent.join(format!("{}_{:03}.vpk", self.base, e.archive_index)), e.offset as u64)
            };
            let mut f = File::open(&file_path)?;
            f.seek(SeekFrom::Start(off))?;
            let start = out.len();
            out.resize(start + e.length as usize, 0);
            f.read_exact(&mut out[start..])?;
        }
        Ok(out)
    }
}

/// Lowercase, forward slashes, no leading `./` or `/`.
pub fn normalize(p: &str) -> String {
    let mut s = p.replace('\\', "/").to_ascii_lowercase();
    while s.starts_with("./") {
        s.drain(..2);
    }
    while s.starts_with('/') {
        s.remove(0);
    }
    while s.contains("//") {
        s = s.replace("//", "/");
    }
    s
}

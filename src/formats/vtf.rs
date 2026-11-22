//! VTF texture decoding (versions 7.0 - 7.5) to RGBA8.
//!
//! Supports the formats used by models: DXT1/3/5, the 8888/888/565/4444/5551
//! families, I8/IA88/A8 and RGBA16161616F (tonemapped).

use super::bin::{Bin, FResult, err, f16_to_f32};

pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fmt {
    Rgba8888,
    Abgr8888,
    Rgb888,
    Bgr888,
    Rgb565,
    I8,
    Ia88,
    P8,
    A8,
    Rgb888Blue,
    Bgr888Blue,
    Argb8888,
    Bgra8888,
    Dxt1,
    Dxt3,
    Dxt5,
    Bgrx8888,
    Bgr565,
    Bgrx5551,
    Bgra4444,
    Dxt1A,
    Bgra5551,
    Uv88,
    Uvwq8888,
    Rgba16f,
    Rgba16,
    Uvlx8888,
}

impl Fmt {
    fn from_i32(v: i32) -> Option<Fmt> {
        use Fmt::*;
        Some(match v {
            0 => Rgba8888,
            1 => Abgr8888,
            2 => Rgb888,
            3 => Bgr888,
            4 => Rgb565,
            5 => I8,
            6 => Ia88,
            7 => P8,
            8 => A8,
            9 => Rgb888Blue,
            10 => Bgr888Blue,
            11 => Argb8888,
            12 => Bgra8888,
            13 => Dxt1,
            14 => Dxt3,
            15 => Dxt5,
            16 => Bgrx8888,
            17 => Bgr565,
            18 => Bgrx5551,
            19 => Bgra4444,
            20 => Dxt1A,
            21 => Bgra5551,
            22 => Uv88,
            23 => Uvwq8888,
            24 => Rgba16f,
            25 => Rgba16,
            26 => Uvlx8888,
            _ => return None,
        })
    }

    fn size(self, w: u32, h: u32) -> usize {
        use Fmt::*;
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        match self {
            Dxt1 | Dxt1A => w.div_ceil(4) * h.div_ceil(4) * 8,
            Dxt3 | Dxt5 => w.div_ceil(4) * h.div_ceil(4) * 16,
            Rgba8888 | Abgr8888 | Argb8888 | Bgra8888 | Bgrx8888 | Uvwq8888 | Uvlx8888 => w * h * 4,
            Rgb888 | Bgr888 | Rgb888Blue | Bgr888Blue => w * h * 3,
            Rgb565 | Bgr565 | Bgrx5551 | Bgra4444 | Bgra5551 | Ia88 | Uv88 => w * h * 2,
            I8 | P8 | A8 => w * h,
            Rgba16f | Rgba16 => w * h * 8,
        }
    }
}

pub struct VtfInfo {
    pub width: u32,
    pub height: u32,
    pub flags: u32,
    pub frames: u32,
    pub mips: u32,
    pub format: i32,
    pub version: (u32, u32),
}

pub fn info(data: &[u8]) -> FResult<VtfInfo> {
    let b = Bin::new(data);
    if b.bytes(0, 4)? != b"VTF\0" {
        return err("not a VTF file");
    }
    Ok(VtfInfo {
        version: (b.u32(4)?, b.u32(8)?),
        width: b.u16(16)? as u32,
        height: b.u16(18)? as u32,
        flags: b.u32(20)?,
        frames: b.u16(24)?.max(1) as u32,
        format: b.i32(52)?,
        mips: b.u8(56)?.max(1) as u32,
    })
}

/// Decode the largest mip whose size is <= `max_dim` (frame 0, face 0, slice 0).
pub fn decode(data: &[u8], max_dim: u32) -> FResult<RgbaImage> {
    let b = Bin::new(data);
    let inf = info(data)?;
    let (vmaj, vmin) = inf.version;
    if vmaj != 7 {
        return err(format!("unsupported VTF version {vmaj}.{vmin}"));
    }
    let header_size = b.u32(12)? as usize;
    let fmt = Fmt::from_i32(inf.format).ok_or_else(|| super::bin::FormatError(format!("unsupported VTF image format {}", inf.format)))?;
    let low_fmt = b.i32(57)?;
    let low_w = b.u8(61)? as u32;
    let low_h = b.u8(62)? as u32;
    let depth = if vmin >= 2 { b.u16(63)?.max(1) as u32 } else { 1 };
    let flags = inf.flags;
    let envmap = flags & 0x4000 != 0;
    let first_frame = b.u16(26)?;
    let faces: u32 = if envmap { if vmin < 5 && first_frame != 0xffff { 7 } else { 6 } } else { 1 };

    // locate high-res data
    let hi_start: usize = if vmin >= 3 {
        let num_res = b.u32(68)? as usize;
        let mut found = None;
        for i in 0..num_res.min(32) {
            let off = 80 + i * 8;
            let tag = b.bytes(off, 3)?;
            if tag == [0x30, 0, 0] {
                found = Some(b.u32(off + 4)? as usize);
            }
        }
        found.ok_or_else(|| super::bin::FormatError("VTF has no high-res image resource".into()))?
    } else {
        let low_size = if low_fmt >= 0 { Fmt::from_i32(low_fmt).map(|f| f.size(low_w, low_h)).unwrap_or(0) } else { 0 };
        header_size + low_size
    };

    // pick mip
    let mut mip = 0;
    while mip + 1 < inf.mips && ((inf.width >> mip) > max_dim || (inf.height >> mip) > max_dim) {
        mip += 1;
    }
    // mips are stored smallest -> largest
    let mut off = hi_start;
    for m in (mip + 1..inf.mips).rev() {
        let w = (inf.width >> m).max(1);
        let h = (inf.height >> m).max(1);
        let d = (depth >> m).max(1);
        off += fmt.size(w, h) * (inf.frames * faces * d) as usize;
    }
    let w = (inf.width >> mip).max(1);
    let h = (inf.height >> mip).max(1);
    let size = fmt.size(w, h);
    let src = b.bytes(off, size)?;
    let pixels = decode_surface(fmt, src, w, h)?;
    Ok(RgbaImage { width: w, height: h, pixels })
}

fn rgb565(c: u16) -> [u8; 3] {
    let r = ((c >> 11) & 31) as u32;
    let g = ((c >> 5) & 63) as u32;
    let b = (c & 31) as u32;
    [((r * 255 + 15) / 31) as u8, ((g * 255 + 31) / 63) as u8, ((b * 255 + 15) / 31) as u8]
}

fn dxt_color_block(blk: &[u8], out: &mut [[u8; 4]; 16], dxt1: bool) {
    let c0 = u16::from_le_bytes([blk[0], blk[1]]);
    let c1 = u16::from_le_bytes([blk[2], blk[3]]);
    let a = rgb565(c0);
    let b = rgb565(c1);
    let mut pal = [[0u8; 4]; 4];
    pal[0] = [a[0], a[1], a[2], 255];
    pal[1] = [b[0], b[1], b[2], 255];
    if c0 > c1 || !dxt1 {
        for i in 0..3 {
            pal[2][i] = ((2 * a[i] as u32 + b[i] as u32) / 3) as u8;
            pal[3][i] = ((a[i] as u32 + 2 * b[i] as u32) / 3) as u8;
        }
        pal[2][3] = 255;
        pal[3][3] = 255;
    } else {
        for i in 0..3 {
            pal[2][i] = ((a[i] as u32 + b[i] as u32) / 2) as u8;
        }
        pal[2][3] = 255;
        pal[3] = [0, 0, 0, 0];
    }
    let bits = u32::from_le_bytes([blk[4], blk[5], blk[6], blk[7]]);
    for (i, px) in out.iter_mut().enumerate() {
        *px = pal[((bits >> (i * 2)) & 3) as usize];
    }
}

fn dxt5_alpha(blk: &[u8]) -> [u8; 16] {
    let a0 = blk[0] as u32;
    let a1 = blk[1] as u32;
    let mut pal = [0u8; 8];
    pal[0] = a0 as u8;
    pal[1] = a1 as u8;
    if a0 > a1 {
        for i in 1..7 {
            pal[i + 1] = (((7 - i as u32) * a0 + i as u32 * a1) / 7) as u8;
        }
    } else {
        for i in 1..5 {
            pal[i + 1] = (((5 - i as u32) * a0 + i as u32 * a1) / 5) as u8;
        }
        pal[6] = 0;
        pal[7] = 255;
    }
    let mut bits: u64 = 0;
    for i in 0..6 {
        bits |= (blk[2 + i] as u64) << (8 * i);
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = pal[((bits >> (3 * i)) & 7) as usize];
    }
    out
}

fn decode_surface(fmt: Fmt, src: &[u8], w: u32, h: u32) -> FResult<Vec<u8>> {
    use Fmt::*;
    let (wu, hu) = (w as usize, h as usize);
    let mut out = vec![0u8; wu * hu * 4];
    match fmt {
        Dxt1 | Dxt1A | Dxt3 | Dxt5 => {
            let bw = wu.div_ceil(4);
            let bh = hu.div_ceil(4);
            let bsize = if matches!(fmt, Dxt1 | Dxt1A) { 8 } else { 16 };
            for by in 0..bh {
                for bx in 0..bw {
                    let blk = &src[(by * bw + bx) * bsize..][..bsize];
                    let mut px = [[0u8; 4]; 16];
                    match fmt {
                        Dxt1 | Dxt1A => dxt_color_block(blk, &mut px, true),
                        Dxt3 => {
                            dxt_color_block(&blk[8..], &mut px, false);
                            for i in 0..16 {
                                let nib = (blk[i / 2] >> ((i % 2) * 4)) & 0xf;
                                px[i][3] = nib * 17;
                            }
                        }
                        _ => {
                            dxt_color_block(&blk[8..], &mut px, false);
                            let a = dxt5_alpha(&blk[..8]);
                            for i in 0..16 {
                                px[i][3] = a[i];
                            }
                        }
                    }
                    for py in 0..4 {
                        for pxx in 0..4 {
                            let x = bx * 4 + pxx;
                            let y = by * 4 + py;
                            if x < wu && y < hu {
                                let o = (y * wu + x) * 4;
                                out[o..o + 4].copy_from_slice(&px[py * 4 + pxx]);
                            }
                        }
                    }
                }
            }
        }
        _ => {
            for i in 0..wu * hu {
                let o = i * 4;
                let px: [u8; 4] = match fmt {
                    Rgba8888 | Uvwq8888 | Uvlx8888 => [src[i * 4], src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3]],
                    Abgr8888 => [src[i * 4 + 3], src[i * 4 + 2], src[i * 4 + 1], src[i * 4]],
                    Argb8888 => [src[i * 4 + 1], src[i * 4 + 2], src[i * 4 + 3], src[i * 4]],
                    Bgra8888 => [src[i * 4 + 2], src[i * 4 + 1], src[i * 4], src[i * 4 + 3]],
                    Bgrx8888 => [src[i * 4 + 2], src[i * 4 + 1], src[i * 4], 255],
                    Rgb888 => [src[i * 3], src[i * 3 + 1], src[i * 3 + 2], 255],
                    Bgr888 => [src[i * 3 + 2], src[i * 3 + 1], src[i * 3], 255],
                    Rgb888Blue | Bgr888Blue => {
                        let (r, g, b) = if fmt == Rgb888Blue { (src[i * 3], src[i * 3 + 1], src[i * 3 + 2]) } else { (src[i * 3 + 2], src[i * 3 + 1], src[i * 3]) };
                        let a = if r == 0 && g == 0 && b == 255 { 0 } else { 255 };
                        [r, g, b, a]
                    }
                    Rgb565 => {
                        let c = rgb565(u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]));
                        [c[0], c[1], c[2], 255]
                    }
                    Bgr565 => {
                        let c = rgb565(u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]));
                        [c[2], c[1], c[0], 255]
                    }
                    Bgra4444 => {
                        let v = u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]);
                        let b = (v & 0xf) as u8 * 17;
                        let g = ((v >> 4) & 0xf) as u8 * 17;
                        let r = ((v >> 8) & 0xf) as u8 * 17;
                        let a = ((v >> 12) & 0xf) as u8 * 17;
                        [r, g, b, a]
                    }
                    Bgra5551 | Bgrx5551 => {
                        let v = u16::from_le_bytes([src[i * 2], src[i * 2 + 1]]);
                        let b = ((v & 31) as u32 * 255 / 31) as u8;
                        let g = (((v >> 5) & 31) as u32 * 255 / 31) as u8;
                        let r = (((v >> 10) & 31) as u32 * 255 / 31) as u8;
                        let a = if fmt == Bgrx5551 || v >> 15 != 0 { 255 } else { 0 };
                        [r, g, b, a]
                    }
                    I8 | P8 => [src[i], src[i], src[i], 255],
                    A8 => [255, 255, 255, src[i]],
                    Ia88 => [src[i * 2], src[i * 2], src[i * 2], src[i * 2 + 1]],
                    Uv88 => [src[i * 2], src[i * 2 + 1], 255, 255],
                    Rgba16f => {
                        let f = |k: usize| f16_to_f32(u16::from_le_bytes([src[i * 8 + k * 2], src[i * 8 + k * 2 + 1]]));
                        let tm = |v: f32| ((v / (1.0 + v)).max(0.0).powf(1.0 / 2.2) * 255.0).clamp(0.0, 255.0) as u8;
                        [tm(f(0)), tm(f(1)), tm(f(2)), (f(3).clamp(0.0, 1.0) * 255.0) as u8]
                    }
                    Rgba16 => {
                        let f = |k: usize| u16::from_le_bytes([src[i * 8 + k * 2], src[i * 8 + k * 2 + 1]]);
                        [(f(0) >> 8) as u8, (f(1) >> 8) as u8, (f(2) >> 8) as u8, (f(3) >> 8) as u8]
                    }
                    Dxt1 | Dxt1A | Dxt3 | Dxt5 => unreachable!(),
                };
                out[o..o + 4].copy_from_slice(&px);
            }
        }
    }
    Ok(out)
}

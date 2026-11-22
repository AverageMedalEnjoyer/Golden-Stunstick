//! .phy collision model reader (IVP compact surfaces + the trailing KeyValues text).

use super::bin::{Bin, FResult, err};
use super::keyvalues::{self, Kv};
use glam::Vec3;

/// One convex piece of a solid: triangles as point triplets, in IVP-converted
/// Source units, in the space of the solid's bone.
#[derive(Clone, Debug, Default)]
pub struct Convex {
    pub tris: Vec<[Vec3; 3]>,
}

#[derive(Clone, Debug, Default)]
pub struct Solid {
    pub convexes: Vec<Convex>,
}

#[derive(Clone, Debug, Default)]
pub struct Phy {
    pub checksum: i32,
    pub solids: Vec<Solid>,
    pub text: String,
    pub kv: Vec<Kv>,
}

const IVP_TO_SOURCE: f32 = 1.0 / 0.0254;

fn ivp_point(b: &Bin, p: usize) -> FResult<Vec3> {
    let x = b.f32(p)?;
    let y = b.f32(p + 4)?;
    let z = b.f32(p + 8)?;
    Ok(Vec3::new(x, z, -y) * IVP_TO_SOURCE)
}

impl Phy {
    pub fn parse(data: &[u8]) -> FResult<Phy> {
        let b = Bin::new(data);
        let header_size = b.i32(0)? as usize;
        if header_size != 16 {
            return err("unexpected PHY header size");
        }
        let solid_count = b.i32(8)?.max(0) as usize;
        let checksum = b.i32(12)?;
        let mut p = header_size;
        let mut solids = Vec::new();
        for _ in 0..solid_count.min(1024) {
            let size = b.i32(p)?.max(0) as usize;
            let start = p + 4;
            let end = start + size;
            let mut s = start;
            if b.bytes(s, 4)? == b"VPHY" {
                s += 28;
            }
            let surface = s;
            let ledgetree_root = surface + b.i32(surface + 32)?.max(0) as usize;
            let mut ledge = surface + 48;
            let mut min_points = end;
            let mut solid = Solid::default();
            let limit = ledgetree_root.min(end);
            let mut guard = 0;
            while ledge + 16 <= limit && ledge < min_points && guard < 100_000 {
                guard += 1;
                let point_off = b.i32(ledge)?;
                let ntri = b.i16(ledge + 12)?.max(0) as usize;
                let points = (ledge as i64 + point_off as i64) as usize;
                if point_off > 0 {
                    min_points = min_points.min(points);
                }
                let mut convex = Convex::default();
                for t in 0..ntri {
                    let tp = ledge + 16 + t * 16;
                    let mut tri = [Vec3::ZERO; 3];
                    for (e, v) in tri.iter_mut().enumerate() {
                        let edge = b.u32(tp + 4 + e * 4)?;
                        let idx = (edge & 0xffff) as usize;
                        *v = ivp_point(&b, points + idx * 16)?;
                    }
                    convex.tris.push(tri);
                }
                solid.convexes.push(convex);
                ledge += 16 + ntri * 16;
            }
            solids.push(solid);
            p = end;
        }
        let text_bytes = data.get(p..).unwrap_or(&[]);
        let text = String::from_utf8_lossy(text_bytes).trim_end_matches('\0').to_string();
        let kv = keyvalues::parse(&text);
        Ok(Phy { checksum, solids, text, kv })
    }

    /// `solid` blocks of the text section, as (index, name, block).
    pub fn solid_infos(&self) -> Vec<(usize, String, &Kv)> {
        self.kv
            .iter()
            .filter(|k| k.key.eq_ignore_ascii_case("solid"))
            .map(|k| (k.get_str("index").and_then(|s| s.parse().ok()).unwrap_or(0), k.get_str("name").unwrap_or("").to_string(), k))
            .collect()
    }

    pub fn block(&self, key: &str) -> Option<&Kv> {
        self.kv.iter().find(|k| k.key.eq_ignore_ascii_case(key))
    }

    pub fn blocks<'a>(&'a self, key: &'a str) -> impl Iterator<Item = &'a Kv> + 'a {
        self.kv.iter().filter(move |k| k.key.eq_ignore_ascii_case(key))
    }
}

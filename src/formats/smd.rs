//! StudioMDL Data (.smd) reader and writer, plus VTA (vertex animation) writer.

use glam::Vec3;
use std::fmt::Write as _;

#[derive(Clone, Debug, Default)]
pub struct SmdNode {
    pub name: String,
    pub parent: i32,
}

#[derive(Clone, Debug, Default)]
pub struct SmdVert {
    pub bone: i32,
    pub pos: Vec3,
    pub normal: Vec3,
    pub uv: [f32; 2],
    pub links: Vec<(i32, f32)>,
}

#[derive(Clone, Debug, Default)]
pub struct SmdTri {
    pub material: String,
    pub verts: [SmdVert; 3],
}

/// Per-bone local transform in SMD convention: position + RadianEuler rotation.
pub type SmdPose = Vec<(Vec3, Vec3)>;

#[derive(Clone, Debug, Default)]
pub struct Smd {
    pub nodes: Vec<SmdNode>,
    /// frames[frame][bone]
    pub frames: Vec<SmdPose>,
    pub triangles: Vec<SmdTri>,
}

fn fmt_f(v: f32) -> String {
    let v = if v.abs() < 0.0000005 { 0.0 } else { v };
    let s = format!("{v:.6}");
    if s == "-0.000000" { "0.000000".into() } else { s }
}

fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut inq = false;
    for c in line.chars() {
        if c == '"' {
            inq = !inq;
            if !inq {
                out.push(std::mem::take(&mut cur));
            }
            continue;
        }
        if c.is_whitespace() && !inq {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(c);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

impl Smd {
    pub fn parse(text: &str) -> Result<Smd, String> {
        let mut smd = Smd::default();
        let mut lines = text.lines().enumerate().peekable();
        let pf = |s: Option<&String>, ln: usize| -> Result<f32, String> { s.and_then(|s| s.parse::<f32>().ok()).ok_or_else(|| format!("line {}: expected a number", ln + 1)) };
        while let Some((_, line)) = lines.next() {
            let l = line.trim();
            if l.is_empty() || l.starts_with("//") || l.starts_with('#') || l.starts_with(';') {
                continue;
            }
            let t = tokens(l);
            match t[0].as_str() {
                "version" => {}
                "nodes" => {
                    for (ln, line) in lines.by_ref() {
                        let t = tokens(line.trim());
                        if t.is_empty() {
                            continue;
                        }
                        if t[0] == "end" {
                            break;
                        }
                        if t.len() < 3 {
                            return Err(format!("line {}: bad node line", ln + 1));
                        }
                        let id: usize = t[0].parse().map_err(|_| format!("line {}: bad node index", ln + 1))?;
                        let parent: i32 = t[t.len() - 1].parse().map_err(|_| format!("line {}: bad parent index", ln + 1))?;
                        let name = t[1..t.len() - 1].join(" ");
                        if smd.nodes.len() <= id {
                            smd.nodes.resize(id + 1, SmdNode::default());
                        }
                        smd.nodes[id] = SmdNode { name, parent };
                    }
                }
                "skeleton" => {
                    let mut cur: Option<SmdPose> = None;
                    let nb = smd.nodes.len();
                    for (ln, line) in lines.by_ref() {
                        let t = tokens(line.trim());
                        if t.is_empty() {
                            continue;
                        }
                        if t[0] == "end" {
                            break;
                        }
                        if t[0] == "time" {
                            if let Some(p) = cur.take() {
                                smd.frames.push(p);
                            }
                            // frames inherit missing bones from the previous frame
                            let base = smd.frames.last().cloned().unwrap_or_else(|| vec![(Vec3::ZERO, Vec3::ZERO); nb]);
                            cur = Some(base);
                            continue;
                        }
                        let Some(pose) = cur.as_mut() else { return Err(format!("line {}: bone data before 'time'", ln + 1)) };
                        let id: usize = t[0].parse().map_err(|_| format!("line {}: bad bone index", ln + 1))?;
                        if t.len() < 7 {
                            return Err(format!("line {}: skeleton line needs 7 values", ln + 1));
                        }
                        let p = Vec3::new(pf(t.get(1), ln)?, pf(t.get(2), ln)?, pf(t.get(3), ln)?);
                        let r = Vec3::new(pf(t.get(4), ln)?, pf(t.get(5), ln)?, pf(t.get(6), ln)?);
                        if pose.len() <= id {
                            pose.resize(id + 1, (Vec3::ZERO, Vec3::ZERO));
                        }
                        pose[id] = (p, r);
                    }
                    if let Some(p) = cur.take() {
                        smd.frames.push(p);
                    }
                }
                "triangles" => {
                    loop {
                        let Some((_, mline)) = lines.next() else { break };
                        let m = mline.trim();
                        if m == "end" {
                            break;
                        }
                        if m.is_empty() {
                            continue;
                        }
                        let mut verts: [SmdVert; 3] = Default::default();
                        for v in verts.iter_mut() {
                            let Some((ln, vl)) = lines.next() else { return Err("unexpected end of file in triangles".into()) };
                            let t = tokens(vl.trim());
                            if t.len() < 9 {
                                return Err(format!("line {}: triangle vertex needs at least 9 values", ln + 1));
                            }
                            v.bone = t[0].parse().unwrap_or(0);
                            v.pos = Vec3::new(pf(t.get(1), ln)?, pf(t.get(2), ln)?, pf(t.get(3), ln)?);
                            v.normal = Vec3::new(pf(t.get(4), ln)?, pf(t.get(5), ln)?, pf(t.get(6), ln)?);
                            v.uv = [pf(t.get(7), ln)?, pf(t.get(8), ln)?];
                            if t.len() > 9 {
                                let n: usize = t[9].parse().unwrap_or(0);
                                for k in 0..n {
                                    let bi = t.get(10 + k * 2).and_then(|s| s.parse().ok()).unwrap_or(0);
                                    let w = t.get(11 + k * 2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                                    v.links.push((bi, w));
                                }
                            }
                        }
                        smd.triangles.push(SmdTri { material: m.to_string(), verts });
                    }
                }
                "vertexanimation" => {
                    for (_, line) in lines.by_ref() {
                        if line.trim() == "end" {
                            break;
                        }
                    }
                }
                _ => {}
            }
        }
        // make sure every frame has an entry for every bone
        let nb = smd.nodes.len();
        for f in &mut smd.frames {
            f.resize(nb, (Vec3::ZERO, Vec3::ZERO));
        }
        Ok(smd)
    }

    pub fn write(&self) -> String {
        let mut s = String::with_capacity(1024 + self.triangles.len() * 300);
        s.push_str("version 1\nnodes\n");
        for (i, n) in self.nodes.iter().enumerate() {
            let _ = writeln!(s, "  {i} \"{}\" {}", n.name, n.parent);
        }
        s.push_str("end\nskeleton\n");
        for (fi, f) in self.frames.iter().enumerate() {
            let _ = writeln!(s, "  time {fi}");
            for (bi, (p, r)) in f.iter().enumerate() {
                let _ = writeln!(s, "    {bi} {} {} {}  {} {} {}", fmt_f(p.x), fmt_f(p.y), fmt_f(p.z), fmt_f(r.x), fmt_f(r.y), fmt_f(r.z));
            }
        }
        s.push_str("end\n");
        if !self.triangles.is_empty() {
            s.push_str("triangles\n");
            for t in &self.triangles {
                s.push_str(&t.material);
                s.push('\n');
                for v in &t.verts {
                    let _ = write!(
                        s,
                        "  {} {} {} {}  {} {} {}  {} {}",
                        v.bone,
                        fmt_f(v.pos.x),
                        fmt_f(v.pos.y),
                        fmt_f(v.pos.z),
                        fmt_f(v.normal.x),
                        fmt_f(v.normal.y),
                        fmt_f(v.normal.z),
                        fmt_f(v.uv[0]),
                        fmt_f(v.uv[1])
                    );
                    if !v.links.is_empty() {
                        let _ = write!(s, " {}", v.links.len());
                        for (b, w) in &v.links {
                            let _ = write!(s, " {b} {}", fmt_f(*w));
                        }
                    }
                    s.push('\n');
                }
            }
            s.push_str("end\n");
        }
        s
    }
}

/// One shape key for a VTA file: list of (vertex index, new position, new normal).
pub struct VtaShape {
    pub verts: Vec<(usize, Vec3, Vec3)>,
}

/// Write a VTA (vertex animation) file. `base` are the vertices of the reference
/// mesh (position, normal); frame 0 is the base shape.
pub fn write_vta(nodes: &[SmdNode], base: &[(Vec3, Vec3)], shapes: &[VtaShape]) -> String {
    let mut s = String::new();
    s.push_str("version 1\nnodes\n");
    for (i, n) in nodes.iter().enumerate() {
        let _ = writeln!(s, "  {i} \"{}\" {}", n.name, n.parent);
    }
    s.push_str("end\nskeleton\n");
    for f in 0..=shapes.len() {
        let _ = writeln!(s, "  time {f}");
    }
    s.push_str("end\nvertexanimation\n  time 0\n");
    for (i, (p, n)) in base.iter().enumerate() {
        let _ = writeln!(s, "    {i} {} {} {}  {} {} {}", fmt_f(p.x), fmt_f(p.y), fmt_f(p.z), fmt_f(n.x), fmt_f(n.y), fmt_f(n.z));
    }
    for (k, sh) in shapes.iter().enumerate() {
        let _ = writeln!(s, "  time {}", k + 1);
        for (i, p, n) in &sh.verts {
            let _ = writeln!(s, "    {i} {} {} {}  {} {} {}", fmt_f(p.x), fmt_f(p.y), fmt_f(p.z), fmt_f(n.x), fmt_f(n.y), fmt_f(n.z));
        }
    }
    s.push_str("end\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let text = "version 1\nnodes\n  0 \"root bone\" -1\n  1 \"child\" 0\nend\nskeleton\n  time 0\n    0 0 0 0 0 0 0\n    1 1 2 3 0.1 0.2 0.3\n  time 1\n    1 4 5 6 0 0 0\nend\ntriangles\nmat\n  0 0 0 0 0 0 1 0 0 1 0 1.0\n  0 1 0 0 0 0 1 1 0\n  1 0 1 0 0 0 1 0 1 2 0 0.5 1 0.5\nend\n";
        let s = Smd::parse(text).unwrap();
        assert_eq!(s.nodes.len(), 2);
        assert_eq!(s.nodes[0].name, "root bone");
        assert_eq!(s.frames.len(), 2);
        assert_eq!(s.frames[1][0].0, Vec3::ZERO);
        assert_eq!(s.frames[1][1].0, Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(s.triangles.len(), 1);
        assert_eq!(s.triangles[0].verts[2].links.len(), 2);
        let s2 = Smd::parse(&s.write()).unwrap();
        assert_eq!(s2.frames.len(), 2);
        assert_eq!(s2.triangles.len(), 1);
    }
}

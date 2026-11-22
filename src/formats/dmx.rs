//! DMX (Valve DataModel) reader: binary encodings 1-5 and KeyValues2 text,
//! plus conversion of a DMX model (skeleton + meshes) into an `Smd`.

use super::smd::{Smd, SmdNode, SmdTri, SmdVert};
use glam::{Affine3A, Quat, Vec3};

#[derive(Clone, Debug)]
pub enum Val {
    Elem(Option<usize>),
    Int(i32),
    Float(f32),
    Bool(bool),
    Str(String),
    Bin(Vec<u8>),
    Time(f32),
    Color([u8; 4]),
    V2([f32; 2]),
    V3([f32; 3]),
    V4([f32; 4]),
    Mat([f32; 16]),
    Arr(Vec<Val>),
}

#[derive(Clone, Debug, Default)]
pub struct Element {
    pub ty: String,
    pub name: String,
    pub attrs: Vec<(String, Val)>,
}

impl Element {
    pub fn get(&self, k: &str) -> Option<&Val> {
        self.attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v)
    }
}

#[derive(Default, Debug)]
pub struct Dmx {
    pub elements: Vec<Element>,
    pub encoding: String,
    pub encoding_version: i32,
    pub format: String,
    pub format_version: i32,
}

struct Reader<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Reader<'a> {
    fn need(&self, n: usize) -> Result<(), String> {
        if self.p + n > self.b.len() { Err(format!("DMX truncated at byte {}", self.p)) } else { Ok(()) }
    }
    fn u8(&mut self) -> Result<u8, String> {
        self.need(1)?;
        self.p += 1;
        Ok(self.b[self.p - 1])
    }
    fn i16(&mut self) -> Result<i16, String> {
        self.need(2)?;
        let v = i16::from_le_bytes([self.b[self.p], self.b[self.p + 1]]);
        self.p += 2;
        Ok(v)
    }
    fn i32(&mut self) -> Result<i32, String> {
        self.need(4)?;
        let v = i32::from_le_bytes(self.b[self.p..self.p + 4].try_into().unwrap());
        self.p += 4;
        Ok(v)
    }
    fn f32(&mut self) -> Result<f32, String> {
        Ok(f32::from_bits(self.i32()? as u32))
    }
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.need(n)?;
        self.p += n;
        Ok(&self.b[self.p - n..self.p])
    }
    fn cstr(&mut self) -> Result<String, String> {
        let start = self.p;
        while self.p < self.b.len() && self.b[self.p] != 0 {
            self.p += 1;
        }
        let s = String::from_utf8_lossy(&self.b[start..self.p]).into_owned();
        self.p += 1;
        Ok(s)
    }
}

fn parse_header(text: &str) -> Option<(String, i32, String, i32)> {
    let h = text.lines().next()?;
    let t: Vec<&str> = h.split_whitespace().collect();
    let enc_i = t.iter().position(|w| *w == "encoding")?;
    let fmt_i = t.iter().position(|w| *w == "format")?;
    Some((t.get(enc_i + 1)?.to_string(), t.get(enc_i + 2)?.parse().ok()?, t.get(fmt_i + 1)?.to_string(), t.get(fmt_i + 2)?.parse().ok()?))
}

impl Dmx {
    pub fn parse(data: &[u8]) -> Result<Dmx, String> {
        let head_end = data.iter().position(|&c| c == b'\n').ok_or("not a DMX file (no header)")?;
        let header = String::from_utf8_lossy(&data[..head_end]).to_string();
        let (enc, ev, fmt, fv) = parse_header(&header).ok_or("not a DMX file (bad header)")?;
        let mut d = Dmx { encoding: enc.clone(), encoding_version: ev, format: fmt, format_version: fv, ..Default::default() };
        if enc == "binary" {
            d.parse_binary(data, head_end + 1)?;
        } else if enc.starts_with("keyvalues2") {
            d.parse_kv2(&String::from_utf8_lossy(&data[head_end + 1..]))?;
        } else {
            return Err(format!("unsupported DMX encoding '{enc}'"));
        }
        if d.elements.is_empty() {
            return Err("DMX file has no elements".into());
        }
        Ok(d)
    }

    fn parse_binary(&mut self, data: &[u8], start: usize) -> Result<(), String> {
        let v = self.encoding_version;
        let mut r = Reader { b: data, p: start };
        // header is terminated by "\n\0"
        if r.p < data.len() && data[r.p] == 0 {
            r.p += 1;
        }
        let mut table: Vec<String> = Vec::new();
        if v >= 2 {
            let n = if v >= 4 { r.i32()? } else { r.i16()? as i32 };
            for _ in 0..n.max(0) {
                table.push(r.cstr()?);
            }
        }
        let tstr = |r: &mut Reader, table: &[String]| -> Result<String, String> {
            if v >= 2 {
                let i = if v >= 5 { r.i32()? } else { r.i16()? as i32 };
                table.get(i.max(0) as usize).cloned().ok_or_else(|| format!("bad DMX string index {i}"))
            } else {
                r.cstr()
            }
        };
        let n = r.i32()?;
        if !(0..=10_000_000).contains(&n) {
            return Err("corrupt DMX element count".into());
        }
        for _ in 0..n {
            let ty = tstr(&mut r, &table)?;
            let name = if v >= 4 { tstr(&mut r, &table)? } else { r.cstr()? };
            r.bytes(16)?; // GUID
            self.elements.push(Element { ty, name, attrs: Vec::new() });
        }
        for e in 0..n as usize {
            let na = r.i32()?;
            for _ in 0..na.max(0) {
                let name = tstr(&mut r, &table)?;
                let ty = r.u8()?;
                let val = read_value(&mut r, ty, v, &table, false)?;
                self.elements[e].attrs.push((name, val));
            }
        }
        Ok(())
    }

    fn parse_kv2(&mut self, text: &str) -> Result<(), String> {
        let toks = kv2_tokens(text);
        let mut ids: Vec<(String, usize)> = Vec::new();
        let mut refs: Vec<(usize, Vec<usize>, String)> = Vec::new(); // (element, attr path, id) fixups
        let mut pos = 0;
        while pos < toks.len() {
            let ty = toks[pos].clone();
            pos += 1;
            if toks.get(pos).map(|s| s.as_str()) != Some("{") {
                return Err(format!("KeyValues2: expected '{{' after element type {ty}"));
            }
            pos += 1;
            self.kv2_element(&toks, &mut pos, ty, &mut ids, &mut refs)?;
        }
        for (e, path, id) in refs {
            let target = ids.iter().find(|(i, _)| *i == id).map(|(_, x)| *x);
            let attr = &mut self.elements[e].attrs[path[0]].1;
            let slot = if path.len() > 1 {
                match attr {
                    Val::Arr(a) => &mut a[path[1]],
                    _ => continue,
                }
            } else {
                attr
            };
            *slot = Val::Elem(target);
        }
        Ok(())
    }

    fn kv2_element(&mut self, t: &[String], pos: &mut usize, ty: String, ids: &mut Vec<(String, usize)>, refs: &mut Vec<(usize, Vec<usize>, String)>) -> Result<usize, String> {
        let me = self.elements.len();
        self.elements.push(Element { ty, ..Default::default() });
        while *pos < t.len() && t[*pos] != "}" {
            let name = t[*pos].clone();
            let ty = t.get(*pos + 1).cloned().ok_or("KeyValues2: truncated")?;
            *pos += 2;
            if t.get(*pos).map(|s| s.as_str()) == Some("{") {
                // inline element
                *pos += 1;
                let child = self.kv2_element(t, pos, ty, ids, refs)?;
                self.elements[me].attrs.push((name, Val::Elem(Some(child))));
                continue;
            }
            if let Some(base) = ty.strip_suffix("_array") {
                if t.get(*pos).map(|s| s.as_str()) != Some("[") {
                    return Err(format!("KeyValues2: expected '[' for {name}"));
                }
                *pos += 1;
                let ai = self.elements[me].attrs.len();
                self.elements[me].attrs.push((name, Val::Arr(Vec::new())));
                let mut k = 0;
                while *pos < t.len() && t[*pos] != "]" {
                    if t[*pos] == "," {
                        *pos += 1;
                        continue;
                    }
                    let v = if base == "element" {
                        if t.get(*pos + 1).map(|s| s.as_str()) == Some("{") {
                            let ety = t[*pos].clone();
                            *pos += 2;
                            Val::Elem(Some(self.kv2_element(t, pos, ety, ids, refs)?))
                        } else {
                            // "element" "id"
                            let id = t.get(*pos + 1).cloned().unwrap_or_default();
                            *pos += 2;
                            refs.push((me, vec![ai, k], id));
                            Val::Elem(None)
                        }
                    } else {
                        let v = kv2_value(base, &t[*pos]);
                        *pos += 1;
                        v
                    };
                    if let Val::Arr(a) = &mut self.elements[me].attrs[ai].1 {
                        a.push(v);
                    }
                    k += 1;
                }
                *pos += 1;
                continue;
            }
            let raw = t.get(*pos).cloned().unwrap_or_default();
            *pos += 1;
            match ty.as_str() {
                "elementid" => {
                    ids.push((raw, me));
                }
                "element" => {
                    let ai = self.elements[me].attrs.len();
                    self.elements[me].attrs.push((name, Val::Elem(None)));
                    if !raw.is_empty() {
                        refs.push((me, vec![ai], raw));
                    }
                }
                _ => {
                    if name == "name" {
                        self.elements[me].name = raw.clone();
                    }
                    self.elements[me].attrs.push((name, kv2_value(&ty, &raw)));
                }
            }
        }
        *pos += 1;
        Ok(me)
    }

    pub fn el(&self, v: Option<&Val>) -> Option<&Element> {
        match v {
            Some(Val::Elem(Some(i))) => self.elements.get(*i),
            _ => None,
        }
    }

    fn el_index(v: Option<&Val>) -> Option<usize> {
        match v {
            Some(Val::Elem(Some(i))) => Some(*i),
            _ => None,
        }
    }

    fn el_array(v: Option<&Val>) -> Vec<usize> {
        match v {
            Some(Val::Arr(a)) => a.iter().filter_map(|x| if let Val::Elem(Some(i)) = x { Some(*i) } else { None }).collect(),
            _ => Vec::new(),
        }
    }
}

fn read_value(r: &mut Reader, ty: u8, ver: i32, table: &[String], in_array: bool) -> Result<Val, String> {
    Ok(match ty {
        1 => {
            let i = r.i32()?;
            if i == -2 {
                r.cstr()?; // external element id
                Val::Elem(None)
            } else if i < 0 {
                Val::Elem(None)
            } else {
                Val::Elem(Some(i as usize))
            }
        }
        2 => Val::Int(r.i32()?),
        3 => Val::Float(r.f32()?),
        4 => Val::Bool(r.u8()? != 0),
        5 => {
            if ver >= 4 && !in_array {
                let i = if ver >= 5 { r.i32()? } else { r.i16()? as i32 };
                Val::Str(table.get(i.max(0) as usize).cloned().unwrap_or_default())
            } else {
                Val::Str(r.cstr()?)
            }
        }
        6 => {
            let n = r.i32()?.max(0) as usize;
            Val::Bin(r.bytes(n)?.to_vec())
        }
        7 => {
            // binary v1-2 (Source 2007-2013): object id; later: time in 1/10000 s
            if ver <= 2 {
                r.bytes(16)?;
                Val::Time(0.0)
            } else {
                Val::Time(r.i32()? as f32 / 10000.0)
            }
        }
        8 => {
            let b = r.bytes(4)?;
            Val::Color([b[0], b[1], b[2], b[3]])
        }
        9 => Val::V2([r.f32()?, r.f32()?]),
        10 | 12 => Val::V3([r.f32()?, r.f32()?, r.f32()?]),
        11 | 13 => Val::V4([r.f32()?, r.f32()?, r.f32()?, r.f32()?]),
        14 => {
            let mut m = [0.0; 16];
            for x in &mut m {
                *x = r.f32()?;
            }
            Val::Mat(m)
        }
        15..=28 => {
            let n = r.i32()?;
            if !(0..=50_000_000).contains(&n) {
                return Err("corrupt DMX array length".into());
            }
            let mut a = Vec::with_capacity(n as usize);
            for _ in 0..n {
                a.push(read_value(r, ty - 14, ver, table, true)?);
            }
            Val::Arr(a)
        }
        t => return Err(format!("unknown DMX attribute type {t} at byte {}", r.p)),
    })
}

fn kv2_tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let c: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        if ch.is_whitespace() {
            i += 1;
        } else if ch == '/' && c.get(i + 1) == Some(&'/') {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
        } else if ch == '<' && c.get(i + 1) == Some(&'!') {
            while i < c.len() && c[i] != '>' {
                i += 1;
            }
            i += 1;
        } else if matches!(ch, '{' | '}' | '[' | ']' | ',') {
            out.push(ch.to_string());
            i += 1;
        } else if ch == '"' {
            i += 1;
            let mut s = String::new();
            while i < c.len() && c[i] != '"' {
                if c[i] == '\\' && i + 1 < c.len() {
                    i += 1;
                }
                s.push(c[i]);
                i += 1;
            }
            i += 1;
            out.push(s);
        } else {
            let mut s = String::new();
            while i < c.len() && !c[i].is_whitespace() && !matches!(c[i], '{' | '}' | '[' | ']' | ',' | '"') {
                s.push(c[i]);
                i += 1;
            }
            out.push(s);
        }
    }
    out
}

fn kv2_value(ty: &str, raw: &str) -> Val {
    let f: Vec<f32> = raw.split_whitespace().filter_map(|x| x.parse().ok()).collect();
    let g = |i: usize| f.get(i).copied().unwrap_or(0.0);
    match ty {
        "int" => Val::Int(raw.trim().parse().unwrap_or(0)),
        "float" => Val::Float(g(0)),
        "bool" => Val::Bool(raw.trim() == "1" || raw.trim().eq_ignore_ascii_case("true")),
        "time" => Val::Time(g(0)),
        "vector2" => Val::V2([g(0), g(1)]),
        "vector3" | "qangle" => Val::V3([g(0), g(1), g(2)]),
        "vector4" | "quaternion" => Val::V4([g(0), g(1), g(2), g(3)]),
        "color" => Val::Color([g(0) as u8, g(1) as u8, g(2) as u8, g(3) as u8]),
        "matrix" => {
            let mut m = [0.0; 16];
            for (k, x) in m.iter_mut().enumerate() {
                *x = g(k);
            }
            Val::Mat(m)
        }
        _ => Val::Str(raw.to_string()),
    }
}

// ---------------------------------------------------------------------------
// Model conversion
// ---------------------------------------------------------------------------

fn floats(v: Option<&Val>) -> Vec<f32> {
    match v {
        Some(Val::Arr(a)) => a.iter().map(|x| if let Val::Float(f) = x { *f } else { 0.0 }).collect(),
        _ => Vec::new(),
    }
}
fn ints(v: Option<&Val>) -> Vec<i32> {
    match v {
        Some(Val::Arr(a)) => a.iter().map(|x| if let Val::Int(i) = x { *i } else { -1 }).collect(),
        _ => Vec::new(),
    }
}
fn v3s(v: Option<&Val>) -> Vec<Vec3> {
    match v {
        Some(Val::Arr(a)) => a.iter().map(|x| if let Val::V3(f) = x { Vec3::from_array(*f) } else { Vec3::ZERO }).collect(),
        _ => Vec::new(),
    }
}
fn v2s(v: Option<&Val>) -> Vec<[f32; 2]> {
    match v {
        Some(Val::Arr(a)) => a.iter().map(|x| if let Val::V2(f) = x { *f } else { [0.0; 2] }).collect(),
        _ => Vec::new(),
    }
}

fn transform_of(d: &Dmx, xf: Option<&Element>) -> (Vec3, Quat) {
    let Some(x) = xf else { return (Vec3::ZERO, Quat::IDENTITY) };
    let p = match x.get("position") {
        Some(Val::V3(v)) => Vec3::from_array(*v),
        _ => Vec3::ZERO,
    };
    let q = match x.get("orientation") {
        Some(Val::V4(v)) => Quat::from_xyzw(v[0], v[1], v[2], v[3]).normalize(),
        _ => Quat::IDENTITY,
    };
    let _ = d;
    (p, q)
}

/// Convert a DMX model into SMD data: skeleton (bind pose as frame 0) and triangles.
pub fn to_smd(d: &Dmx) -> Result<Smd, String> {
    let root = &d.elements[0];
    let model_i = Dmx::el_index(root.get("model")).or_else(|| Dmx::el_index(root.get("skeleton"))).ok_or("DMX has no 'model' or 'skeleton'")?;
    let model = &d.elements[model_i];

    // walk the dag hierarchy: (element index, parent element index, world transform, local)
    struct Node {
        el: usize,
        parent: Option<usize>,
        local: (Vec3, Quat),
        world: Affine3A,
    }
    let mut nodes: Vec<Node> = Vec::new();
    fn walk(d: &Dmx, el: usize, parent: Option<usize>, parent_world: Affine3A, nodes: &mut Vec<Node>, depth: usize) {
        if depth > 256 || nodes.iter().any(|n| n.el == el) {
            return;
        }
        let e = &d.elements[el];
        let local = transform_of(d, d.el(e.get("transform")));
        let world = parent_world * Affine3A::from_rotation_translation(local.1, local.0);
        let me = nodes.len();
        nodes.push(Node { el, parent, local, world });
        for c in Dmx::el_array(e.get("children")) {
            walk(d, c, Some(me), world, nodes, depth + 1);
        }
    }
    for c in Dmx::el_array(model.get("children")) {
        walk(d, c, None, Affine3A::IDENTITY, &mut nodes, 0);
    }

    // which nodes are joints, in joint-index order
    let joint_list = Dmx::el_array(model.get("jointList"));
    let joint_xfs = Dmx::el_array(model.get("jointTransforms"));
    let joint_nodes: Vec<usize> = if !joint_list.is_empty() {
        joint_list.iter().filter_map(|j| nodes.iter().position(|n| n.el == *j)).collect()
    } else if !joint_xfs.is_empty() {
        joint_xfs.iter().filter_map(|t| nodes.iter().position(|n| Dmx::el_index(d.elements[n.el].get("transform")) == Some(*t))).collect()
    } else {
        (0..nodes.len()).filter(|&i| d.el(d.elements[nodes[i].el].get("shape")).is_none()).collect()
    };
    // joint-array index -> smd bone; nodes that are not joints map to their nearest joint ancestor
    let mut smd = Smd::default();
    let mut node_bone: Vec<Option<usize>> = vec![None; nodes.len()];
    for (bi, &ni) in joint_nodes.iter().enumerate() {
        node_bone[ni] = Some(bi);
    }
    let nearest_joint = |mut ni: Option<usize>, node_bone: &[Option<usize>]| -> Option<usize> {
        while let Some(n) = ni {
            if let Some(b) = node_bone[n] {
                return Some(b);
            }
            ni = nodes[n].parent;
        }
        None
    };
    let mut frame = Vec::new();
    for &ni in &joint_nodes {
        let parent_bone = nearest_joint(nodes[ni].parent, &node_bone).map(|b| b as i32).unwrap_or(-1);
        // local transform relative to the parent joint (skips non-joint dags in between)
        let world = nodes[ni].world;
        let local = if parent_bone >= 0 { nodes[joint_nodes[parent_bone as usize]].world.inverse() * world } else { world };
        let (_, q, p) = local.to_scale_rotation_translation();
        let _ = nodes[ni].local;
        smd.nodes.push(SmdNode { name: d.elements[nodes[ni].el].name.clone(), parent: parent_bone });
        frame.push((p, crate::formats::mdl::quat_to_euler(q)));
    }
    if smd.nodes.is_empty() {
        smd.nodes.push(SmdNode { name: "root".into(), parent: -1 });
        frame.push((Vec3::ZERO, Vec3::ZERO));
    }
    smd.frames.push(frame);

    // meshes
    for (ni, n) in nodes.iter().enumerate() {
        let dag = &d.elements[n.el];
        let Some(mesh) = d.el(dag.get("shape")) else { continue };
        if mesh.ty != "DmeMesh" {
            continue;
        }
        let state = d.el(mesh.get("bindState")).or_else(|| d.el(mesh.get("currentState"))).or_else(|| Dmx::el_array(mesh.get("baseStates")).first().map(|&i| &d.elements[i]));
        let Some(vd) = state else { continue };
        let pick = |a: &str, b: &str| vd.get(a).or_else(|| vd.get(b));
        let pos = v3s(pick("positions", "position$0"));
        let pos_i = ints(pick("positionsIndices", "position$0Indices"));
        let nrm = v3s(pick("normals", "normal$0"));
        let nrm_i = ints(pick("normalsIndices", "normal$0Indices"));
        let uv = v2s(pick("textureCoordinates", "texcoord$0"));
        let uv_i = ints(pick("textureCoordinatesIndices", "texcoord$0Indices"));
        let jw = floats(pick("jointWeights", "blendweights$0"));
        let ji = ints(pick("jointIndices", "blendindices$0"));
        let jc = match vd.get("jointCount") {
            Some(Val::Int(c)) => *c.max(&0) as usize,
            _ => 0,
        };
        let flip_v = matches!(vd.get("flipVCoordinates"), Some(Val::Bool(true)));
        let rigid_bone = nearest_joint(Some(ni), &node_bone).unwrap_or(0) as i32;
        let world = n.world;
        let make_vert = |k: usize| -> SmdVert {
            let pi = pos_i.get(k).copied().unwrap_or(k as i32).max(0) as usize;
            let p = world.transform_point3(pos.get(pi).copied().unwrap_or(Vec3::ZERO));
            let nn = nrm_i.get(k).and_then(|&i| nrm.get(i.max(0) as usize)).copied().unwrap_or(Vec3::Z);
            let nn = world.transform_vector3(nn).normalize_or(Vec3::Z);
            let t = uv_i.get(k).and_then(|&i| uv.get(i.max(0) as usize)).copied().unwrap_or([0.0, 0.0]);
            // flipVCoordinates = 1 means V is stored bottom-up like SMD (verified against a
            // compiled model); otherwise it is top-down and must be flipped into SMD convention
            let t = if flip_v { t } else { [t[0], 1.0 - t[1]] };
            let mut links = Vec::new();
            if jc > 0 {
                for j in 0..jc {
                    let w = jw.get(pi * jc + j).copied().unwrap_or(0.0);
                    let b = ji.get(pi * jc + j).copied().unwrap_or(-1);
                    if w > 0.0 && b >= 0 && (b as usize) < smd.nodes.len() {
                        links.push((b, w));
                    }
                }
            }
            if links.is_empty() {
                links.push((rigid_bone, 1.0));
            }
            SmdVert { bone: links[0].0, pos: p, normal: nn, uv: t, links }
        };
        for fs in Dmx::el_array(mesh.get("faceSets")) {
            let fse = &d.elements[fs];
            let mat = d
                .el(fse.get("material"))
                .and_then(|m| match m.get("mtlName") {
                    Some(Val::Str(s)) => Some(s.clone()),
                    _ => Some(m.name.clone()),
                })
                .unwrap_or_else(|| "default".into());
            let mat = mat.replace('\\', "/");
            let faces = ints(fse.get("faces"));
            let mut poly: Vec<usize> = Vec::new();
            for f in faces.iter().chain(std::iter::once(&-1)) {
                if *f < 0 {
                    for k in 1..poly.len().saturating_sub(1) {
                        smd.triangles.push(SmdTri { material: mat.clone(), verts: [make_vert(poly[0]), make_vert(poly[k]), make_vert(poly[k + 1])] });
                    }
                    poly.clear();
                } else {
                    poly.push(*f as usize);
                }
            }
        }
    }
    Ok(smd)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn kv2_model() {
        let text = "<!-- dmx encoding keyvalues2 1 format model 1 -->\n\
\"DmElement\" { \"id\" \"elementid\" \"a\" \"model\" \"element\" \"m\" }\n\
\"DmeModel\" { \"id\" \"elementid\" \"m\" \"children\" \"element_array\" [ \"element\" \"j\" ] \"jointTransforms\" \"element_array\" [ \"element\" \"jt\" ] }\n\
\"DmeJoint\" { \"id\" \"elementid\" \"j\" \"name\" \"string\" \"root\" \"transform\" \"DmeTransform\" { \"id\" \"elementid\" \"jt\" \"position\" \"vector3\" \"0 0 1\" \"orientation\" \"quaternion\" \"0 0 0 1\" }\n\
 \"shape\" \"DmeMesh\" { \"id\" \"elementid\" \"me\" \"bindState\" \"DmeVertexData\" { \"id\" \"elementid\" \"vd\" \"positions\" \"vector3_array\" [ \"0 0 0\", \"1 0 0\", \"0 1 0\", \"1 1 0\" ] \"positionsIndices\" \"int_array\" [ \"0\", \"1\", \"2\", \"3\" ] }\n\
 \"faceSets\" \"element_array\" [ \"DmeFaceSet\" { \"id\" \"elementid\" \"fs\" \"faces\" \"int_array\" [ \"0\", \"1\", \"3\", \"2\", \"-1\" ] \"material\" \"DmeMaterial\" { \"id\" \"elementid\" \"mt\" \"mtlName\" \"string\" \"skin\" } } ] }\n\
 \"children\" \"element_array\" [ ] }\n";
        let d = Dmx::parse(text.as_bytes()).unwrap();
        let s = to_smd(&d).unwrap();
        assert_eq!(s.nodes.len(), 1);
        assert_eq!(s.triangles.len(), 2);
        assert_eq!(s.triangles[0].material, "skin");
        assert!((s.triangles[0].verts[1].pos - Vec3::new(1.0, 0.0, 1.0)).length() < 1e-5);
    }
}

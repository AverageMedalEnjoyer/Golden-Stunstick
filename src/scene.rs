//! A loaded studio model ready to be posed and drawn: MDL + VVD + VTX + PHY,
//! `$includemodel` animation sources, VMT/VTF materials from the mounted game.

use crate::formats::keyvalues::{self, Kv};
use crate::formats::mdl::*;
use crate::formats::phy::Phy;
use crate::formats::vtf;
use crate::gamefs::GameFs;
use crate::render::{Blend, DrawCall, GpuVertex, LineVertex, TextureUpload, line, wire_box};
use glam::{Affine3A, Mat3, Quat, Vec3};
use std::path::Path;
use std::sync::Arc;

#[derive(Clone, Debug, Default)]
pub struct MaterialInfo {
    pub name: String,
    pub vmt_path: Option<String>,
    pub vmt_source: Option<String>,
    pub shader: String,
    pub base_texture: Option<String>,
    pub base_texture_source: Option<String>,
    pub translucent: bool,
    pub additive: bool,
    pub alphatest: Option<f32>,
    pub nocull: bool,
    pub selfillum: bool,
    pub blend_tint_by_base_alpha: bool,
    /// TF2 paintable (ItemTintColor proxy or $blendtintbybasealpha)
    pub paintable: bool,
    pub color: [f32; 3],
    pub error: Option<String>,
}

/// An animation source: the model itself (index 0) or one of its `$includemodel`s.
pub struct AnimSource {
    pub name: String,
    pub mdl: Mdl,
    pub ani: Option<Vec<u8>>,
    /// for each bone of this source, the bone index in the main model
    pub to_main: Vec<Option<usize>>,
}

#[derive(Clone, Debug)]
pub struct SeqRef {
    pub source: usize,
    pub index: usize,
    pub name: String,
    pub activity: String,
    pub frames: i32,
    pub fps: f32,
    pub looping: bool,
    pub delta: bool,
}

pub struct ModelScene {
    pub path: String,
    pub vvd: Option<Vvd>,
    pub vtx: Option<Vtx>,
    pub phy: Option<Phy>,
    /// sources[0] is the model itself
    pub sources: Vec<AnimSource>,
    pub seqs: Vec<SeqRef>,
    pub materials: Vec<MaterialInfo>,
    pub uploads: Vec<TextureUpload>,
    pub bind_world: Vec<Affine3A>,
    pub skin_base: Vec<Affine3A>,
    pub warnings: Vec<String>,
    pub mdl_source: String,
    /// pose parameters of the model and its `$includemodel`s, merged by name
    pub pose_params: Vec<PoseParam>,
}

fn affine_from_3x4(m: &[[f32; 4]; 3]) -> Affine3A {
    Affine3A::from_mat3_translation(
        Mat3::from_cols(Vec3::new(m[0][0], m[1][0], m[2][0]), Vec3::new(m[0][1], m[1][1], m[2][1]), Vec3::new(m[0][2], m[1][2], m[2][2])),
        Vec3::new(m[0][3], m[1][3], m[2][3]),
    )
}

fn missing_texture() -> TextureUpload {
    let mut px = vec![0u8; 16 * 16 * 4];
    for y in 0..16 {
        for x in 0..16 {
            let o = (y * 16 + x) * 4;
            let purple = (x / 8 + y / 8) % 2 == 0;
            px[o..o + 4].copy_from_slice(if purple { &[255, 0, 255, 255] } else { &[0, 0, 0, 255] });
        }
    }
    TextureUpload { key: "__missing".into(), width: 16, height: 16, rgba: Arc::new(px), clamp: false }
}

/// Where model bytes come from.
pub enum ModelOrigin<'a> {
    Game(&'a str),
    Disk(&'a Path),
}

impl ModelScene {
    pub fn main(&self) -> &Mdl {
        &self.sources[0].mdl
    }

    pub fn load(fs: Option<&GameFs>, origin: ModelOrigin) -> Result<ModelScene, String> {
        let mut warnings = Vec::new();
        let is_game = matches!(origin, ModelOrigin::Game(_));
        let (path_label, read): (String, Box<dyn Fn(&str) -> Option<Vec<u8>> + '_>) = match origin {
            ModelOrigin::Game(p) => {
                let gfs = fs.ok_or("No game is mounted")?;
                let base = p.trim_end_matches(".mdl").to_string();
                let read = move |ext: &str| gfs.read(&format!("{base}{ext}"));
                (p.to_string(), Box::new(read))
            }
            ModelOrigin::Disk(p) => {
                let base = p.with_extension("");
                let read = move |ext: &str| std::fs::read(format!("{}{ext}", base.display())).ok();
                (p.display().to_string(), Box::new(read))
            }
        };
        let mdl_bytes = read(".mdl").ok_or_else(|| format!("Could not read {path_label}"))?;
        let mdl = Mdl::parse(mdl_bytes).map_err(|e| format!("{path_label}: {e}"))?;
        let vvd = read(".vvd").and_then(|d| Vvd::parse(&d).map_err(|e| warnings.push(format!("VVD: {e}"))).ok());
        let vtx = read(".dx90.vtx").or_else(|| read(".dx80.vtx")).or_else(|| read(".sw.vtx")).and_then(|d| Vtx::parse(&d).map_err(|e| warnings.push(format!("VTX: {e}"))).ok());
        if vvd.is_none() && !mdl.bodyparts.iter().all(|b| b.models.iter().all(|m| m.meshes.is_empty())) {
            warnings.push("VVD file missing: the model has no visible geometry".into());
        }
        let phy = read(".phy").and_then(|d| Phy::parse(&d).ok());
        let mut ani = read(".ani");
        if ani.is_none() && !mdl.anim_block_name.is_empty() {
            ani = fs.and_then(|f| f.read(&mdl.anim_block_name));
        }
        let mdl_source = match fs {
            Some(f) if is_game => f.find(&path_label).map(|i| f.sources[i].short_name()).unwrap_or_default(),
            _ => "disk".into(),
        };

        let mut scene = ModelScene {
            path: path_label,
            vvd,
            vtx,
            phy,
            sources: Vec::new(),
            seqs: Vec::new(),
            materials: Vec::new(),
            uploads: vec![missing_texture()],
            bind_world: Vec::new(),
            skin_base: Vec::new(),
            warnings,
            mdl_source,
            pose_params: Vec::new(),
        };
        scene.bind_world = crate::decompile::bind_world(&mdl);
        scene.skin_base = mdl.bones.iter().map(|b| affine_from_3x4(&b.pose_to_bone)).collect();
        let main_bones: Vec<String> = mdl.bones.iter().map(|b| b.name.clone()).collect();
        let includes: Vec<String> = mdl.include_models.iter().map(|i| i.name.clone()).collect();
        scene.sources.push(AnimSource { name: "(this model)".into(), to_main: (0..mdl.bones.len()).map(Some).collect(), mdl, ani });
        // includes (one level deep plus their own includes)
        if let Some(fs) = fs {
            let mut queue: Vec<String> = includes;
            let mut seen = std::collections::HashSet::new();
            while let Some(inc) = queue.pop() {
                let p = if inc.to_ascii_lowercase().starts_with("models/") { inc.clone() } else { format!("models/{inc}") };
                if !seen.insert(p.to_ascii_lowercase()) || seen.len() > 16 {
                    continue;
                }
                match fs.read(&p).map(Mdl::parse) {
                    Some(Ok(m)) => {
                        let ani = fs.read(&p.replace(".mdl", ".ani")).or_else(|| (!m.anim_block_name.is_empty()).then(|| fs.read(&m.anim_block_name)).flatten());
                        let to_main = m.bones.iter().map(|b| main_bones.iter().position(|n| n.eq_ignore_ascii_case(&b.name))).collect();
                        queue.extend(m.include_models.iter().map(|i| i.name.clone()));
                        scene.sources.push(AnimSource { name: p, mdl: m, ani, to_main });
                    }
                    Some(Err(e)) => scene.warnings.push(format!("$includemodel {p}: {e}")),
                    None => scene.warnings.push(format!("$includemodel {p} not found in the mounted game")),
                }
            }
        }
        // virtual sequence list: local first, then includes; first name wins
        let mut names = std::collections::HashSet::new();
        for (si, src) in scene.sources.iter().enumerate() {
            for (qi, s) in src.mdl.seqs.iter().enumerate() {
                if s.flags & STUDIO_OVERRIDE != 0 && (s.groupsize[0] == 0 || s.anim_indices.iter().any(|&a| a < 0 || a as usize >= src.mdl.anims.len())) {
                    continue;
                }
                if !names.insert(s.name.to_ascii_lowercase()) {
                    continue;
                }
                let first = s.anim_indices.first().map(|&a| a.max(0) as usize).and_then(|a| src.mdl.anims.get(a));
                scene.seqs.push(SeqRef {
                    source: si,
                    index: qi,
                    name: s.name.clone(),
                    activity: s.activity_name.clone(),
                    frames: first.map(|a| a.numframes).unwrap_or(1),
                    fps: first.map(|a| a.fps).unwrap_or(30.0),
                    looping: s.flags & STUDIO_LOOPING != 0,
                    delta: s.flags & STUDIO_DELTA != 0,
                });
            }
        }
        // pose parameters of the model and its includes, merged by name (as the engine's
        // virtual model does); the first definition wins
        for src in &scene.sources {
            for p in &src.mdl.pose_params {
                if !scene.pose_params.iter().any(|q| q.name.eq_ignore_ascii_case(&p.name)) {
                    scene.pose_params.push(p.clone());
                }
            }
        }
        scene.load_materials(fs);
        Ok(scene)
    }

    /// Scene-wide pose parameter indices that sequence `seq` (or its layers) responds to.
    pub fn sequence_pose_params(&self, seq: usize) -> Vec<usize> {
        let mut out = Vec::new();
        let Some(sr) = self.seqs.get(seq) else { return out };
        let mut stack = vec![(sr.source, sr.index, 0u32)];
        while let Some((si, qi, depth)) = stack.pop() {
            let src = &self.sources[si];
            let Some(sd) = src.mdl.seqs.get(qi) else { continue };
            let mut add = |local: i32| {
                let Some(name) = usize::try_from(local).ok().and_then(|l| src.mdl.pose_params.get(l)).map(|p| &p.name) else { return };
                if let Some(i) = self.pose_params.iter().position(|p| p.name.eq_ignore_ascii_case(name)) {
                    if !out.contains(&i) {
                        out.push(i);
                    }
                }
            };
            for k in 0..2 {
                if sd.groupsize[k] > 1 {
                    add(sd.paramindex[k]);
                }
            }
            for l in &sd.autolayers {
                if l.flags & 0x4000 != 0 && l.start != l.end {
                    add(l.pose as i32);
                }
                if l.sequence >= 0 && depth < 4 {
                    stack.push((si, l.sequence as usize, depth + 1));
                }
            }
        }
        out
    }

    /// Value of source `si`'s pose parameter `local` from the scene-wide list `values`.
    fn pose_value(&self, si: usize, local: i32, values: &[f32]) -> Option<f32> {
        let name = &self.sources[si].mdl.pose_params.get(usize::try_from(local).ok()?)?.name;
        let i = self.pose_params.iter().position(|p| p.name.eq_ignore_ascii_case(name))?;
        values.get(i).copied()
    }

    fn load_materials(&mut self, fs: Option<&GameFs>) {
        let mdl = &self.sources[0].mdl;
        let mut mats = Vec::new();
        for t in &mdl.textures {
            let mut m = MaterialInfo { name: t.name.clone(), color: [1.0; 3], ..Default::default() };
            let Some(fs) = fs else {
                m.error = Some("no game mounted".into());
                mats.push(m);
                continue;
            };
            let Some(vmt) = fs.find_material(&mdl.cdmaterials, &t.name) else {
                m.error = Some("VMT not found in any $cdmaterials path".into());
                mats.push(m);
                continue;
            };
            m.vmt_source = fs.find(&vmt).map(|i| fs.sources[i].short_name());
            m.vmt_path = Some(vmt.clone());
            let kv = load_vmt(fs, &vmt, 0);
            if let Some(kv) = kv {
                m.shader = kv.key.clone();
                let g = |k: &str| kv.get_str(k).map(|s| s.to_string());
                let flag = |k: &str| g(k).map(|v| v.trim() == "1").unwrap_or(false);
                m.translucent = flag("$translucent");
                m.additive = flag("$additive");
                m.nocull = flag("$nocull");
                m.selfillum = flag("$selfillum") && !m.shader.eq_ignore_ascii_case("VertexLitGeneric");
                if flag("$alphatest") {
                    m.alphatest = Some(g("$alphatestreference").and_then(|s| s.parse().ok()).unwrap_or(0.5));
                }
                m.blend_tint_by_base_alpha = flag("$blendtintbybasealpha");
                m.paintable = m.blend_tint_by_base_alpha || kv.get("Proxies").map(|p| p.get("ItemTintColor").is_some()).unwrap_or(false);
                if let Some(c) = g("$color2").or_else(|| g("$color")) {
                    let nums: Vec<f32> = c.trim_matches(|ch| ch == '[' || ch == ']' || ch == '{' || ch == '}').split_whitespace().filter_map(|s| s.parse().ok()).collect();
                    if nums.len() >= 3 {
                        let scale = if c.contains('{') { 1.0 / 255.0 } else { 1.0 };
                        m.color = [nums[0] * scale, nums[1] * scale, nums[2] * scale];
                    }
                }
                if let Some(bt) = g("$basetexture") {
                    let tp = format!("materials/{}.vtf", bt.replace('\\', "/").trim_end_matches(".vtf"));
                    m.base_texture_source = fs.find(&tp).map(|i| fs.sources[i].short_name());
                    match fs.read(&tp) {
                        Some(data) => match vtf::decode(&data, 2048) {
                            Ok(img) => {
                                self.uploads.push(TextureUpload { key: tp.clone(), width: img.width, height: img.height, rgba: Arc::new(img.pixels), clamp: false });
                                m.base_texture = Some(tp);
                            }
                            Err(e) => m.error = Some(format!("{tp}: {e}")),
                        },
                        None => m.error = Some(format!("{tp} not found")),
                    }
                }
            } else {
                m.error = Some(format!("{vmt} could not be parsed"));
            }
            mats.push(m);
        }
        self.materials = mats;
    }

    pub fn bones(&self) -> &[Bone] {
        &self.sources[0].mdl.bones
    }

    pub fn bind_pose(&self) -> Vec<(Vec3, Quat)> {
        self.bones().iter().map(|b| (b.pos, b.quat)).collect()
    }

    /// Local pose for a sequence at `frame` (fractional): its pose-parameter blend plus its
    /// auto-layers (`addlayer` / `blendlayer`, e.g. HL2MP's body_yaw/body_pitch aim layers),
    /// following the engine's `AccumulatePose` / `AddSequenceLayers`.
    /// `pose_params` are indexed like [`ModelScene::pose_params`].
    pub fn sequence_pose(&self, seq: usize, frame: f32, pose_params: &[f32]) -> Vec<(Vec3, Quat)> {
        let mut out = self.bind_pose();
        let Some(sr) = self.seqs.get(seq) else { return out };
        let cycle = if sr.frames > 1 { frame / (sr.frames - 1) as f32 } else { 0.0 };
        // (a delta sequence on its own is shown on top of the bind pose)
        self.accumulate(&mut out, sr.source, sr.index, cycle, 1.0, pose_params, 0);
        out
    }

    fn accumulate(&self, out: &mut [(Vec3, Quat)], si: usize, qi: usize, cycle: f32, weight: f32, pose_params: &[f32], depth: u32) {
        let src = &self.sources[si];
        let Some(sd) = src.mdl.seqs.get(qi) else { return };
        if weight > 1e-4 {
            if let Some(vals) = self.raw_sequence_values(si, qi, cycle, pose_params) {
                let delta = sd.flags & STUDIO_DELTA != 0 || sd.anim_indices.first().and_then(|&a| src.mdl.anims.get(a.max(0) as usize)).map(|a| a.flags & STUDIO_DELTA != 0).unwrap_or(false);
                let post = sd.flags & STUDIO_POST != 0;
                for (i, v) in vals.into_iter().enumerate() {
                    let Some((p, q)) = v else { continue };
                    let Some(Some(mi)) = src.to_main.get(i) else { continue };
                    let bw = weight * sd.weightlist.get(i).copied().unwrap_or(1.0);
                    if bw <= 0.0 {
                        continue;
                    }
                    let o = &mut out[*mi];
                    if delta {
                        let dq = Quat::IDENTITY.slerp(q, bw.min(1.0));
                        o.1 = if post { o.1 * dq } else { dq * o.1 }.normalize();
                        o.0 += p * bw;
                    } else if bw >= 1.0 {
                        *o = (p, q);
                    } else {
                        let q = if o.1.dot(q) < 0.0 { -q } else { q };
                        *o = (o.0.lerp(p, bw), o.1.slerp(q, bw).normalize());
                    }
                }
            }
        }
        if depth >= 4 {
            return;
        }
        // auto-layers (studio.h STUDIO_AL_*)
        const AL_SPLINE: i32 = 0x40;
        const AL_XFADE: i32 = 0x80;
        const AL_NOBLEND: i32 = 0x200;
        const AL_POSE: i32 = 0x4000;
        for l in &sd.autolayers {
            let mut layer_cycle = cycle;
            let mut layer_weight = weight;
            if l.start != l.end {
                let index = if l.flags & AL_POSE != 0 { self.pose_value(si, l.pose as i32, pose_params).unwrap_or(0.0) } else { cycle };
                if index < l.start || index >= l.end {
                    continue;
                }
                let mut s = 1.0;
                if index < l.peak && l.start != l.peak {
                    s = (index - l.start) / (l.peak - l.start);
                } else if index > l.tail && l.end != l.tail {
                    s = (l.end - index) / (l.end - l.tail);
                }
                if l.flags & AL_SPLINE != 0 {
                    s = s * s * (3.0 - 2.0 * s);
                }
                layer_weight = if l.flags & AL_XFADE != 0 && index > l.tail {
                    (s * weight) / (1.0 - weight + s * weight).max(1e-6)
                } else if l.flags & AL_NOBLEND != 0 {
                    s
                } else {
                    weight * s
                };
                if l.flags & AL_POSE == 0 {
                    layer_cycle = (cycle - l.start) / (l.end - l.start);
                }
            }
            if l.sequence >= 0 {
                self.accumulate(out, si, l.sequence as usize, layer_cycle, layer_weight, pose_params, depth + 1);
            }
        }
    }

    /// The blended local values of source `si`'s sequence `qi` at `cycle` (0..1), per bone of
    /// that source (`None` for bones the sequence does not animate). Delta sequences give
    /// their raw deltas.
    fn raw_sequence_values(&self, si: usize, qi: usize, cycle: f32, pose_params: &[f32]) -> Option<Vec<Option<(Vec3, Quat)>>> {
        let src = &self.sources[si];
        let sd = src.mdl.seqs.get(qi)?;
        let looping = sd.flags & STUDIO_LOOPING != 0;
        let gx = sd.groupsize[0].max(1) as usize;
        let gy = sd.groupsize[1].max(1) as usize;
        // blend weights
        let param_t = |k: usize, n: usize| -> (usize, usize, f32) {
            if n <= 1 {
                return (0, 0, 0.0);
            }
            let v = self.pose_value(si, sd.paramindex[k], pose_params).unwrap_or((sd.paramstart[k] + sd.paramend[k]) * 0.5);
            let span = sd.paramend[k] - sd.paramstart[k];
            let s = if span.abs() > 1e-6 { ((v - sd.paramstart[k]) / span).clamp(0.0, 1.0) } else { 0.0 } * (n - 1) as f32;
            let i0 = (s.floor() as usize).min(n - 1);
            let i1 = (i0 + 1).min(n - 1);
            (i0, i1, s - i0 as f32)
        };
        let (x0, x1, tx) = param_t(0, gx);
        let (y0, y1, ty) = param_t(1, gy);
        let mut blend: Vec<(usize, f32)> = Vec::new();
        for (xi, wx) in [(x0, 1.0 - tx), (x1, tx)] {
            for (yi, wy) in [(y0, 1.0 - ty), (y1, ty)] {
                let w = wx * wy;
                if w > 1e-4 {
                    if let Some(&a) = sd.anim_indices.get(yi * gx + xi) {
                        blend.push((a.max(0) as usize, w));
                    }
                }
            }
        }
        if blend.is_empty() {
            return None;
        }
        let nb = src.mdl.bones.len();
        let mut acc: Vec<(Vec3, Quat, f32)> = vec![(Vec3::ZERO, Quat::IDENTITY, 0.0); nb];
        for (ai, w) in blend {
            let Some(ad) = src.mdl.anims.get(ai) else { continue };
            let n = ad.numframes.max(1);
            let c = if looping { cycle.rem_euclid(1.0) } else { cycle.clamp(0.0, 1.0) };
            let f = (c * (n - 1) as f32).clamp(0.0, (n - 1) as f32);
            let f0 = f.floor() as i32;
            let f1 = if looping && f0 + 1 >= n { 0 } else { (f0 + 1).min(n - 1) };
            let t = f - f0 as f32;
            let a = src.mdl.decode_frame(ai, f0, src.ani.as_deref());
            let b = if t > 1e-4 { src.mdl.decode_frame(ai, f1, src.ani.as_deref()) } else { a.clone() };
            for i in 0..nb {
                let p = a[i].0.lerp(b[i].0, t);
                let q = a[i].1.slerp(b[i].1, t);
                let e = &mut acc[i];
                let q = if e.2 > 0.0 && e.1.dot(q) < 0.0 { -q } else { q };
                e.0 += p * w;
                e.1 = if e.2 == 0.0 { q } else { e.1.slerp(q, w / (e.2 + w)) };
                e.2 += w;
            }
        }
        Some(acc.into_iter().map(|(p, q, w)| (w > 0.0).then(|| (p / w, q.normalize()))).collect())
    }

    /// World transforms for a local pose.
    pub fn world(&self, pose: &[(Vec3, Quat)]) -> Vec<Affine3A> {
        let mut out: Vec<Affine3A> = Vec::with_capacity(pose.len());
        for (i, b) in self.bones().iter().enumerate() {
            let local = Affine3A::from_rotation_translation(pose[i].1, pose[i].0);
            out.push(if b.parent >= 0 && (b.parent as usize) < out.len() { out[b.parent as usize] * local } else { local });
        }
        out
    }

    /// Flexdesc weights from controller values (indexed like `flex_controllers`).
    pub fn flex_weights(&self, controller_values: &[f32]) -> Vec<f32> {
        let m = self.main();
        let mut w = vec![0.0f32; m.flex_descs.len()];
        for rule in &m.flex_rules {
            let mut st: Vec<f32> = Vec::new();
            let mut ok = true;
            for &(op, idx, val) in &rule.ops {
                match op {
                    1 => st.push(val),
                    2 => st.push(controller_values.get(idx.max(0) as usize).copied().unwrap_or(0.0)),
                    3 => st.push(w.get(idx.max(0) as usize).copied().unwrap_or(0.0)),
                    4..=7 | 13 | 14 => {
                        let b = st.pop().unwrap_or(0.0);
                        let a = st.pop().unwrap_or(0.0);
                        st.push(match op {
                            4 => a + b,
                            5 => a - b,
                            6 => a * b,
                            7 => if b.abs() > 1e-9 { a / b } else { 0.0 },
                            13 => a.max(b),
                            _ => a.min(b),
                        });
                    }
                    8 => {
                        let a = st.pop().unwrap_or(0.0);
                        st.push(-a);
                    }
                    15 => st.push((-controller_values.get(idx.max(0) as usize).copied().unwrap_or(0.0)).clamp(0.0, 1.0)),
                    16 => st.push(controller_values.get(idx.max(0) as usize).copied().unwrap_or(0.0).clamp(0.0, 1.0)),
                    18 => {
                        let n = idx.max(0) as usize;
                        let mut prod = 1.0;
                        for _ in 0..n {
                            prod *= st.pop().unwrap_or(0.0);
                        }
                        st.push(prod);
                    }
                    19 => {
                        let n = idx.max(0) as usize;
                        let mut prod = 1.0;
                        for _ in 0..n {
                            prod *= st.pop().unwrap_or(0.0);
                        }
                        let v = st.pop().unwrap_or(0.0);
                        st.push(v * (1.0 - prod));
                    }
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                if let Some(slot) = w.get_mut(rule.flex.max(0) as usize) {
                    *slot = st.pop().unwrap_or(0.0);
                }
            }
        }
        w
    }

    /// Build GPU geometry for the current pose.
    #[allow(clippy::too_many_arguments)]
    pub fn build_geometry(
        &self,
        world: &[Affine3A],
        body: i32,
        skin: usize,
        lod: usize,
        flex: Option<&[f32]>,
        tint: Option<[f32; 3]>,
        verts: &mut Vec<GpuVertex>,
        indices: &mut Vec<u32>,
        draws: &mut Vec<DrawCall>,
        normals_out: Option<&mut Vec<LineVertex>>,
    ) {
        let (Some(vvd), Some(vtx)) = (&self.vvd, &self.vtx) else { return };
        let m = self.main();
        let skin_mats: Vec<Affine3A> = world.iter().zip(&self.skin_base).map(|(w, s)| *w * *s).collect();
        let mut normals_out = normals_out;
        for (bi, bp) in m.bodyparts.iter().enumerate() {
            if bp.models.is_empty() {
                continue;
            }
            let base = bp.base.max(1);
            let mi = ((body / base) as usize) % bp.models.len();
            let model = &bp.models[mi];
            let Some(vm) = vtx.bodyparts.get(bi).and_then(|b| b.get(mi)) else { continue };
            let lod = lod.min(vm.lods.len().saturating_sub(1));
            let Some(lod_meshes) = vm.lods.get(lod) else { continue };
            for (me, mesh) in model.meshes.iter().enumerate() {
                let Some(vmesh) = lod_meshes.get(me) else { continue };
                if vmesh.tris.is_empty() {
                    continue;
                }
                let first_v = (model.first_vertex + mesh.vertex_offset) as usize;
                let nverts = mesh.num_vertices.max(0) as usize;
                // flex deltas for this mesh
                let mut deltas: Vec<(Vec3, Vec3)> = Vec::new();
                if let Some(fw) = flex {
                    if !mesh.flexes.is_empty() {
                        deltas = vec![(Vec3::ZERO, Vec3::ZERO); nverts];
                        for fx in &mesh.flexes {
                            let wl = fw.get(fx.flexdesc.max(0) as usize).copied().unwrap_or(0.0);
                            let wr = if fx.flexpair != 0 { fw.get(fx.flexpair as usize).copied().unwrap_or(0.0) } else { wl };
                            if wl.abs() < 1e-4 && wr.abs() < 1e-4 {
                                continue;
                            }
                            for va in &fx.verts {
                                let s = va.side as f32 / 255.0;
                                let w = if fx.flexpair != 0 { wl * (1.0 - s) + wr * s } else { wl };
                                if let Some(d) = deltas.get_mut(va.index as usize) {
                                    d.0 += va.delta * w;
                                    d.1 += va.ndelta * w;
                                }
                            }
                        }
                    }
                }
                let vbase = verts.len() as u32;
                for k in 0..nverts {
                    let Some(v) = vvd.vertices.get(first_v + k) else { break };
                    let (dp, dn) = deltas.get(k).copied().unwrap_or((Vec3::ZERO, Vec3::ZERO));
                    let p0 = v.pos + dp;
                    let n0 = v.normal + dn;
                    let mut p = Vec3::ZERO;
                    let mut n = Vec3::ZERO;
                    let nb = v.num_bones.clamp(1, 3) as usize;
                    let mut wsum = 0.0;
                    for j in 0..nb {
                        let b = v.bones[j] as usize;
                        let w = if nb == 1 { 1.0 } else { v.weights[j] };
                        if let Some(sm) = skin_mats.get(b) {
                            p += sm.transform_point3(p0) * w;
                            n += sm.transform_vector3(n0) * w;
                            wsum += w;
                        }
                    }
                    if wsum <= 0.0 {
                        p = p0;
                        n = n0;
                    }
                    let n = n.normalize_or(Vec3::Z);
                    if let Some(no) = normals_out.as_deref_mut() {
                        line(no, p, p + n * 1.5, [0.3, 0.6, 1.0, 1.0]);
                    }
                    verts.push(GpuVertex { pos: p.to_array(), normal: n.to_array(), uv: v.uv });
                }
                let first = indices.len() as u32;
                for t in &vmesh.tris {
                    if t.iter().all(|&i| (i as usize) < nverts) {
                        indices.extend(t.iter().map(|&i| vbase + i as u32));
                    }
                }
                let count = indices.len() as u32 - first;
                let tex_idx = m.skins.get(skin).or_else(|| m.skins.first()).and_then(|f| f.get(mesh.material as usize)).map(|&i| i as usize).unwrap_or(mesh.material.max(0) as usize);
                let mat = self.materials.get(tex_idx);
                let (texture, blend, alpha_ref, cull, selfillum, mut color) = match mat {
                    Some(mm) => (
                        Some(mm.base_texture.clone().unwrap_or_else(|| "__missing".into())),
                        if mm.additive {
                            Blend::Additive
                        } else if mm.translucent {
                            Blend::Translucent
                        } else if mm.alphatest.is_some() {
                            Blend::AlphaTest
                        } else {
                            Blend::Opaque
                        },
                        mm.alphatest.unwrap_or(0.0),
                        !mm.nocull,
                        mm.selfillum,
                        mm.color,
                    ),
                    None => (Some("__missing".into()), Blend::Opaque, 0.0, true, false, [1.0; 3]),
                };
                if let (Some(t), Some(mm)) = (tint, mat) {
                    if mm.paintable {
                        color = t;
                    }
                }
                draws.push(DrawCall { first, count, texture, blend, alpha_ref, cull, tint: [color[0], color[1], color[2], 1.0], selfillum });
            }
        }
    }

    /// Box used to frame the camera: the movement hull for animated models
    /// (their bind pose is often lying down), otherwise the mesh bounds.
    pub fn frame_bounds(&self) -> (Vec3, Vec3) {
        let m = self.main();
        if !self.seqs.is_empty() && m.hull_min != m.hull_max && (m.hull_max - m.hull_min).length() > 1.0 && m.flags & 0x10 == 0 && self.seqs.len() > 1 {
            return (m.hull_min, m.hull_max);
        }
        self.bounds()
    }

    /// Bounding box of the bind pose mesh (or hull).
    pub fn bounds(&self) -> (Vec3, Vec3) {
        if let Some(vvd) = &self.vvd {
            if !vvd.vertices.is_empty() {
                let mut mn = Vec3::splat(f32::MAX);
                let mut mx = Vec3::splat(f32::MIN);
                for v in &vvd.vertices {
                    mn = mn.min(v.pos);
                    mx = mx.max(v.pos);
                }
                return (mn, mx);
            }
        }
        let m = self.main();
        if m.hull_min != m.hull_max {
            (m.hull_min, m.hull_max)
        } else {
            (Vec3::splat(-16.0), Vec3::splat(16.0))
        }
    }

    /// Collision model lines for the current pose.
    pub fn physics_lines(&self, world: &[Affine3A], out: &mut Vec<LineVertex>) {
        let Some(phy) = &self.phy else { return };
        let infos = phy.solid_infos();
        let ragdoll = phy.solids.len() > 1;
        let pose0 = crate::decompile::first_sequence_world(self.main(), self.sources[0].ani.as_deref());
        for (si, s) in phy.solids.iter().enumerate() {
            let bone = infos.iter().find(|i| i.0 == si).and_then(|i| self.main().bone_index(&i.1)).unwrap_or(0);
            let w = world.get(bone).copied().unwrap_or(Affine3A::IDENTITY);
            let xf = if ragdoll { w } else { w * pose0.get(bone).copied().unwrap_or(Affine3A::IDENTITY).inverse() };
            for c in &s.convexes {
                for t in &c.tris {
                    let p: Vec<Vec3> = t.iter().map(|v| xf.transform_point3(*v)).collect();
                    let col = [1.0, 0.55, 0.1, 0.9];
                    line(out, p[0], p[1], col);
                    line(out, p[1], p[2], col);
                    line(out, p[2], p[0], col);
                }
            }
        }
    }

    pub fn hitbox_lines(&self, world: &[Affine3A], set: usize, highlight: Option<usize>, out: &mut Vec<LineVertex>) {
        let Some(s) = self.main().hitbox_sets.get(set) else { return };
        for (i, hb) in s.hitboxes.iter().enumerate() {
            let w = world.get(hb.bone.max(0) as usize).copied().unwrap_or(Affine3A::IDENTITY);
            let palette = [[1.0, 1.0, 1.0, 1.0], [1.0, 0.3, 0.3, 1.0], [0.3, 1.0, 0.3, 1.0], [1.0, 1.0, 0.3, 1.0], [0.3, 0.5, 1.0, 1.0], [1.0, 0.4, 1.0, 1.0], [0.3, 1.0, 1.0, 1.0], [1.0, 0.6, 0.2, 1.0]];
            let mut c = palette[(hb.group.max(0) as usize) % palette.len()];
            if highlight == Some(i) {
                c = [1.0, 1.0, 0.0, 1.0];
            }
            wire_box(out, w, hb.min, hb.max, c);
        }
    }
}

/// Load a VMT, resolving `patch` materials (include + insert/replace).
pub fn load_vmt(fs: &GameFs, path: &str, depth: usize) -> Option<Kv> {
    if depth > 4 {
        return None;
    }
    let text = String::from_utf8_lossy(&fs.read(path)?).to_string();
    let mut root = keyvalues::parse_root(&text)?;
    if root.key.eq_ignore_ascii_case("patch") {
        let inc = root.get_str("include")?.replace('\\', "/");
        let mut base = load_vmt(fs, &inc, depth + 1)?;
        if let keyvalues::KvValue::Block(children) = &mut base.value {
            for sec in ["replace", "insert"] {
                if let Some(b) = root.get(sec) {
                    for kv in b.children() {
                        if let Some(existing) = children.iter_mut().find(|c| c.key.eq_ignore_ascii_case(&kv.key)) {
                            *existing = kv.clone();
                        } else {
                            children.push(kv.clone());
                        }
                    }
                }
            }
        }
        root = base;
    }
    Some(root)
}

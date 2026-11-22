//! Animator core: a QC-based rig (skeleton + reference meshes from SMDs), the
//! keyframed animation document, interpolation / easing / crossfades, the
//! "easy rig" IK chains, TXT save/load and SMD / DMX export.

use crate::formats::mdl::{euler_to_quat, quat_to_euler};
use crate::formats::qc::{self, Qc};
use crate::formats::smd::{Smd, SmdNode, SmdTri};
use glam::{Affine3A, Quat, Vec3};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Rig loaded from a QC
// ---------------------------------------------------------------------------

/// One reference mesh (SMD) of the QC with its own bind skeleton.
pub struct RigMesh {
    pub file: PathBuf,
    pub body: String,
    pub tris: Vec<SmdTri>,
    /// mesh-SMD bone index -> rig bone index
    pub bone_map: Vec<Option<usize>>,
    /// inverse bind world transforms of the mesh SMD's own skeleton
    pub inv_bind: Vec<Affine3A>,
}

pub struct Rig {
    pub qc_path: PathBuf,
    pub qc_name: String,
    pub bones: Vec<SmdNode>,
    /// bind pose: local position + rotation per bone
    pub bind: Vec<(Vec3, Quat)>,
    pub meshes: Vec<RigMesh>,
    pub cdmaterials: Vec<String>,
    pub warnings: Vec<String>,
}

/// Load an SMD or DMX (binary or text) as SMD data.
pub fn load_mesh_file(p: &Path) -> Result<Smd, String> {
    if p.extension().map(|e| e.eq_ignore_ascii_case("dmx")).unwrap_or(false) {
        let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
        let dmx = crate::formats::dmx::Dmx::parse(&bytes)?;
        crate::formats::dmx::to_smd(&dmx)
    } else {
        std::fs::read_to_string(p).map_err(|e| e.to_string()).and_then(|t| Smd::parse(&t))
    }
}

fn world_of(nodes: &[SmdNode], pose: &[(Vec3, Quat)]) -> Vec<Affine3A> {
    let mut out: Vec<Affine3A> = Vec::with_capacity(nodes.len());
    for (i, n) in nodes.iter().enumerate() {
        let (p, q) = pose.get(i).copied().unwrap_or((Vec3::ZERO, Quat::IDENTITY));
        let local = Affine3A::from_rotation_translation(q, p);
        out.push(if n.parent >= 0 && (n.parent as usize) < out.len() { out[n.parent as usize] * local } else { local });
    }
    out
}

impl Rig {
    pub fn load(qc_path: &Path) -> Result<Rig, String> {
        let q = Qc::load(qc_path)?;
        let root = q.qc_dir();
        let mut warnings: Vec<String> = q.issues.iter().filter(|i| i.severity == qc::Severity::Error).map(|i| format!("QC line {}: {}", i.line, i.message)).collect();
        let mut meshes_src: Vec<(String, PathBuf, Smd)> = Vec::new();
        for (body, file, cmd) in q.mesh_refs() {
            match qc::resolve(&root, &q.files[cmd.file], &file) {
                Some(p) => match load_mesh_file(&p) {
                    Ok(s) => meshes_src.push((body, p, s)),
                    Err(e) => warnings.push(format!("{}: {e}", p.display())),
                },
                None => warnings.push(format!("Mesh file not found: {file}")),
            }
        }
        // skeleton: union of mesh skeletons, first mesh first; fall back to the first animation SMD
        let mut bones: Vec<SmdNode> = Vec::new();
        let mut bind: Vec<(Vec3, Quat)> = Vec::new();
        let add_skeleton = |smd: &Smd, bones: &mut Vec<SmdNode>, bind: &mut Vec<(Vec3, Quat)>| {
            let frame = smd.frames.first();
            let mut map: Vec<usize> = Vec::new();
            for (i, n) in smd.nodes.iter().enumerate() {
                if let Some(ix) = bones.iter().position(|b| b.name == n.name) {
                    map.push(ix);
                    continue;
                }
                let parent = if n.parent >= 0 { map.get(n.parent as usize).map(|&p| p as i32).unwrap_or(-1) } else { -1 };
                let (p, r) = frame.and_then(|f| f.get(i)).copied().unwrap_or((Vec3::ZERO, Vec3::ZERO));
                map.push(bones.len());
                bones.push(SmdNode { name: n.name.clone(), parent });
                bind.push((p, euler_to_quat(r)));
            }
        };
        for (_, _, s) in &meshes_src {
            add_skeleton(s, &mut bones, &mut bind);
        }
        if bones.is_empty() {
            // animation-only QC: take the skeleton from the first $sequence / $animation SMD
            'outer: for c in q.commands.iter().filter(|c| c.name == "$sequence" || c.name == "$animation") {
                for t in c.args.iter().skip(1) {
                    if let Some(p) = qc::resolve(&root, &q.files[t.file], &t.text) {
                        if let Ok(s) = load_mesh_file(&p) {
                            add_skeleton(&s, &mut bones, &mut bind);
                            warnings.push(format!("No reference mesh; skeleton taken from {}", p.display()));
                            break 'outer;
                        }
                    }
                }
            }
        }
        if bones.is_empty() {
            return Err("No skeleton could be loaded from this QC (no $body/$model/$bodygroup SMDs found)".into());
        }
        let meshes = meshes_src
            .into_iter()
            .map(|(body, file, smd)| {
                let bone_map: Vec<Option<usize>> = smd.nodes.iter().map(|n| bones.iter().position(|b| b.name == n.name)).collect();
                let pose: Vec<(Vec3, Quat)> = smd.frames.first().map(|f| f.iter().map(|(p, r)| (*p, euler_to_quat(*r))).collect()).unwrap_or_default();
                let inv_bind = world_of(&smd.nodes, &pose).into_iter().map(|w| w.inverse()).collect();
                let mut tris = smd.triangles;
                tris.sort_by(|a, b| a.material.cmp(&b.material));
                RigMesh { file, body, tris, bone_map, inv_bind }
            })
            .collect();
        let qc_name = qc_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "model".into());
        Ok(Rig { qc_path: qc_path.to_path_buf(), qc_name, bones, bind, meshes, cdmaterials: q.cdmaterials(), warnings })
    }

    pub fn world(&self, pose: &[(Vec3, Quat)]) -> Vec<Affine3A> {
        world_of(&self.bones, pose)
    }

    pub fn bone_index(&self, name: &str) -> Option<usize> {
        self.bones.iter().position(|b| b.name.eq_ignore_ascii_case(name))
    }

    pub fn children(&self, i: usize) -> Vec<usize> {
        self.bones.iter().enumerate().filter(|(_, b)| b.parent == i as i32).map(|(k, _)| k).collect()
    }

    pub fn bone_signature(&self) -> Vec<(String, String)> {
        self.bones.iter().map(|b| (b.name.clone(), if b.parent >= 0 { self.bones[b.parent as usize].name.clone() } else { String::new() })).collect()
    }
}

// ---------------------------------------------------------------------------
// Easy rig (IK chains)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct IkChain {
    pub name: String,
    /// bones from the chain root down to the effector (last)
    pub bones: Vec<usize>,
    pub enabled: bool,
}

fn has_any(name: &str, keys: &[&str]) -> bool {
    let n = name.to_ascii_lowercase();
    keys.iter().any(|k| n.contains(k))
}

/// Detect arm / leg chains from bone names: hand -> forearm -> upper arm (-> clavicle),
/// foot -> calf -> thigh. Works with Valve biped, TF2 (bip_*) and most custom rigs.
pub fn detect_chains(rig: &Rig) -> Vec<IkChain> {
    let mut out = Vec::new();
    for (i, b) in rig.bones.iter().enumerate() {
        let n = b.name.to_ascii_lowercase();
        let is_hand = has_any(&n, &["hand", "wrist"]) && !has_any(&n, &["finger", "thumb", "index", "middle", "ring", "pinky", "weapon", "effect", "attach", "prop", "hlp", "helper"]);
        let is_foot = has_any(&n, &["foot", "ankle"]) && !has_any(&n, &["toe", "hlp", "helper"]);
        if !is_hand && !is_foot {
            continue;
        }
        // walk up
        let mut chain = vec![i];
        let mut p = b.parent;
        let max_links = if is_hand { 3 } else { 2 };
        while p >= 0 && chain.len() <= max_links {
            let pn = rig.bones[p as usize].name.to_ascii_lowercase();
            if has_any(&pn, &["spine", "pelvis", "neck", "head", "root", "hips"]) && chain.len() > 1 {
                break;
            }
            chain.push(p as usize);
            if has_any(&pn, &["clavicle", "collar", "shoulder"]) || (is_foot && has_any(&pn, &["thigh", "hip"])) {
                break;
            }
            p = rig.bones[p as usize].parent;
        }
        if chain.len() < 3 {
            continue;
        }
        chain.reverse();
        let side = if n.ends_with('l') || n.contains("_l_") || n.contains(".l") || n.contains("left") || n.ends_with("_l") { "Left" } else if n.ends_with('r') || n.contains("_r_") || n.contains(".r") || n.contains("right") { "Right" } else { "" };
        let kind = if is_hand { "arm" } else { "leg" };
        out.push(IkChain { name: format!("{side} {kind} ({})", b.name).trim().to_string(), bones: chain, enabled: true });
    }
    out
}

/// CCD IK: move the chain's effector toward `target` (world space), keeping the
/// effector's world rotation. Modifies the local pose in place.
pub fn solve_ik(rig: &Rig, pose: &mut [(Vec3, Quat)], chain: &[usize], target: Vec3) {
    let Some(&effector) = chain.last() else { return };
    let eff_world_rot = {
        let w = rig.world(pose);
        let (_, r, _) = w[effector].to_scale_rotation_translation();
        r
    };
    for _ in 0..24 {
        let mut done = false;
        for &j in chain.iter().rev().skip(1) {
            let w = rig.world(pose);
            let joint: Vec3 = w[j].translation.into();
            let end: Vec3 = w[effector].translation.into();
            let to_end = (end - joint).normalize_or_zero();
            let to_target = (target - joint).normalize_or_zero();
            if to_end == Vec3::ZERO || to_target == Vec3::ZERO {
                continue;
            }
            let delta = Quat::from_rotation_arc(to_end, to_target);
            // limit each step for stability
            let (axis, angle) = delta.to_axis_angle();
            let delta = Quat::from_axis_angle(axis, angle.min(0.35));
            // apply delta in world space to joint's local rotation
            let parent_rot = {
                let pi = rig.bones[j].parent;
                if pi >= 0 { w[pi as usize].to_scale_rotation_translation().1 } else { Quat::IDENTITY }
            };
            let world_rot = parent_rot * pose[j].1;
            let new_world = (delta * world_rot).normalize();
            pose[j].1 = (parent_rot.inverse() * new_world).normalize();
            let w2 = rig.world(pose);
            let e: Vec3 = w2[effector].translation.into();
            if (e - target).length() < 0.01 {
                done = true;
                break;
            }
        }
        if done {
            break;
        }
    }
    // restore the effector's world orientation
    let w = rig.world(pose);
    let pi = rig.bones[effector].parent;
    if pi >= 0 {
        let parent_rot = w[pi as usize].to_scale_rotation_translation().1;
        pose[effector].1 = (parent_rot.inverse() * eff_world_rot).normalize();
    }
}

// ---------------------------------------------------------------------------
// Animation document
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ease {
    Linear,
    Smooth,
    EaseIn,
    EaseOut,
    Spline,
    Hold,
}

impl Ease {
    pub const ALL: [Ease; 6] = [Ease::Linear, Ease::Smooth, Ease::EaseIn, Ease::EaseOut, Ease::Spline, Ease::Hold];
    pub fn name(self) -> &'static str {
        match self {
            Ease::Linear => "linear",
            Ease::Smooth => "smooth",
            Ease::EaseIn => "easein",
            Ease::EaseOut => "easeout",
            Ease::Spline => "spline",
            Ease::Hold => "hold",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Ease::Linear => "Linear",
            Ease::Smooth => "Smooth (ease in + out)",
            Ease::EaseIn => "Ease in (slow start)",
            Ease::EaseOut => "Ease out (slow end)",
            Ease::Spline => "Spline (smooth through keys)",
            Ease::Hold => "Hold (step)",
        }
    }
    pub fn parse(s: &str) -> Ease {
        Ease::ALL.into_iter().find(|e| e.name().eq_ignore_ascii_case(s)).unwrap_or(Ease::Linear)
    }
    pub fn apply(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Ease::Linear | Ease::Spline => t,
            Ease::Smooth => t * t * (3.0 - 2.0 * t),
            Ease::EaseIn => t * t,
            Ease::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
            Ease::Hold => 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Keyframe {
    pub pose: Vec<(Vec3, Quat)>,
    /// interpolation from this key to the next one
    pub ease: Ease,
}

#[derive(Clone, Debug)]
pub struct AnimDoc {
    pub name: Option<String>,
    pub qc_path: PathBuf,
    pub fps: f32,
    pub frames: u32,
    pub looping: bool,
    /// 0..1: extra smoothing applied when sampling (blends toward a spline through the keys)
    pub smoothing: f32,
    pub bones: Vec<(String, String)>,
    pub keys: BTreeMap<u32, Keyframe>,
}

fn catmull(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, t: f32) -> Vec3 {
    let t2 = t * t;
    let t3 = t2 * t;
    0.5 * ((2.0 * p1) + (-p0 + p2) * t + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * t2 + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * t3)
}

fn quat_catmull(q0: Quat, q1: Quat, q2: Quat, q3: Quat, t: f32) -> Quat {
    // align hemispheres then interpolate components (good enough for small steps), renormalize
    let a = |q: Quat, r: Quat| if q.dot(r) < 0.0 { -q } else { q };
    let q0 = a(q0, q1);
    let q2 = a(q2, q1);
    let q3 = a(q3, q2);
    let v = catmull(Vec3::new(q0.x, q0.y, q0.z), Vec3::new(q1.x, q1.y, q1.z), Vec3::new(q2.x, q2.y, q2.z), Vec3::new(q3.x, q3.y, q3.z), t);
    let w = catmull(Vec3::splat(q0.w), Vec3::splat(q1.w), Vec3::splat(q2.w), Vec3::splat(q3.w), t).x;
    Quat::from_xyzw(v.x, v.y, v.z, w).normalize()
}

impl AnimDoc {
    pub fn new(rig: &Rig) -> AnimDoc {
        let mut keys = BTreeMap::new();
        keys.insert(0, Keyframe { pose: rig.bind.clone(), ease: Ease::Smooth });
        AnimDoc { name: None, qc_path: rig.qc_path.clone(), fps: 30.0, frames: 30, looping: false, smoothing: 0.0, bones: rig.bone_signature(), keys }
    }

    pub fn display_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| "(unsaved animation)".into())
    }

    /// Sample the pose at a (fractional) frame.
    pub fn sample(&self, frame: f32, bind: &[(Vec3, Quat)]) -> Vec<(Vec3, Quat)> {
        let n = bind.len();
        if self.keys.is_empty() {
            return bind.to_vec();
        }
        let keys: Vec<(&u32, &Keyframe)> = self.keys.iter().collect();
        let first = keys[0];
        let last = keys[keys.len() - 1];
        let pad = |k: &Keyframe| -> Vec<(Vec3, Quat)> {
            let mut p = k.pose.clone();
            p.resize(n, (Vec3::ZERO, Quat::IDENTITY));
            for (i, b) in bind.iter().enumerate() {
                if i >= k.pose.len() {
                    p[i] = *b;
                }
            }
            p
        };
        if frame <= *first.0 as f32 || keys.len() == 1 {
            if keys.len() > 1 && self.looping && frame < *first.0 as f32 {
                // wrap from last to first
            } else {
                return pad(first.1);
            }
        }
        // find segment
        let mut seg = None;
        for w in keys.windows(2) {
            if frame >= *w[0].0 as f32 && frame <= *w[1].0 as f32 {
                seg = Some((w[0], w[1]));
                break;
            }
        }
        let (a, b) = match seg {
            Some(s) => s,
            None => {
                if self.looping && keys.len() > 1 {
                    // after the last key: blend last -> first over the remaining frames
                    let span = (self.frames as f32 - *last.0 as f32).max(1.0);
                    let t = ((frame - *last.0 as f32) / span).clamp(0.0, 1.0);
                    let pa = pad(last.1);
                    let pb = pad(first.1);
                    let e = last.1.ease.apply(t);
                    return pa.iter().zip(pb.iter()).map(|(x, y)| (x.0.lerp(y.0, e), x.1.slerp(y.1, e))).collect();
                }
                return pad(last.1);
            }
        };
        let span = (*b.0 as f32 - *a.0 as f32).max(1e-6);
        let t = (frame - *a.0 as f32) / span;
        let pa = pad(a.1);
        let pb = pad(b.1);
        let ease = a.1.ease;
        let e = ease.apply(t);
        let mut out: Vec<(Vec3, Quat)> = pa.iter().zip(pb.iter()).map(|(x, y)| (x.0.lerp(y.0, e), x.1.slerp(y.1, e).normalize())).collect();
        let spline_amount = if ease == Ease::Spline { 1.0 } else { self.smoothing };
        if spline_amount > 0.0 && ease != Ease::Hold {
            let idx = keys.iter().position(|k| k.0 == a.0).unwrap_or(0);
            let prev = if idx > 0 { pad(keys[idx - 1].1) } else { pa.clone() };
            let next = if idx + 2 < keys.len() { pad(keys[idx + 2].1) } else { pb.clone() };
            let ts = if ease == Ease::Spline { t } else { e };
            for i in 0..n {
                let sp = catmull(prev[i].0, pa[i].0, pb[i].0, next[i].0, ts);
                let sq = quat_catmull(prev[i].1, pa[i].1, pb[i].1, next[i].1, ts);
                out[i].0 = out[i].0.lerp(sp, spline_amount);
                out[i].1 = out[i].1.slerp(sq, spline_amount).normalize();
            }
        }
        out
    }

    /// Crossfade: make frames `from`..`to` blend smoothly from the pose at `from` into the pose at `to`.
    pub fn crossfade(&mut self, from: u32, to: u32, ease: Ease, bind: &[(Vec3, Quat)]) {
        if to <= from {
            return;
        }
        let pa = self.sample(from as f32, bind);
        let pb = self.sample(to as f32, bind);
        let inner: Vec<u32> = self.keys.range(from + 1..to).map(|(k, _)| *k).collect();
        for k in inner {
            self.keys.remove(&k);
        }
        let next_ease = self.keys.get(&to).map(|k| k.ease).unwrap_or(Ease::Smooth);
        self.keys.insert(from, Keyframe { pose: pa, ease });
        self.keys.insert(to, Keyframe { pose: pb, ease: next_ease });
    }

    // ---------------- save / load ----------------

    pub fn to_text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "// Golden Stunstick animation");
        let _ = writeln!(s, "version 1");
        let _ = writeln!(s, "name {}", self.name.clone().unwrap_or_default());
        let _ = writeln!(s, "qc {}", self.qc_path.display());
        let _ = writeln!(s, "fps {}", self.fps);
        let _ = writeln!(s, "frames {}", self.frames);
        let _ = writeln!(s, "loop {}", self.looping as u8);
        let _ = writeln!(s, "smoothing {}", self.smoothing);
        let _ = writeln!(s, "bones {}", self.bones.len());
        for (n, p) in &self.bones {
            let _ = writeln!(s, "bone \"{n}\" \"{p}\"");
        }
        for (f, k) in &self.keys {
            let _ = writeln!(s, "key {f} {}", k.ease.name());
            for (i, (p, q)) in k.pose.iter().enumerate() {
                let _ = writeln!(s, "  {i} {} {} {} {} {} {} {}", p.x, p.y, p.z, q.x, q.y, q.z, q.w);
            }
            let _ = writeln!(s, "endkey");
        }
        s
    }

    pub fn from_text(text: &str) -> Result<AnimDoc, String> {
        let mut d = AnimDoc { name: None, qc_path: PathBuf::new(), fps: 30.0, frames: 30, looping: false, smoothing: 0.0, bones: Vec::new(), keys: BTreeMap::new() };
        let mut lines = text.lines().enumerate();
        let mut saw_version = false;
        while let Some((ln, line)) = lines.next() {
            let l = line.trim();
            if l.is_empty() || l.starts_with("//") {
                continue;
            }
            let (k, v) = l.split_once(' ').unwrap_or((l, ""));
            let v = v.trim();
            match k {
                "version" => saw_version = true,
                "name" => d.name = (!v.is_empty()).then(|| v.to_string()),
                "qc" => d.qc_path = PathBuf::from(v),
                "fps" => d.fps = v.parse().unwrap_or(30.0),
                "frames" => d.frames = v.parse().unwrap_or(30),
                "loop" => d.looping = v == "1",
                "smoothing" => d.smoothing = v.parse().unwrap_or(0.0),
                "bones" => {}
                "bone" => {
                    let parts: Vec<&str> = v.split('"').filter(|s| !s.trim().is_empty()).collect();
                    let name = parts.first().copied().unwrap_or("").to_string();
                    let parent = parts.get(1).copied().unwrap_or("").to_string();
                    d.bones.push((name, parent));
                }
                "key" => {
                    let mut it = v.split_whitespace();
                    let f: u32 = it.next().and_then(|x| x.parse().ok()).ok_or(format!("line {}: bad key frame", ln + 1))?;
                    let ease = Ease::parse(it.next().unwrap_or("linear"));
                    let mut pose = vec![(Vec3::ZERO, Quat::IDENTITY); d.bones.len()];
                    for (ln2, line2) in lines.by_ref() {
                        let l2 = line2.trim();
                        if l2 == "endkey" {
                            break;
                        }
                        let n: Vec<f32> = l2.split_whitespace().filter_map(|x| x.parse().ok()).collect();
                        if n.len() != 8 {
                            return Err(format!("line {}: expected 'bone px py pz qx qy qz qw'", ln2 + 1));
                        }
                        let bi = n[0] as usize;
                        if bi >= pose.len() {
                            pose.resize(bi + 1, (Vec3::ZERO, Quat::IDENTITY));
                        }
                        pose[bi] = (Vec3::new(n[1], n[2], n[3]), Quat::from_xyzw(n[4], n[5], n[6], n[7]).normalize());
                    }
                    d.keys.insert(f, Keyframe { pose, ease });
                }
                _ => {}
            }
        }
        if !saw_version {
            return Err("not a Golden Stunstick animation file".into());
        }
        Ok(d)
    }

    /// Differences between the animation's skeleton and a rig.
    pub fn rig_differences(&self, rig: &Rig) -> Vec<String> {
        let sig = rig.bone_signature();
        let mut d = Vec::new();
        if sig.len() != self.bones.len() {
            d.push(format!("bone count: animation has {}, model has {}", self.bones.len(), sig.len()));
        }
        for (i, (n, p)) in self.bones.iter().enumerate() {
            match sig.get(i) {
                Some((sn, sp)) if !sn.eq_ignore_ascii_case(n) => d.push(format!("bone {i}: '{n}' in the animation, '{sn}' in the model")),
                Some((_, sp)) if !sp.eq_ignore_ascii_case(p) => d.push(format!("bone '{n}': parent '{p}' in the animation, '{sp}' in the model")),
                None => d.push(format!("bone '{n}' does not exist in the model")),
                _ => {}
            }
        }
        d
    }

    // ---------------- export ----------------

    pub fn export_smd(&self, rig: &Rig) -> String {
        let mut smd = Smd { nodes: rig.bones.clone(), frames: Vec::new(), triangles: Vec::new() };
        for f in 0..self.frames.max(1) {
            let pose = self.sample(f as f32, &rig.bind);
            smd.frames.push(pose.iter().map(|(p, q)| (*p, quat_to_euler(*q))).collect());
        }
        smd.write()
    }

    /// KeyValues2 DMX ("format model 1", the version Source SDK 2013 / TF2 studiomdl reads
    /// natively) with a skeleton and one DmeChannelsClip. Times are integer ticks of 1/10000 s.
    pub fn export_dmx(&self, rig: &Rig) -> String {
        let mut next_id = 0u64;
        let mut id = || {
            next_id += 1;
            format!("{:08x}-{:04x}-{:04x}-{:04x}-{:012x}", 0x6a5d0000u64 + next_id, 0x60, 0x4000, 0x8000, next_id)
        };
        let name = self.name.clone().unwrap_or_else(|| format!("{}_animation", rig.qc_name));
        let n = rig.bones.len();
        let joint_ids: Vec<String> = (0..n).map(|_| id()).collect();
        let xf_ids: Vec<String> = (0..n).map(|_| id()).collect();
        let (root_id, model_id, model_xf, list_id, clip_id, tf_id, base_id) = (id(), id(), id(), id(), id(), id(), id());
        let base_xf_ids: Vec<String> = (0..n).map(|_| id()).collect();
        let v3 = |v: Vec3| format!("{} {} {}", v.x, v.y, v.z);
        let q4 = |q: Quat| format!("{} {} {} {}", q.x, q.y, q.z, q.w);
        let mut s = String::from("<!-- dmx encoding keyvalues2 1 format model 1 -->\n");
        let _ = writeln!(s, "\"DmElement\"\n{{\n\t\"id\" \"elementid\" \"{root_id}\"\n\t\"name\" \"string\" \"root\"\n\t\"skeleton\" \"element\" \"{model_id}\"\n\t\"animationList\" \"element\" \"{list_id}\"\n}}\n");
        // model
        let roots: Vec<usize> = (0..n).filter(|&i| rig.bones[i].parent < 0).collect();
        let _ = writeln!(s, "\"DmeModel\"\n{{\n\t\"id\" \"elementid\" \"{model_id}\"\n\t\"name\" \"string\" \"{}\"", rig.qc_name);
        let _ = writeln!(s, "\t\"transform\" \"element\" \"{model_xf}\"\n\t\"shape\" \"element\" \"\"\n\t\"visible\" \"bool\" \"1\"");
        let _ = writeln!(s, "\t\"children\" \"element_array\"\n\t[\n{}\n\t]", roots.iter().map(|&r| format!("\t\t\"element\" \"{}\"", joint_ids[r])).collect::<Vec<_>>().join(",\n"));
        let _ = writeln!(s, "\t\"jointList\" \"element_array\"\n\t[\n{}\n\t]", joint_ids.iter().map(|j| format!("\t\t\"element\" \"{j}\"")).collect::<Vec<_>>().join(",\n"));
        let _ = writeln!(s, "\t\"baseStates\" \"element_array\"\n\t[\n\t\t\"element\" \"{base_id}\"\n\t]\n}}\n");
        let _ = writeln!(s, "\"DmeTransform\"\n{{\n\t\"id\" \"elementid\" \"{model_xf}\"\n\t\"name\" \"string\" \"{}\"\n\t\"position\" \"vector3\" \"0 0 0\"\n\t\"orientation\" \"quaternion\" \"0 0 0 1\"\n}}\n", rig.qc_name);
        // joints + transforms
        for i in 0..n {
            let kids: Vec<String> = rig.children(i).iter().map(|&c| format!("\t\t\"element\" \"{}\"", joint_ids[c])).collect();
            let _ = writeln!(s, "\"DmeJoint\"\n{{\n\t\"id\" \"elementid\" \"{}\"\n\t\"name\" \"string\" \"{}\"\n\t\"transform\" \"element\" \"{}\"\n\t\"shape\" \"element\" \"\"\n\t\"visible\" \"bool\" \"1\"", joint_ids[i], rig.bones[i].name, xf_ids[i]);
            let _ = writeln!(s, "\t\"children\" \"element_array\"\n\t[\n{}\n\t]\n}}\n", kids.join(",\n"));
            let (p, q) = rig.bind[i];
            let _ = writeln!(s, "\"DmeTransform\"\n{{\n\t\"id\" \"elementid\" \"{}\"\n\t\"name\" \"string\" \"{}\"\n\t\"position\" \"vector3\" \"{}\"\n\t\"orientation\" \"quaternion\" \"{}\"\n}}\n", xf_ids[i], rig.bones[i].name, v3(p), q4(q));
        }
        // base state
        let _ = writeln!(s, "\"DmeTransformList\"\n{{\n\t\"id\" \"elementid\" \"{base_id}\"\n\t\"name\" \"string\" \"bind\"\n\t\"transforms\" \"element_array\"\n\t[\n{}\n\t]\n}}\n", base_xf_ids.iter().map(|j| format!("\t\t\"element\" \"{j}\"")).collect::<Vec<_>>().join(",\n"));
        for i in 0..n {
            let (p, q) = rig.bind[i];
            let _ = writeln!(s, "\"DmeTransform\"\n{{\n\t\"id\" \"elementid\" \"{}\"\n\t\"name\" \"string\" \"{}\"\n\t\"position\" \"vector3\" \"{}\"\n\t\"orientation\" \"quaternion\" \"{}\"\n}}\n", base_xf_ids[i], rig.bones[i].name, v3(p), q4(q));
        }
        // animation
        let frames: Vec<Vec<(Vec3, Quat)>> = (0..self.frames.max(1)).map(|f| self.sample(f as f32, &rig.bind)).collect();
        let ticks = |f: usize| ((f as f64 / self.fps.max(1.0) as f64) * 10000.0).round() as i64;
        let times: Vec<String> = (0..frames.len()).map(|f| format!("\t\t\"{}\"", ticks(f))).collect();
        let duration = ticks(frames.len().saturating_sub(1));
        let mut channels = Vec::new();
        let mut chan_text = String::new();
        for i in 0..n {
            for (kind, attr) in [("p", "position"), ("o", "orientation")] {
                let (cid, lid, llid) = (id(), id(), id());
                channels.push(format!("\t\t\"element\" \"{cid}\""));
                let (log_type, layer_type, arr_type) = if kind == "p" { ("DmeVector3Log", "DmeVector3LogLayer", "vector3_array") } else { ("DmeQuaternionLog", "DmeQuaternionLogLayer", "quaternion_array") };
                let values: Vec<String> = frames.iter().map(|fr| format!("\t\t\"{}\"", if kind == "p" { v3(fr[i].0) } else { q4(fr[i].1) })).collect();
                let _ = writeln!(
                    chan_text,
                    "\"DmeChannel\"\n{{\n\t\"id\" \"elementid\" \"{cid}\"\n\t\"name\" \"string\" \"{}_{kind}\"\n\t\"fromElement\" \"element\" \"{}\"\n\t\"fromAttribute\" \"string\" \"{attr}\"\n\t\"fromIndex\" \"int\" \"0\"\n\t\"toElement\" \"element\" \"{}\"\n\t\"toAttribute\" \"string\" \"{attr}\"\n\t\"toIndex\" \"int\" \"0\"\n\t\"mode\" \"int\" \"3\"\n\t\"log\" \"element\" \"{lid}\"\n}}\n",
                    rig.bones[i].name, xf_ids[i], xf_ids[i]
                );
                let _ = writeln!(
                    chan_text,
                    "\"{log_type}\"\n{{\n\t\"id\" \"elementid\" \"{lid}\"\n\t\"name\" \"string\" \"{}_{kind}\"\n\t\"layers\" \"element_array\"\n\t[\n\t\t\"element\" \"{llid}\"\n\t]\n\t\"curveinfo\" \"element\" \"\"\n\t\"usedefaultvalue\" \"bool\" \"0\"\n}}\n",
                    rig.bones[i].name
                );
                let _ = writeln!(
                    chan_text,
                    "\"{layer_type}\"\n{{\n\t\"id\" \"elementid\" \"{llid}\"\n\t\"name\" \"string\" \"{}_{kind}\"\n\t\"times\" \"int_array\"\n\t[\n{}\n\t]\n\t\"curvetypes\" \"int_array\"\n\t[\n\t]\n\t\"values\" \"{arr_type}\"\n\t[\n{}\n\t]\n}}\n",
                    rig.bones[i].name,
                    times.join(",\n"),
                    values.join(",\n")
                );
            }
        }        let _ = writeln!(s, "\"DmeAnimationList\"\n{{\n\t\"id\" \"elementid\" \"{list_id}\"\n\t\"name\" \"string\" \"animationList\"\n\t\"animations\" \"element_array\"\n\t[\n\t\t\"element\" \"{clip_id}\"\n\t]\n}}\n");
        let _ = writeln!(s, "\"DmeChannelsClip\"\n{{\n\t\"id\" \"elementid\" \"{clip_id}\"\n\t\"name\" \"string\" \"{name}\"\n\t\"timeFrame\" \"element\" \"{tf_id}\"\n\t\"color\" \"color\" \"0 0 0 0\"\n\t\"text\" \"string\" \"\"\n\t\"mute\" \"bool\" \"0\"\n\t\"trackGroups\" \"element_array\"\n\t[\n\t]\n\t\"displayScale\" \"float\" \"1\"\n\t\"frameRate\" \"int\" \"{}\"", self.fps.round() as i32);
        let _ = writeln!(s, "\t\"channels\" \"element_array\"\n\t[\n{}\n\t]\n}}\n", channels.join(",\n"));
        let _ = writeln!(s, "\"DmeTimeFrame\"\n{{\n\t\"id\" \"elementid\" \"{tf_id}\"\n\t\"name\" \"string\" \"timeframe\"\n\t\"startTime\" \"int\" \"0\"\n\t\"durationTime\" \"int\" \"{duration}\"\n\t\"offsetTime\" \"int\" \"0\"\n\t\"scale\" \"float\" \"1\"\n}}\n");
        s.push_str(&chan_text);
        s
    }
}

/// Next free "<qc>_animation_N" name in a folder.
pub fn next_auto_name(dir: &Path, qc_name: &str) -> String {
    let mut n = 1;
    loop {
        let name = format!("{qc_name}_animation_{n}");
        if !dir.join(format!("{name}.txt")).exists() {
            return name;
        }
        n += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn rig() -> Rig {
        Rig {
            qc_path: PathBuf::from("x.qc"),
            qc_name: "x".into(),
            bones: vec![
                SmdNode { name: "root".into(), parent: -1 },
                SmdNode { name: "upperarm".into(), parent: 0 },
                SmdNode { name: "forearm".into(), parent: 1 },
                SmdNode { name: "hand".into(), parent: 2 },
            ],
            bind: vec![(Vec3::ZERO, Quat::IDENTITY), (Vec3::new(0.0, 0.0, 10.0), Quat::IDENTITY), (Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY), (Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY)],
            meshes: vec![],
            cdmaterials: vec![],
            warnings: vec![],
        }
    }

    #[test]
    fn roundtrip_text_and_sample() {
        let r = rig();
        let mut d = AnimDoc::new(&r);
        let mut p = r.bind.clone();
        p[1].1 = Quat::from_rotation_z(1.0);
        d.keys.insert(10, Keyframe { pose: p, ease: Ease::Linear });
        d.name = Some("x_animation_1".into());
        let t = d.to_text();
        let d2 = AnimDoc::from_text(&t).unwrap();
        assert_eq!(d2.keys.len(), 2);
        let mid = d2.sample(5.0, &r.bind);
        let ang = mid[1].1.angle_between(Quat::IDENTITY);
        assert!((ang - 0.5 * 0.5 * (3.0 - 1.0) * 1.0).abs() < 0.2, "smooth ease at midpoint {ang}");
        assert!(d2.rig_differences(&r).is_empty());
        assert!(d2.export_smd(&r).contains("time 29"));
        assert!(d2.export_dmx(&r).contains("DmeChannelsClip"));
    }

    #[test]
    fn ik_reaches_target() {
        let r = rig();
        let chains = detect_chains(&r);
        assert_eq!(chains.len(), 1, "{chains:?}");
        let mut pose = r.bind.clone();
        let target = Vec3::new(8.0, 6.0, 14.0);
        solve_ik(&r, &mut pose, &chains[0].bones, target);
        let w = r.world(&pose);
        let e: Vec3 = w[3].translation.into();
        assert!((e - target).length() < 0.5, "{e:?}");
    }
}

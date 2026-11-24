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
    /// the QC's `eyeball` lines, for drawing the iris
    pub eyeballs: Vec<RigEyeball>,
}

/// A QC `eyeball`: the eye's centre and axes in its bone's frame.
#[derive(Clone, Debug)]
pub struct RigEyeball {
    /// material of the eyeball mesh (SMD triangle material, without extension)
    pub material: String,
    pub bone: usize,
    pub org: Vec3,
    pub forward: Vec3,
    pub up: Vec3,
    /// 1 / iris size in units (as studiomdl stores it)
    pub iris_scale: f32,
    pub radius: f32,
}

/// Read the `eyeball` lines inside the QC's `$model` blocks:
/// `eyeball <name> <bone> <x> <y> <z> <material> <diameter> <angle> <iris material> <iris size>`.
/// The position is in model space; studiomdl takes the eye to look along the model's forward
/// axis, which this works out from where the eyes sit relative to their bone (in front of it).
fn qc_eyeballs(q: &Qc, bones: &[SmdNode], bind: &[(Vec3, Quat)]) -> Vec<RigEyeball> {
    let world = world_of(bones, bind);
    let mut raw: Vec<(String, usize, Vec3, f32, f32)> = Vec::new();
    for c in q.all("$model") {
        let a = &c.args;
        let mut i = 0;
        while i < a.len() {
            if !a[i].quoted && a[i].text.eq_ignore_ascii_case("eyeball") && i + 10 < a.len() {
                let num = |k: usize| a[i + k].text.parse::<f32>().ok();
                let bone = bones.iter().position(|b| b.name.eq_ignore_ascii_case(&a[i + 2].text));
                if let (Some(bone), Some(x), Some(y), Some(z), Some(d), Some(iris)) = (bone, num(3), num(4), num(5), num(7), num(10)) {
                    let mat = a[i + 6].text.trim_end_matches(".vmt").to_string();
                    raw.push((mat, bone, Vec3::new(x, y, z), d * 0.5, if iris.abs() > 1e-6 { 1.0 / iris } else { 0.0 }));
                }
                i += 11;
            } else {
                i += 1;
            }
        }
    }
    if raw.is_empty() {
        return Vec::new();
    }
    // forward: horizontal direction from the eyes' bone (head) to the middle of the eyes
    let mid = raw.iter().map(|r| r.2).sum::<Vec3>() / raw.len() as f32;
    let head = Vec3::from(world[raw[0].1].translation);
    let mut fwd = mid - head;
    fwd.z = 0.0;
    let forward = if fwd.length() > 1e-3 { fwd.normalize() } else { -Vec3::Y };
    raw.into_iter()
        .map(|(material, bone, pos, radius, iris_scale)| {
            let inv = world[bone].inverse();
            RigEyeball { material, bone, org: inv.transform_point3(pos), forward: inv.transform_vector3(forward), up: inv.transform_vector3(Vec3::Z), iris_scale, radius }
        })
        .collect()
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
        let eyeballs = qc_eyeballs(&q, &bones, &bind);
        Ok(Rig { qc_path: qc_path.to_path_buf(), qc_name, bones, bind, meshes, cdmaterials: q.cdmaterials(), warnings, eyeballs })
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
    // a "wrist" helper next to a real "hand" on the same forearm (Valve biped) is not a limb
    let lower = |c: &IkChain| c.bones.get(c.bones.len().wrapping_sub(2)).copied();
    let hands: Vec<Option<usize>> = out.iter().filter(|c| has_any(&rig.bones[*c.bones.last().unwrap()].name, &["hand"])).map(lower).collect();
    out.retain(|c| {
        let eff = &rig.bones[*c.bones.last().unwrap()].name;
        !(has_any(eff, &["wrist"]) && !has_any(eff, &["hand"]) && hands.contains(&lower(c)))
    });
    out
}

fn rot_of(a: &Affine3A) -> Quat {
    a.to_scale_rotation_translation().1.normalize()
}

fn any_perpendicular(v: Vec3) -> Vec3 {
    let o = if v.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    v.cross(o).normalize_or(Vec3::Z)
}

/// Rotation taking the frame (x1 along, z1 hinge) onto (x2 along, z2 hinge).
fn frame_rotation(x1: Vec3, z1: Vec3, x2: Vec3, z2: Vec3) -> Quat {
    let basis = |x: Vec3, z: Vec3| -> Option<glam::Mat3> {
        let x = x.normalize_or_zero();
        let z = (z - x * z.dot(x)).normalize_or_zero();
        if x == Vec3::ZERO || z == Vec3::ZERO {
            return None;
        }
        Some(glam::Mat3::from_cols(x, z.cross(x), z))
    };
    match (basis(x1, z1), basis(x2, z2)) {
        (Some(a), Some(b)) => Quat::from_mat3(&(b * a.transpose())).normalize(),
        _ => Quat::from_rotation_arc(x1.normalize_or(Vec3::X), x2.normalize_or(Vec3::X)),
    }
}

/// Set bone `i`'s world rotation (its parent's world transform is `w[parent]`).
fn set_world_rot(rig: &Rig, pose: &mut [(Vec3, Quat)], w: &[Affine3A], i: usize, world_rot: Quat) {
    let pi = rig.bones[i].parent;
    let parent_rot = if pi >= 0 { rot_of(&w[pi as usize]) } else { Quat::IDENTITY };
    pose[i].1 = (parent_rot.inverse() * world_rot).normalize();
}

/// The limb's bend set-up, taken from the bind (reference) pose:
/// * `pole` – the direction the elbow/knee points, in the frame of the limb root's parent
///   (clavicle / pelvis), so it follows the body but not the limb;
/// * `hinge_upper` / `hinge_lower` – the bend axis in the upper / lower bone's local frame.
struct LimbSetup {
    pole: Vec3,
    hinge_upper: Vec3,
    hinge_lower: Vec3,
}

fn limb_setup(rig: &Rig, ia: usize, ib: usize, ic: usize) -> LimbSetup {
    let wb = rig.world(&rig.bind);
    let (a, b, c) = (Vec3::from(wb[ia].translation), Vec3::from(wb[ib].translation), Vec3::from(wb[ic].translation));
    let d = (c - a).normalize_or(Vec3::Z);
    let off = (b - a) - d * (b - a).dot(d);
    let len = (b - a).length() + (c - b).length();
    let pole_world = if off.length() > len * 0.01 {
        off.normalize()
    } else {
        // nearly straight in the reference pose: knees point where the toes do, elbows the
        // other way
        let forward = rig
            .bones
            .iter()
            .enumerate()
            .find(|(_, n)| has_any(&n.name, &["foot", "ankle"]) && !has_any(&n.name, &["toe"]))
            .and_then(|(f, _)| rig.children(f).first().map(|&t| Vec3::from(wb[t].translation) - Vec3::from(wb[f].translation)))
            .map(|v| v - d * v.dot(d))
            .filter(|v| v.length() > 1e-4)
            .map(|v| v.normalize());
        let is_leg = has_any(&rig.bones[ic].name, &["foot", "ankle"]);
        match forward {
            Some(f) if is_leg => f,
            Some(f) => -f,
            None => any_perpendicular(d),
        }
    };
    let hinge = d.cross(pole_world).normalize_or(any_perpendicular(d));
    let pa = rig.bones[ia].parent;
    let root_rot = if pa >= 0 { rot_of(&wb[pa as usize]) } else { Quat::IDENTITY };
    LimbSetup {
        pole: root_rot.inverse() * pole_world,
        hinge_upper: rot_of(&wb[ia]).inverse() * hinge,
        hinge_lower: rot_of(&wb[ib]).inverse() * hinge,
    }
}

/// The natural bend direction (unit, perpendicular to `dir`) of a limb aimed along `dir`:
/// the reference pose's pole, following the body (clavicle / pelvis). `None` when `dir`
/// points straight along the pole.
fn natural_bend(rig: &Rig, w: &[Affine3A], setup: &LimbSetup, ia: usize, dir: Vec3) -> Option<Vec3> {
    let pa = rig.bones[ia].parent;
    let root_rot = if pa >= 0 { rot_of(&w[pa as usize]) } else { Quat::IDENTITY };
    let pole = root_rot * setup.pole;
    let p = pole - dir * pole.dot(dir);
    (p.length() > 1e-4).then(|| p.normalize())
}

/// Easy-rig IK (like Source Filmmaker's): an analytic two-bone solve on the last three bones
/// of `chain` (upper arm / forearm / hand, or thigh / calf / foot). The shoulder or hip stays
/// put, the hand/foot goes exactly to `target` (clamped to the limb's reach) and keeps its
/// world rotation. The elbow/knee bends the natural way (elbows back, knees forward, from the
/// reference pose) turned by however far it is currently swung from that (see [`swing_limb`]),
/// so a knee the animator pointed outwards stays pointed outwards while the foot moves.
pub fn solve_ik(rig: &Rig, pose: &mut [(Vec3, Quat)], chain: &[usize], target: Vec3) {
    let n = chain.len();
    if n < 3 {
        return;
    }
    let (ia, ib, ic) = (chain[n - 3], chain[n - 2], chain[n - 1]);
    let setup = limb_setup(rig, ia, ib, ic);
    let w = rig.world(pose);
    let (a, b, c) = (Vec3::from(w[ia].translation), Vec3::from(w[ib].translation), Vec3::from(w[ic].translation));
    // current swing: signed angle from the natural bend to the actual one, about the limb axis
    let cur_dir = (c - a).normalize_or(Vec3::Z);
    let cur_bend = (b - a) - cur_dir * (b - a).dot(cur_dir);
    let swing = match natural_bend(rig, &w, &setup, ia, cur_dir) {
        Some(nat) if cur_bend.length() > (b - a).length() * 1e-3 => {
            let cb = cur_bend.normalize();
            nat.cross(cb).dot(cur_dir).atan2(nat.dot(cb))
        }
        _ => 0.0,
    };
    let to_t = target - a;
    let dir = if to_t.length() > 1e-5 { to_t.normalize() } else { cur_dir };
    let bend = match natural_bend(rig, &w, &setup, ia, dir) {
        Some(nat) => Quat::from_axis_angle(dir, swing) * nat,
        // target straight along the pole: keep the current bend side
        None => {
            let p = (b - a) - dir * (b - a).dot(dir);
            if p.length() > 1e-4 { p.normalize() } else { any_perpendicular(dir) }
        }
    };
    solve_two_bone(rig, pose, &setup, (ia, ib, ic), target, bend);
}

/// Easy-rig elbow/knee control (Source Filmmaker's pole vector): swing the elbow/knee of
/// `chain` towards `pole_target` around the shoulder-to-hand (hip-to-foot) line, keeping the
/// hand/foot exactly where it is.
pub fn swing_limb(rig: &Rig, pose: &mut [(Vec3, Quat)], chain: &[usize], pole_target: Vec3) {
    let n = chain.len();
    if n < 3 {
        return;
    }
    let (ia, ib, ic) = (chain[n - 3], chain[n - 2], chain[n - 1]);
    let setup = limb_setup(rig, ia, ib, ic);
    let w = rig.world(pose);
    let (a, c) = (Vec3::from(w[ia].translation), Vec3::from(w[ic].translation));
    let dir = (c - a).normalize_or(Vec3::Z);
    let p = (pole_target - a) - dir * (pole_target - a).dot(dir);
    if p.length() < 1e-4 {
        return;
    }
    solve_two_bone(rig, pose, &setup, (ia, ib, ic), c, p.normalize());
}

/// Two-bone solve: put the end of `ic` at `target` with the middle joint bending towards
/// `bend` (unit, perpendicular to the root-to-target line); the end bone keeps its world rotation.
fn solve_two_bone(rig: &Rig, pose: &mut [(Vec3, Quat)], setup: &LimbSetup, (ia, ib, ic): (usize, usize, usize), target: Vec3, bend: Vec3) {
    let w = rig.world(pose);
    let eff_rot = rot_of(&w[ic]);
    let (a, b, c) = (Vec3::from(w[ia].translation), Vec3::from(w[ib].translation), Vec3::from(w[ic].translation));
    let (l1, l2) = ((b - a).length(), (c - b).length());
    if l1 < 1e-4 || l2 < 1e-4 {
        return;
    }
    let to_t = target - a;
    let dir = if to_t.length() > 1e-5 { to_t.normalize() } else { (c - a).normalize_or(Vec3::Z) };
    let dist = to_t.length().clamp((l1 - l2).abs() + 1e-3, (l1 + l2) * 0.9999);
    let p = (bend - dir * bend.dot(dir)).normalize_or(any_perpendicular(dir));
    let x = (l1 * l1 - l2 * l2 + dist * dist) / (2.0 * dist);
    let y = (l1 * l1 - x * x).max(0.0).sqrt();
    let elbow = a + dir * x + p * y;
    let goal = a + dir * dist;
    let hinge = dir.cross(p);
    // upper bone: along a->elbow, hinge axis onto the bend plane's normal
    let up_rot = rot_of(&w[ia]);
    let r = frame_rotation(b - a, up_rot * setup.hinge_upper, elbow - a, hinge);
    set_world_rot(rig, pose, &w, ia, (r * up_rot).normalize());
    // lower bone: along elbow->goal, same hinge
    let w = rig.world(pose);
    let (b2, c2) = (Vec3::from(w[ib].translation), Vec3::from(w[ic].translation));
    let lo_rot = rot_of(&w[ib]);
    let r = frame_rotation(c2 - b2, lo_rot * setup.hinge_lower, goal - b2, hinge);
    set_world_rot(rig, pose, &w, ib, (r * lo_rot).normalize());
    // the hand/foot keeps its world orientation
    let w = rig.world(pose);
    set_world_rot(rig, pose, &w, ic, eff_rot);
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

// ---------------------------------------------------------------------------
// SMD animation import
// ---------------------------------------------------------------------------

/// How an SMD animation's skeleton maps onto a rig.
pub struct SmdMatch {
    /// rig bone -> SMD node
    pub map: Vec<Option<usize>>,
    /// rig bones whose parent differs from the SMD's (converted through world space)
    pub reparented: Vec<bool>,
    /// the SMD comes from a skeleton with other bone lengths: use the rig's own bone
    /// lengths (rotations only) and scale the root motion
    pub other_proportions: bool,
    pub root_scale: f32,
    /// human-readable differences (empty when the skeletons match)
    pub diffs: Vec<String>,
}

fn name_list(v: &[String]) -> String {
    let mut s = v.iter().take(12).map(|n| format!("'{n}'")).collect::<Vec<_>>().join(", ");
    if v.len() > 12 {
        let _ = write!(s, " and {} more", v.len() - 12);
    }
    s
}

/// Compare an SMD's skeleton with the rig (bones are matched by name, ignoring case).
pub fn match_smd(smd: &Smd, rig: &Rig) -> SmdMatch {
    let find = |name: &str| smd.nodes.iter().position(|n| n.name.eq_ignore_ascii_case(name));
    let map: Vec<Option<usize>> = rig.bones.iter().map(|b| find(&b.name)).collect();
    let smd_parent = |i: usize| -> String { smd.nodes[i].parent.try_into().ok().and_then(|p: usize| smd.nodes.get(p)).map(|n| n.name.clone()).unwrap_or_default() };
    let rig_parent = |r: usize| -> String { rig.bones[r].parent.try_into().ok().and_then(|p: usize| rig.bones.get(p)).map(|n| n.name.clone()).unwrap_or_default() };
    let reparented: Vec<bool> = (0..rig.bones.len()).map(|r| map[r].map(|i| !smd_parent(i).eq_ignore_ascii_case(&rig_parent(r))).unwrap_or(false)).collect();
    let mut diffs = Vec::new();
    let missing: Vec<String> = (0..rig.bones.len()).filter(|&r| map[r].is_none()).map(|r| rig.bones[r].name.clone()).collect();
    let extra: Vec<String> = smd.nodes.iter().filter(|n| !rig.bones.iter().any(|b| b.name.eq_ignore_ascii_case(&n.name))).map(|n| n.name.clone()).collect();
    if map.iter().all(|m| m.is_none()) {
        diffs.push("None of the SMD's bones exist in this model.".into());
    }
    if !missing.is_empty() {
        diffs.push(format!("{} bone(s) of the model are not in the SMD; they keep the model's reference pose: {}", missing.len(), name_list(&missing)));
    }
    if !extra.is_empty() {
        diffs.push(format!("{} bone(s) in the SMD are not in the model and are ignored: {}", extra.len(), name_list(&extra)));
    }
    for r in (0..rig.bones.len()).filter(|&r| reparented[r]) {
        let i = map[r].unwrap_or(0);
        diffs.push(format!("'{}' has parent '{}' in the SMD but '{}' in the model; its world position is kept", rig.bones[r].name, smd_parent(i), rig_parent(r)));
    }
    // bone lengths (first frame vs. the reference pose); animated translations are rare, so
    // a different length on many bones means a differently proportioned skeleton
    let mut ratios = Vec::new();
    let mut differing = 0;
    let mut measured = 0;
    if let Some(f0) = smd.frames.first() {
        for r in 0..rig.bones.len() {
            let (Some(i), true) = (map[r], rig.bones[r].parent >= 0) else { continue };
            let bind_len = rig.bind[r].0.length();
            let Some(&(p, _)) = f0.get(i) else { continue };
            if bind_len < 0.5 || reparented[r] {
                continue;
            }
            measured += 1;
            let ratio = p.length() / bind_len;
            ratios.push(ratio);
            if (ratio - 1.0).abs() > 0.05 {
                differing += 1;
            }
        }
    }
    let other_proportions = measured > 0 && differing * 10 > measured * 3;
    let root_scale = if other_proportions {
        ratios.sort_by(f32::total_cmp);
        let median = ratios[ratios.len() / 2];
        if median > 1e-3 { 1.0 / median } else { 1.0 }
    } else {
        1.0
    };
    if other_proportions {
        diffs.push(format!(
            "{differing} of {measured} bones have different lengths: the SMD comes from a differently proportioned skeleton. Its rotations are used with this model's bone lengths, and root motion is scaled by {root_scale:.2}."
        ));
    }
    SmdMatch { map, reparented, other_proportions, root_scale, diffs }
}

/// Turn an SMD animation into an animation document for `rig` (one linear key per SMD frame).
pub fn import_smd(smd: &Smd, rig: &Rig, m: &SmdMatch, fps: f32, looping: bool) -> AnimDoc {
    let n = rig.bones.len();
    let mut keys = BTreeMap::new();
    for (f, frame) in smd.frames.iter().enumerate() {
        // the SMD's local pose, adjusted to the rig's proportions if needed
        let mut local: Vec<(Vec3, Quat)> = smd.nodes.iter().enumerate().map(|(i, _)| frame.get(i).map(|(p, r)| (*p, euler_to_quat(*r))).unwrap_or((Vec3::ZERO, Quat::IDENTITY))).collect();
        if m.other_proportions {
            for r in 0..n {
                if let Some(i) = m.map[r] {
                    if smd.nodes[i].parent >= 0 {
                        if !m.reparented[r] {
                            local[i].0 = rig.bind[r].0;
                        }
                    } else {
                        local[i].0 *= m.root_scale;
                    }
                }
            }
        }
        let smd_world = world_of(&smd.nodes, &local);
        let mut pose: Vec<(Vec3, Quat)> = Vec::with_capacity(n);
        let mut rig_world: Vec<Affine3A> = Vec::with_capacity(n);
        for r in 0..n {
            let parent = rig.bones[r].parent;
            let parent_world = if parent >= 0 && (parent as usize) < rig_world.len() { rig_world[parent as usize] } else { Affine3A::IDENTITY };
            let l = match m.map[r] {
                Some(i) if !m.reparented[r] => local[i],
                Some(i) => {
                    // different parent: keep the bone where the SMD puts it in the world
                    let (_, q, p) = (parent_world.inverse() * smd_world[i]).to_scale_rotation_translation();
                    (if m.other_proportions && parent >= 0 { rig.bind[r].0 } else { p }, q.normalize())
                }
                None => rig.bind[r],
            };
            rig_world.push(parent_world * Affine3A::from_rotation_translation(l.1, l.0));
            pose.push(l);
        }
        keys.insert(f as u32, Keyframe { pose, ease: Ease::Linear });
    }
    if keys.is_empty() {
        keys.insert(0, Keyframe { pose: rig.bind.clone(), ease: Ease::Linear });
    }
    AnimDoc { name: None, qc_path: rig.qc_path.clone(), fps: fps.max(1.0), frames: (smd.frames.len() as u32).max(1), looping, smoothing: 0.0, bones: rig.bone_signature(), keys }
}

/// Largest local rotation (degrees) a key-reduced animation may differ from the source by.
pub const REDUCE_ROT_TOL_DEG: f32 = 0.5;
/// Largest bone position error (units, world space) a key-reduced animation may have.
pub const REDUCE_POS_TOL: f32 = 0.05;

/// Source poses for [`reduce_keys`] and the error of interpolating between chosen keys.
struct Reducer<'a> {
    rig: &'a Rig,
    frames: Vec<u32>,
    poses: Vec<Vec<(Vec3, Quat)>>,
    worlds: Vec<Vec<Vec3>>,
}

/// (error, worst source index, ease) of one segment; error <= 1 means within tolerance.
type SegFit = (f32, usize, Ease);

impl Reducer<'_> {
    /// Pose at source index `i` of the segment `a`..`b` (spline neighbours `prev` / `next`),
    /// exactly as [`AnimDoc::sample`] interpolates it (smoothing 0).
    fn interp_at(&self, prev: usize, a: usize, b: usize, next: usize, ease: Ease, i: usize) -> Vec<(Vec3, Quat)> {
        let t = (self.frames[i] - self.frames[a]) as f32 / (self.frames[b] - self.frames[a]).max(1) as f32;
        let (pa, pb) = (&self.poses[a], &self.poses[b]);
        if ease == Ease::Spline {
            let (pp, pn) = (&self.poses[prev], &self.poses[next]);
            return (0..pa.len()).map(|j| (catmull(pp[j].0, pa[j].0, pb[j].0, pn[j].0, t), quat_catmull(pp[j].1, pa[j].1, pb[j].1, pn[j].1, t))).collect();
        }
        let e = ease.apply(t);
        pa.iter().zip(pb).map(|(x, y)| (x.0.lerp(y.0, e), x.1.slerp(y.1, e).normalize())).collect()
    }

    /// Error of an interpolated pose against source index `i` (<= 1 is within tolerance).
    fn pose_error(&self, p: &[(Vec3, Quat)], i: usize) -> f32 {
        let mut err = 0.0f32;
        for (x, y) in p.iter().zip(&self.poses[i]) {
            err = err.max((x.1.dot(y.1).abs().min(1.0).acos() * 2.0).to_degrees() / REDUCE_ROT_TOL_DEG);
        }
        if err > 1.0 {
            return err;
        }
        for (w, y) in self.rig.world(p).iter().zip(&self.worlds[i]) {
            err = err.max(Vec3::from(w.translation).distance(*y) / REDUCE_POS_TOL);
        }
        err
    }

    /// Does segment `a`..`b` reproduce every source frame in between with `ease`?
    fn fits(&self, prev: usize, a: usize, b: usize, next: usize, ease: Ease) -> bool {
        // test the middle first: it fails most often
        let mid = (a + b) / 2;
        (a + 1..b).filter(|&i| i != mid).chain((b > a + 1).then_some(mid)).collect::<Vec<_>>().iter().rev().all(|&i| self.pose_error(&self.interp_at(prev, a, b, next, ease, i), i) <= 1.0)
    }

    fn seg_error(&self, k: &[usize], s: usize, ease: Ease) -> (f32, usize) {
        let (a, b) = (k[s], k[s + 1]);
        let prev = if s > 0 { k[s - 1] } else { a };
        let next = if s + 2 < k.len() { k[s + 2] } else { b };
        let mut worst = (0.0f32, a);
        for i in a + 1..b {
            let err = self.pose_error(&self.interp_at(prev, a, b, next, ease, i), i);
            if err > worst.0 {
                worst = (err, i);
            }
        }
        worst
    }

    /// The ease that reproduces segment `s` best.
    fn best(&self, k: &[usize], s: usize) -> SegFit {
        let mut best = (f32::MAX, k[s], Ease::Linear);
        for ease in [Ease::Spline, Ease::Smooth, Ease::Linear, Ease::EaseIn, Ease::EaseOut, Ease::Hold] {
            let (err, i) = self.seg_error(k, s, ease);
            if err < best.0 {
                best = (err, i, ease);
            }
        }
        best
    }

    /// Recompute the fits of the segments a change at key position `p` can affect
    /// (spline segments look one key further each way).
    fn refresh(&self, k: &[usize], fits: &mut [SegFit], p: usize) {
        let segs = k.len() - 1;
        for s in p.saturating_sub(3)..(p + 2).min(segs) {
            fits[s] = self.best(k, s);
        }
    }
}

/// Pose-to-pose key reduction: keep only the keys needed to reproduce the animation, each
/// segment getting the ease that fits it (spline / smooth / linear / ease in / ease out / hold).
/// An animation made pose-to-pose and baked to one key per frame (an exported SMD) comes back
/// as roughly its original poses. Every frame stays within [`REDUCE_ROT_TOL_DEG`] /
/// [`REDUCE_POS_TOL`] of the source. Returns the number of keys removed.
pub fn reduce_keys(doc: &mut AnimDoc, rig: &Rig) -> usize {
    let frames: Vec<u32> = doc.keys.keys().copied().collect();
    if frames.len() < 3 || doc.smoothing > 0.0 {
        return 0;
    }
    let n = rig.bones.len();
    let pad = |p: &[(Vec3, Quat)]| -> Vec<(Vec3, Quat)> { (0..n).map(|i| p.get(i).copied().unwrap_or(rig.bind[i])).collect() };
    let poses: Vec<Vec<(Vec3, Quat)>> = frames.iter().map(|f| pad(&doc.keys[f].pose)).collect();
    let worlds = poses.iter().map(|p| rig.world(p).iter().map(|w| Vec3::from(w.translation)).collect()).collect();
    let r = Reducer { rig, frames, poses, worlds };
    let last = r.frames.len() - 1;
    let mut k = if r.frames.len() <= DP_MAX_FRAMES { dp_keys(&r) } else { top_down_keys(&r) };
    // the pose after the last key holds, so a final key that only holds the previous pose
    // is not needed (unless the animation loops back to its start from there)
    while !doc.looping && k.len() > 2 {
        let p = k[k.len() - 2];
        if (p + 1..=last).all(|i| r.pose_error(&r.poses[p], i) <= 1.0) {
            k.pop();
        } else {
            break;
        }
    }
    let fits: Vec<SegFit> = (0..k.len() - 1).map(|s| r.best(&k, s)).collect();

    let before = doc.keys.len();
    let last_ease = doc.keys[&r.frames[*k.last().unwrap()]].ease;
    let mut keys = BTreeMap::new();
    for (s, &i) in k.iter().enumerate() {
        if let Some(mut key) = doc.keys.remove(&r.frames[i]) {
            key.ease = fits.get(s).map(|f| f.2).unwrap_or(last_ease);
            keys.insert(r.frames[i], key);
        }
    }
    doc.keys = keys;
    before - doc.keys.len()
}

/// Longest animation (in source keys) [`dp_keys`] handles; longer ones (mocap) use the
/// faster [`top_down_keys`].
const DP_MAX_FRAMES: usize = 900;
/// Longest gap between two keys [`dp_keys`] considers.
const DP_MAX_GAP: usize = 40;

/// Fewest keys that reproduce the source, by dynamic programming over (previous key, key)
/// pairs - a spline segment depends on the keys either side of it, so keys can't be judged
/// one at a time. Returns source indices, first and last included.
fn dp_keys(r: &Reducer) -> Vec<usize> {
    let last = r.frames.len() - 1;
    let g = DP_MAX_GAP;
    let idx = |i: usize, j: usize| j * (g + 1) + (j - i);
    // state (i, j): the last two keys are i, j and every segment before i..j fits;
    // value: (number of keys, key before i)
    let mut st: Vec<Option<(u32, usize)>> = vec![None; (last + 1) * (g + 1)];
    for j in 1..=g.min(last) {
        st[idx(0, j)] = Some((2, 0));
    }
    let mut simple: std::collections::HashMap<(usize, usize), bool> = std::collections::HashMap::new();
    let mut valid = |h: usize, i: usize, j: usize, k: usize| -> bool {
        let ok = *simple.entry((i, j)).or_insert_with(|| [Ease::Smooth, Ease::Linear, Ease::EaseIn, Ease::EaseOut, Ease::Hold].iter().any(|&e| r.fits(i, i, j, j, e)));
        ok || r.fits(h, i, j, k, Ease::Spline)
    };
    let mut end: Option<(u32, usize)> = None;
    for j in 1..=last {
        for i in j.saturating_sub(g)..j {
            let Some((cnt, h)) = st[idx(i, j)] else { continue };
            if j == last {
                if end.is_none_or(|(c, _)| cnt < c) && valid(h, i, last, last) {
                    end = Some((cnt, i));
                }
                continue;
            }
            for k in j + 1..=(j + g).min(last) {
                if st[idx(j, k)].is_some_and(|(c, _)| c <= cnt + 1) {
                    continue;
                }
                if valid(h, i, j, k) {
                    st[idx(j, k)] = Some((cnt + 1, i));
                }
            }
        }
    }
    let Some((_, mut i)) = end else { return (0..=last).collect() };
    let mut j = last;
    let mut keys = vec![last];
    loop {
        keys.push(i);
        if i == 0 {
            break;
        }
        let Some((_, h)) = st[idx(i, j)] else { return (0..=last).collect() };
        j = i;
        i = h;
    }
    keys.reverse();
    keys
}

/// Key reduction for long animations: start with the ends, add a key where the fit is worst
/// until everything fits, then drop / slide keys that are not needed.
fn top_down_keys(r: &Reducer) -> Vec<usize> {
    let last = r.frames.len() - 1;
    let mut k = vec![0, last];
    let mut fits = vec![r.best(&k, 0)];
    loop {
        let (s, &(err, worst, _)) = fits.iter().enumerate().max_by(|a, b| a.1.0.total_cmp(&b.1.0)).unwrap();
        if err <= 1.0 || worst <= k[s] || worst >= k[s + 1] {
            break;
        }
        k.insert(s + 1, worst);
        fits.insert(s + 1, (0.0, 0, Ease::Linear));
        r.refresh(&k, &mut fits, s + 1);
    }
    // then alternately drop every key the others can do without and slide keys to where they
    // leave the most slack (insertion picks worst-error frames, not the animator's poses)
    for _round in 0..8 {
        let mut changed = false;
        let mut p = 1;
        while p + 1 < k.len() {
            let mut trial = k.clone();
            trial.remove(p);
            let mut tf = fits.clone();
            tf.remove(p);
            r.refresh(&trial, &mut tf, p);
            if tf.iter().all(|f| f.0 <= 1.0) {
                k = trial;
                fits = tf;
                changed = true;
                continue;
            }
            p += 1;
        }
        let local = |f: &[SegFit], p: usize| -> f32 { f[p.saturating_sub(3)..(p + 2).min(f.len())].iter().map(|x| x.0).sum() };
        for p in 1..k.len().saturating_sub(1) {
            let (lo, hi) = (k[p - 1] + 1, k[p + 1] - 1);
            let mut best = (local(&fits, p), k[p], None);
            for cand in k[p].saturating_sub(4).max(lo)..=(k[p] + 4).min(hi) {
                if cand == k[p] {
                    continue;
                }
                let mut trial = k.clone();
                trial[p] = cand;
                let mut tf = fits.clone();
                r.refresh(&trial, &mut tf, p);
                let score = local(&tf, p);
                if tf.iter().all(|f| f.0 <= 1.0) && score < best.0 - 1e-3 {
                    best = (score, cand, Some(tf));
                }
            }
            if let (cand, Some(tf)) = (best.1, best.2) {
                k[p] = cand;
                fits = tf;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    k
}

/// `fps` and `loop` of the QC's `$sequence` / `$animation` that uses `smd` (if any).
pub fn qc_sequence_settings(qc_path: &Path, smd: &Path) -> (Option<f32>, Option<bool>) {
    let Ok(q) = Qc::load(qc_path) else { return (None, None) };
    let stem = smd.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    for c in q.commands.iter().filter(|c| c.name == "$sequence" || c.name == "$animation") {
        let uses = c.args.iter().any(|t| {
            let t = t.text.replace('\\', "/").to_ascii_lowercase();
            let file = t.rsplit('/').next().unwrap_or(&t);
            file == stem || file.strip_suffix(".smd").map(|s| s == stem).unwrap_or(false)
        });
        if !uses {
            continue;
        }
        let mut fps = None;
        let mut looping = None;
        for (i, t) in c.args.iter().enumerate() {
            if t.text.eq_ignore_ascii_case("fps") {
                fps = c.args.get(i + 1).and_then(|v| v.text.parse::<f32>().ok());
            }
            if t.text.eq_ignore_ascii_case("loop") {
                looping = Some(true);
            }
        }
        return (fps, looping);
    }
    (None, None)
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
            eyeballs: vec![],
        }
    }

    /// Baking a pose-to-pose animation to one key per frame (what an SMD export + import does)
    /// and reducing it gives the original keys and eases back.
    #[test]
    fn reduce_recovers_pose_to_pose_keys() {
        let r = rig();
        let mut d = AnimDoc::new(&r);
        d.frames = 60;
        let pose = |a: f32, b: f32| {
            let mut p = r.bind.clone();
            p[1].1 = Quat::from_rotation_z(a);
            p[2].1 = Quat::from_rotation_y(b);
            p
        };
        d.keys.insert(0, Keyframe { pose: pose(0.0, 0.0), ease: Ease::Spline });
        d.keys.insert(12, Keyframe { pose: pose(0.8, 0.3), ease: Ease::Spline });
        d.keys.insert(25, Keyframe { pose: pose(1.2, -0.4), ease: Ease::Smooth });
        d.keys.insert(40, Keyframe { pose: pose(-0.5, 0.9), ease: Ease::Linear });
        d.keys.insert(50, Keyframe { pose: pose(0.2, 0.1), ease: Ease::Smooth });
        let mut baked = d.clone();
        baked.keys = (0..d.frames).map(|f| (f, Keyframe { pose: d.sample(f as f32, &r.bind), ease: Ease::Linear })).collect();
        reduce_keys(&mut baked, &r);
        let got: Vec<(u32, Ease)> = baked.keys.iter().map(|(f, k)| (*f, k.ease)).collect();
        assert_eq!(got.iter().map(|g| g.0).collect::<Vec<_>>(), vec![0, 12, 25, 40, 50], "{got:?}");
        assert_eq!(got[0].1, Ease::Spline);
        assert_eq!(got[2].1, Ease::Smooth);
        assert_eq!(got[3].1, Ease::Linear);
        for f in 0..d.frames {
            let (a, b) = (d.sample(f as f32, &r.bind), baked.sample(f as f32, &r.bind));
            assert!(a.iter().zip(&b).all(|(x, y)| x.1.angle_between(y.1).to_degrees() <= REDUCE_ROT_TOL_DEG + 1e-3), "frame {f}");
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

    /// Dragging the elbow swings it round the shoulder-hand line with the hand fixed, and
    /// moving the hand afterwards keeps the swing instead of snapping back.
    #[test]
    fn elbow_swing_keeps_hand_and_persists() {
        let r = rig();
        let chain = detect_chains(&r)[0].bones.clone();
        let mut pose = r.bind.clone();
        solve_ik(&r, &mut pose, &chain, Vec3::new(14.0, 0.0, 10.0)); // bend the arm
        let pos = |p: &[(Vec3, Quat)], i: usize| -> Vec3 { r.world(p)[i].translation.into() };
        let (shoulder, hand) = (pos(&pose, 1), pos(&pose, 3));
        let axis = (hand - shoulder).normalize();
        let bend_dir = |p: &[(Vec3, Quat)]| {
            let e = pos(p, 2) - shoulder;
            (e - axis * e.dot(axis)).normalize()
        };
        let natural = bend_dir(&pose);
        // swing the elbow 90 degrees round the arm
        let side = axis.cross(natural);
        swing_limb(&r, &mut pose, &chain, shoulder + axis * 7.0 + side * 20.0);
        assert!(pos(&pose, 3).distance(hand) < 1e-3, "hand moved");
        let swung = bend_dir(&pose);
        assert!(swung.dot(side) > 0.99, "elbow did not swing: {swung:?}");
        // move the hand a little: the elbow stays swung
        solve_ik(&r, &mut pose, &chain, hand + Vec3::new(-0.5, 0.0, 0.3));
        let e = pos(&pose, 2) - shoulder;
        let axis2 = (pos(&pose, 3) - shoulder).normalize();
        let after = (e - axis2 * e.dot(axis2)).normalize();
        assert!(after.dot(side) > 0.95, "swing snapped back: {after:?}");
    }
}

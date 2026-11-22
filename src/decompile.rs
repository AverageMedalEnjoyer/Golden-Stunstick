//! MDL -> QC / SMD / VTA / VRD decompiler.
//!
//! Coordinate conventions (verified by compiling probe models with TF2's
//! studiomdl and reading the result back):
//! * reference skeleton + mesh vertices: stored exactly as in the SMD;
//! * animation SMDs: root bones are rotated +90 degrees about Z;
//! * `$eyeposition` / `$illumposition`: rotated +90 degrees about Z;
//! * `$collisionmodel` meshes: stored in bone space, posed by the first frame
//!   of the first sequence;
//! * `$staticprop` models: mesh vertices and collision are rotated +90 degrees about Z.
//!
//! The decompiler applies the inverse of each of these.

use crate::formats::mdl::*;
use crate::formats::phy::Phy;
use crate::formats::smd::{Smd, SmdNode, SmdTri, SmdVert, VtaShape, write_vta};
use glam::{Affine3A, Mat3, Quat, Vec3};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct DecompileOptions {
    pub write_qc: bool,
    pub reference_smd: bool,
    pub lod_smd: bool,
    pub physics_smd: bool,
    pub anim_smd: bool,
    pub anims_subfolder: String,
    pub flex_vta: bool,
    pub procedural_vrd: bool,
    pub definebones: bool,
    pub group_into_qci: bool,
    pub prefix_with_model_name: bool,
    pub skip_hitboxes_if_auto: bool,
}

impl Default for DecompileOptions {
    fn default() -> Self {
        Self {
            write_qc: true,
            reference_smd: true,
            lod_smd: true,
            physics_smd: true,
            anim_smd: true,
            anims_subfolder: "anims".into(),
            flex_vta: true,
            procedural_vrd: true,
            definebones: false,
            group_into_qci: false,
            prefix_with_model_name: false,
            skip_hitboxes_if_auto: false,
        }
    }
}

/// All bytes needed to decompile one model.
pub struct DecompileInput {
    pub stem: String,
    pub mdl: Vec<u8>,
    pub vvd: Option<Vec<u8>>,
    pub vtx: Option<Vec<u8>>,
    pub phy: Option<Vec<u8>>,
    pub ani: Option<Vec<u8>>,
}

impl DecompileInput {
    /// Load the MDL and its sibling files from disk.
    pub fn from_disk(mdl_path: &Path) -> Result<DecompileInput, String> {
        let mdl = std::fs::read(mdl_path).map_err(|e| format!("Could not read {}: {e}", mdl_path.display()))?;
        let stem = mdl_path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "model".into());
        let sib = |ext: &str| -> Option<Vec<u8>> { std::fs::read(mdl_path.with_extension(ext)).ok() };
        let vtx = sib("dx90.vtx").or_else(|| sib("dx80.vtx")).or_else(|| sib("sw.vtx")).or_else(|| sib("vtx"));
        let ani = sib("ani");
        Ok(DecompileInput { stem, vvd: sib("vvd"), vtx, phy: sib("phy"), ani, mdl })
    }

    /// Load from the mounted game filesystem (first source wins).
    pub fn from_gamefs(fs: &crate::gamefs::GameFs, path: &str) -> Result<DecompileInput, String> {
        let mdl = fs.read(path).ok_or_else(|| format!("{path} not found in mounted game"))?;
        let base = path.trim_end_matches(".mdl").trim_end_matches(".MDL");
        let stem = Path::new(base).file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "model".into());
        let vtx = fs.read(&format!("{base}.dx90.vtx")).or_else(|| fs.read(&format!("{base}.dx80.vtx"))).or_else(|| fs.read(&format!("{base}.sw.vtx")));
        Ok(DecompileInput { stem, vvd: fs.read(&format!("{base}.vvd")), vtx, phy: fs.read(&format!("{base}.phy")), ani: fs.read(&format!("{base}.ani")), mdl })
    }
}

#[derive(Default, Debug, Clone)]
pub struct DecompileReport {
    pub qc_path: Option<PathBuf>,
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
    /// file name of each written animation SMD keyed by MDL anim index
    pub anim_files: BTreeMap<usize, String>,
    /// added helper animation used for delta subtraction
    pub refpose_anim: Option<String>,
}

/// Rotation applied to MDL data to get SMD (source) space. Identity: studiomdl
/// stores SMD data unrotated (see module docs).
pub fn rot_to_smd() -> Quat {
    Quat::IDENTITY
}

/// Rotation applied to QC point values that studiomdl rotates on compile ($illumposition).
pub fn rot_qc_point() -> Quat {
    Quat::from_rotation_z(-std::f32::consts::FRAC_PI_2)
}

/// Bones studiomdl keeps without help: weighted by vertices, bonemerge or procedural bones
/// and their ancestors. (Bones used only by attachments / hitboxes get collapsed, verified
/// against TF2's heavy.mdl, so those need `$definebone`.)
pub fn bones_kept_by_studiomdl(mdl: &Mdl) -> Vec<bool> {
    let mut kept: Vec<bool> = mdl.bones.iter().map(|b| b.flags & (0x3fc00 | 0x40000) != 0 || b.proctype != 0).collect();
    for i in (0..mdl.bones.len()).rev() {
        if kept[i] {
            let mut p = mdl.bones[i].parent;
            while p >= 0 {
                kept[p as usize] = true;
                p = mdl.bones[p as usize].parent;
            }
        }
    }
    kept
}

pub fn fmt(v: f32) -> String {
    let v = if v.abs() < 0.0000005 { 0.0 } else { v };
    let s = format!("{v:.6}");
    let s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    if s == "-0" || s.is_empty() { "0".into() } else { s }
}

fn fmt3(v: Vec3) -> String {
    format!("{} {} {}", fmt(v.x), fmt(v.y), fmt(v.z))
}

fn sanitize(name: &str) -> String {
    let n: String = name
        .trim_start_matches('@')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' { c } else { '_' })
        .collect();
    let n = n.trim_end_matches(".smd").trim_end_matches(".dmx").to_string();
    if n.is_empty() { "unnamed".into() } else { n }
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "'"))
}

/// World (model-space) transforms of the bind pose.
pub fn bind_world(mdl: &Mdl) -> Vec<Affine3A> {
    let mut out: Vec<Affine3A> = Vec::with_capacity(mdl.bones.len());
    for b in mdl.bones.iter() {
        let local = Affine3A::from_rotation_translation(b.quat, b.pos);
        let w = if b.parent >= 0 && (b.parent as usize) < out.len() { out[b.parent as usize] * local } else { local };
        out.push(w);
    }
    out
}

/// Convert MDL local bone transforms to SMD skeleton values (root bones rotated back).
pub fn pose_to_smd(mdl: &Mdl, pose: &[(Vec3, Quat)], animation: bool) -> Vec<(Vec3, Vec3)> {
    let r = if animation { rot_qc_point() } else { rot_to_smd() };
    pose.iter()
        .enumerate()
        .map(|(i, (p, q))| {
            if mdl.bones[i].parent < 0 {
                (r * *p, quat_to_euler((r * *q).normalize()))
            } else {
                (*p, quat_to_euler(q.normalize()))
            }
        })
        .collect()
}

fn nodes(mdl: &Mdl) -> Vec<SmdNode> {
    mdl.bones.iter().map(|b| SmdNode { name: b.name.clone(), parent: b.parent }).collect()
}

fn contents_names(c: i32) -> Vec<&'static str> {
    let mut v = Vec::new();
    if c == 0 {
        v.push("notsolid");
        return v;
    }
    let table: &[(i32, &str)] = &[(0x1, "solid"), (0x8, "grate"), (0x2000000, "monster"), (0x20000000, "ladder"), (0x2000, "playerclip"), (0x10000, "monsterclip"), (0x4000000, "debris")];
    for (bit, n) in table {
        if c & bit != 0 {
            v.push(n);
        }
    }
    v
}

fn motion_flags(f: i32) -> String {
    let names = [(0x1, "X"), (0x2, "Y"), (0x4, "Z"), (0x8, "XR"), (0x10, "YR"), (0x20, "ZR"), (0x40, "LX"), (0x80, "LY"), (0x100, "LZ"), (0x200, "LXR"), (0x400, "LYR"), (0x800, "LZR"), (0x1000, "LINEAR")];
    names.iter().filter(|(b, _)| f & b != 0).map(|(_, n)| *n).collect::<Vec<_>>().join(" ")
}

struct Ctx<'a> {
    mdl: &'a Mdl,
    input: &'a DecompileInput,
    opts: &'a DecompileOptions,
    out_dir: PathBuf,
    report: DecompileReport,
    vvd: Option<Vvd>,
    vtx: Option<Vtx>,
    used_names: HashSet<String>,
    /// smd file name per (bodypart, model)
    model_files: HashMap<(usize, usize), String>,
    lod_files: Vec<(usize, Vec<(String, String)>, f32)>,
    phy: Option<Phy>,
}

impl<'a> Ctx<'a> {
    fn unique_file(&mut self, base: &str, ext: &str) -> String {
        let base = sanitize(base);
        let mut name = format!("{base}.{ext}");
        let mut n = 2;
        while self.used_names.contains(&name.to_ascii_lowercase()) {
            name = format!("{base}_{n}.{ext}");
            n += 1;
        }
        self.used_names.insert(name.to_ascii_lowercase());
        name
    }

    fn write(&mut self, rel: &str, content: &str) -> Result<(), String> {
        let p = self.out_dir.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Could not create {}: {e}", parent.display()))?;
        }
        std::fs::write(&p, content).map_err(|e| format!("Could not write {}: {e}", p.display()))?;
        self.report.files.push(p);
        Ok(())
    }

    fn prefix(&self, s: &str) -> String {
        if self.opts.prefix_with_model_name { format!("{}_{}", self.input.stem, s) } else { s.to_string() }
    }

    /// Rotation from MDL mesh space back to SMD space for vertices / collision.
    fn mesh_rot(&self) -> Quat {
        if self.mdl.flags & 0x10 != 0 { rot_qc_point() } else { rot_to_smd() }
    }

    fn mesh_material(&self, mesh: &Mesh) -> String {
        self.mdl.mesh_texture(mesh.material, 0).map(|t| t.name.clone()).unwrap_or_else(|| format!("material_{}", mesh.material))
    }

    /// Build the SMD triangles of one body model at `lod`.
    fn model_triangles(&self, bp: usize, mi: usize, lod: usize) -> Vec<SmdTri> {
        let (Some(vvd), Some(vtx)) = (&self.vvd, &self.vtx) else { return Vec::new() };
        let model = &self.mdl.bodyparts[bp].models[mi];
        let Some(vm) = vtx.bodyparts.get(bp).and_then(|b| b.get(mi)) else { return Vec::new() };
        let Some(lod_meshes) = vm.lods.get(lod) else { return Vec::new() };
        let r = self.mesh_rot();
        let mut out = Vec::new();
        for (me, mesh) in model.meshes.iter().enumerate() {
            let Some(vmesh) = lod_meshes.get(me) else { continue };
            let material = self.mesh_material(mesh);
            let base = (model.first_vertex + mesh.vertex_offset) as usize;
            // Decide winding by majority agreement between face and vertex normals.
            let mut agree = 0i64;
            for t in &vmesh.tris {
                let vs: Vec<&Vertex> = t.iter().filter_map(|&i| vvd.vertices.get(base + i as usize)).collect();
                if vs.len() != 3 {
                    continue;
                }
                let n = (vs[1].pos - vs[0].pos).cross(vs[2].pos - vs[0].pos);
                let vn = vs[0].normal + vs[1].normal + vs[2].normal;
                if n.dot(vn) >= 0.0 { agree += 1 } else { agree -= 1 }
            }
            let flip = agree < 0;
            for t in &vmesh.tris {
                let order = if flip { [t[0], t[2], t[1]] } else { [t[0], t[1], t[2]] };
                let mut verts: [SmdVert; 3] = Default::default();
                let mut ok = true;
                for (k, &i) in order.iter().enumerate() {
                    let Some(v) = vvd.vertices.get(base + i as usize) else {
                        ok = false;
                        break;
                    };
                    let n = v.num_bones.clamp(1, 3) as usize;
                    let links: Vec<(i32, f32)> = (0..n).map(|j| (v.bones[j] as i32, v.weights[j])).collect();
                    verts[k] = SmdVert { bone: v.bones[0] as i32, pos: r * v.pos, normal: (r * v.normal).normalize_or_zero(), uv: [v.uv[0], 1.0 - v.uv[1]], links };
                }
                if ok {
                    out.push(SmdTri { material: material.clone(), verts });
                }
            }
        }
        out
    }

    fn bind_pose_smd(&self) -> Vec<(Vec3, Vec3)> {
        let pose: Vec<(Vec3, Quat)> = self.mdl.bones.iter().map(|b| (b.pos, b.quat)).collect();
        pose_to_smd(self.mdl, &pose, false)
    }

    fn write_meshes(&mut self) -> Result<(), String> {
        if self.vvd.is_none() || self.vtx.is_none() {
            self.report.warnings.push("VVD or VTX file missing - reference meshes were not written".into());
            return Ok(());
        }
        let skeleton = self.bind_pose_smd();
        let nodes = nodes(self.mdl);
        for bp in 0..self.mdl.bodyparts.len() {
            for mi in 0..self.mdl.bodyparts[bp].models.len() {
                let model = &self.mdl.bodyparts[bp].models[mi];
                if model.meshes.is_empty() || model.num_vertices == 0 {
                    continue;
                }
                let base_name = if model.name.trim().is_empty() { format!("{}_{}", self.mdl.bodyparts[bp].name, mi) } else { model.name.clone() };
                let base_name = self.prefix(Path::new(&base_name).file_stem().map(|s| s.to_string_lossy().to_string()).as_deref().unwrap_or(&base_name));
                let file = self.unique_file(&base_name, "smd");
                if self.opts.reference_smd {
                    let smd = Smd { nodes: nodes.clone(), frames: vec![skeleton.clone()], triangles: self.model_triangles(bp, mi, 0) };
                    self.write(&file, &smd.write())?;
                }
                self.model_files.insert((bp, mi), file.clone());
            }
        }
        // LODs
        let nlods = self.vtx.as_ref().map(|v| v.num_lods).unwrap_or(1).max(1) as usize;
        if self.opts.lod_smd && nlods > 1 {
            for lod in 1..nlods {
                let mut replacements = Vec::new();
                let mut switch = 0.0;
                let keys: Vec<(usize, usize)> = self.model_files.keys().copied().collect();
                let mut keys = keys;
                keys.sort();
                for (bp, mi) in keys {
                    let tris = self.model_triangles(bp, mi, lod);
                    if let Some(sp) = self.vtx.as_ref().and_then(|v| v.bodyparts.get(bp)).and_then(|b| b.get(mi)).and_then(|m| m.switch_points.get(lod)) {
                        switch = *sp;
                    }
                    let src = self.model_files[&(bp, mi)].clone();
                    if tris.is_empty() {
                        continue;
                    }
                    let stem = src.trim_end_matches(".smd").to_string();
                    let file = self.unique_file(&format!("{stem}_lod{lod}"), "smd");
                    let smd = Smd { nodes: nodes.clone(), frames: vec![skeleton.clone()], triangles: tris };
                    self.write(&file, &smd.write())?;
                    replacements.push((src, file));
                }
                self.lod_files.push((lod, replacements, switch));
            }
        }
        Ok(())
    }

    fn write_physics(&mut self) -> Result<Option<String>, String> {
        let Some(data) = &self.input.phy else { return Ok(None) };
        let phy = match Phy::parse(data) {
            Ok(p) => p,
            Err(e) => {
                self.report.warnings.push(format!("Could not read .phy: {e}"));
                return Ok(None);
            }
        };
        let world = bind_world(self.mdl);
        let r = self.mesh_rot();
        let infos = phy.solid_infos();
        let ragdoll = phy.solids.len() > 1 || phy.blocks("ragdollconstraint").next().is_some();
        // $collisionmodel meshes are posed by frame 0 of the first sequence
        let posed = first_sequence_world(self.mdl, self.input.ani.as_deref());
        let mut tris = Vec::new();
        for (si, solid) in phy.solids.iter().enumerate() {
            let bone_name = infos.iter().find(|i| i.0 == si).map(|i| i.1.clone()).unwrap_or_default();
            let bone = self.mdl.bone_index(&bone_name).unwrap_or(0);
            let bind = world.get(bone).copied().unwrap_or(Affine3A::IDENTITY);
            let w = if ragdoll { bind } else { bind * posed.get(bone).copied().unwrap_or(Affine3A::IDENTITY).inverse() };
            for (ci, convex) in solid.convexes.iter().enumerate() {
                // studiomdl separates convex pieces by connectivity; one material keeps it under the 32 material limit
                let _ = ci;
                let material = "phy".to_string();
                for t in &convex.tris {
                    let p: Vec<Vec3> = t.iter().map(|v| r * w.transform_point3(*v)).collect();
                    let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize_or_zero();
                    let mk = |pos: Vec3| SmdVert { bone: bone as i32, pos, normal: n, uv: [0.0, 0.0], links: vec![(bone as i32, 1.0)] };
                    tris.push(SmdTri { material: material.clone(), verts: [mk(p[0]), mk(p[1]), mk(p[2])] });
                }
            }
        }
        let file = self.unique_file(&self.prefix(&format!("{}_physics", self.input.stem)), "smd");
        if self.opts.physics_smd {
            let smd = Smd { nodes: nodes(self.mdl), frames: vec![self.bind_pose_smd()], triangles: tris };
            self.write(&file, &smd.write())?;
        }
        self.phy = Some(phy);
        Ok(Some(file))
    }

    /// Returns SMD frames for an animation in source space.
    pub fn anim_frames(&self, ai: usize) -> Vec<Vec<(Vec3, Vec3)>> {
        anim_frames(self.mdl, ai, self.input.ani.as_deref())
    }

    fn write_anims(&mut self) -> Result<(), String> {
        let sub = self.opts.anims_subfolder.trim().trim_matches(['/', '\\']).to_string();
        let mut need_ref = false;
        for ai in 0..self.mdl.anims.len() {
            if self.mdl.anim_uses_block(ai) && self.input.ani.is_none() {
                self.report.warnings.push(format!("Animation '{}' is stored in an external .ani file that was not found; frames use the bind pose", self.mdl.anims[ai].name));
            }
            if self.mdl.anims[ai].flags & STUDIO_FRAMEANIM != 0 {
                self.report.warnings.push(format!("Animation '{}' uses the CS:GO frame-anim encoding which is not supported", self.mdl.anims[ai].name));
            }
            let name = self.prefix(&sanitize(&self.mdl.anims[ai].name));
            let file_name = self.unique_file(&name, "smd");
            let rel = if sub.is_empty() { file_name.clone() } else { format!("{sub}/{file_name}") };
            if self.opts.anim_smd {
                let smd = Smd { nodes: nodes(self.mdl), frames: self.anim_frames(ai), triangles: Vec::new() };
                self.write(&rel, &smd.write())?;
            }
            if self.mdl.anims[ai].flags & STUDIO_DELTA != 0 {
                need_ref = true;
            }
            self.report.anim_files.insert(ai, rel);
        }
        if need_ref {
            let file_name = self.unique_file("gs_refpose", "smd");
            let rel = if sub.is_empty() { file_name.clone() } else { format!("{sub}/{file_name}") };
            let pose: Vec<(Vec3, Quat)> = self.mdl.bones.iter().map(|b| (b.pos, b.quat)).collect();
            let smd = Smd { nodes: nodes(self.mdl), frames: vec![pose_to_smd(self.mdl, &pose, true)], triangles: Vec::new() };
            self.write(&rel, &smd.write())?;
            self.report.refpose_anim = Some(rel);
        }
        Ok(())
    }

    fn write_vta(&mut self) -> Result<HashMap<(usize, usize), (String, Vec<(i32, usize, i32)>)>, String> {
        // returns per model: (vta file, Vec<(flexdesc, frame, flexpair)>)
        let mut out = HashMap::new();
        let Some(vvd) = self.vvd.clone() else { return Ok(out) };
        let r = self.mesh_rot();
        let keys: Vec<(usize, usize)> = {
            let mut k: Vec<_> = self.model_files.keys().copied().collect();
            k.sort();
            k
        };
        for (bp, mi) in keys {
            let model = &self.mdl.bodyparts[bp].models[mi];
            if !model.meshes.iter().any(|m| !m.flexes.is_empty()) {
                continue;
            }
            // VTA vertex list = the SMD-style unique vertices of the model in VVD order
            let first = model.first_vertex as usize;
            let count = model.num_vertices.max(0) as usize;
            let base: Vec<(Vec3, Vec3)> = (0..count).filter_map(|i| vvd.vertices.get(first + i)).map(|v| (r * v.pos, (r * v.normal).normalize_or_zero())).collect();
            let mut shapes: Vec<VtaShape> = Vec::new();
            let mut flexes: Vec<(i32, usize, i32)> = Vec::new();
            // group by flexdesc (one shape per flexdesc, summing meshes)
            let mut by_desc: BTreeMap<(i32, i32), BTreeMap<usize, (Vec3, Vec3)>> = BTreeMap::new();
            for mesh in &model.meshes {
                for fx in &mesh.flexes {
                    let e = by_desc.entry((fx.flexdesc, fx.flexpair)).or_default();
                    for va in &fx.verts {
                        let vi = mesh.vertex_offset as usize + va.index as usize;
                        if let Some(b) = base.get(vi) {
                            e.insert(vi, (b.0 + r * va.delta, (b.1 + r * va.ndelta).normalize_or_zero()));
                        }
                    }
                }
            }
            for ((desc, pair), verts) in by_desc {
                shapes.push(VtaShape { verts: verts.into_iter().map(|(i, (p, n))| (i, p, n)).collect() });
                flexes.push((desc, shapes.len(), pair));
            }
            let stem = self.model_files[&(bp, mi)].trim_end_matches(".smd").to_string();
            let file = self.unique_file(&stem, "vta");
            if self.opts.flex_vta {
                let text = write_vta(&nodes(self.mdl), &base, &shapes);
                self.write(&file, &text)?;
            }
            out.insert((bp, mi), (file, flexes));
        }
        Ok(out)
    }

    fn write_vrd(&mut self) -> Result<Option<String>, String> {
        let b = crate::formats::bin::Bin::new(&self.mdl.data);
        let mut text = String::new();
        for bone in &self.mdl.bones {
            if bone.proctype != 2 || bone.procindex == 0 {
                continue;
            }
            let base = (bone.offset as i64 + bone.procindex as i64) as usize;
            let (Ok(control), Ok(ntrig), Ok(tidx)) = (b.i32(base), b.i32(base + 4), b.i32(base + 8)) else { continue };
            let control_name = self.mdl.bones.get(control as usize).map(|b| b.name.clone()).unwrap_or_default();
            let control_parent = self.mdl.bones.get(control as usize).and_then(|c| self.mdl.bones.get(c.parent.max(0) as usize)).map(|b| b.name.clone()).unwrap_or_default();
            let parent = self.mdl.bones.get(bone.parent.max(0) as usize).map(|b| b.name.clone()).unwrap_or_default();
            let _ = writeln!(text, "<helper> {} {} {} {}", bone.name, parent, control_parent, control_name);
            let _ = writeln!(text, "<basepos> {}", fmt3(bone.pos));
            let to = (base as i64 + tidx as i64) as usize;
            for t in 0..ntrig.clamp(0, 64) as usize {
                let p = to + t * 48;
                let (Ok(inv_tol), Ok(tq), Ok(pos), Ok(q)) = (b.f32(p), b.vec4(p + 4), b.vec3(p + 20), b.vec4(p + 32)) else { continue };
                let tol = if inv_tol.abs() > 1e-9 { (1.0 / inv_tol).to_degrees() } else { 0.0 };
                let te = quat_to_euler(Quat::from_xyzw(tq[0], tq[1], tq[2], tq[3]));
                let qe = quat_to_euler(Quat::from_xyzw(q[0], q[1], q[2], q[3]));
                let _ = writeln!(
                    text,
                    "<trigger> {} {} {} {} {} {} {} {}",
                    fmt(tol),
                    fmt(te.x.to_degrees()),
                    fmt(te.y.to_degrees()),
                    fmt(te.z.to_degrees()),
                    fmt(qe.x.to_degrees()),
                    fmt(qe.y.to_degrees()),
                    fmt(qe.z.to_degrees()),
                    fmt3(Vec3::from_array(pos))
                );
            }
            text.push('\n');
        }
        for bone in &self.mdl.bones {
            if bone.proctype != 0 && bone.proctype != 2 && bone.proctype != 5 {
                self.report.warnings.push(format!("Procedural bone '{}' has type {} which is not exported", bone.name, bone.proctype));
            }
        }
        if text.is_empty() {
            return Ok(None);
        }
        let file = self.unique_file(&self.prefix(&self.input.stem.clone()), "vrd");
        if self.opts.procedural_vrd {
            self.write(&file, &text)?;
        }
        Ok(Some(file))
    }

    fn jigglebones(&self) -> String {
        let b = crate::formats::bin::Bin::new(&self.mdl.data);
        let mut s = String::new();
        for bone in &self.mdl.bones {
            if bone.proctype != 5 || bone.procindex == 0 {
                continue;
            }
            let o = (bone.offset as i64 + bone.procindex as i64) as usize;
            let f = |k: usize| b.f32(o + 4 + k * 4).unwrap_or(0.0);
            let Ok(flags) = b.i32(o) else { continue };
            let deg = |v: f32| fmt(v.to_degrees());
            let _ = writeln!(s, "$jigglebone {}\n{{", quote(&bone.name));
            // field indices after flags: 0 length,1 tipMass,2 yawStiff,3 yawDamp,4 pitchStiff,5 pitchDamp,6 alongStiff,7 alongDamp,
            // 8 angleLimit,9 minYaw,10 maxYaw,11 yawFriction,12 yawBounce,13 minPitch,14 maxPitch,15 pitchFriction,16 pitchBounce,
            // 17 baseMass,18 baseStiffness,19 baseDamping,20 baseMinLeft,21 baseMaxLeft,22 baseLeftFriction,23 baseMinUp,24 baseMaxUp,
            // 25 baseUpFriction,26 baseMinForward,27 baseMaxForward,28 baseForwardFriction,29 boingImpactSpeed,30 boingImpactAngle,
            // 31 boingDampingRate,32 boingFrequency,33 boingAmplitude
            if flags & 0x01 != 0 {
                let _ = writeln!(s, "\tis_flexible\n\t{{");
                let _ = writeln!(s, "\t\tlength {}\n\t\ttip_mass {}", fmt(f(0)), fmt(f(1)));
                let _ = writeln!(s, "\t\tyaw_stiffness {}\n\t\tyaw_damping {}", fmt(f(2)), fmt(f(3)));
                let _ = writeln!(s, "\t\tpitch_stiffness {}\n\t\tpitch_damping {}", fmt(f(4)), fmt(f(5)));
                let _ = writeln!(s, "\t\talong_stiffness {}\n\t\talong_damping {}", fmt(f(6)), fmt(f(7)));
                if flags & 0x10 != 0 {
                    let _ = writeln!(s, "\t\tangle_constraint {}", deg(f(8)));
                }
                if flags & 0x04 != 0 {
                    let _ = writeln!(s, "\t\tyaw_constraint {} {}\n\t\tyaw_friction {}\n\t\tyaw_bounce {}", deg(f(9)), deg(f(10)), fmt(f(11)), fmt(f(12)));
                }
                if flags & 0x08 != 0 {
                    let _ = writeln!(s, "\t\tpitch_constraint {} {}\n\t\tpitch_friction {}\n\t\tpitch_bounce {}", deg(f(13)), deg(f(14)), fmt(f(15)), fmt(f(16)));
                }
                if flags & 0x20 == 0 {
                    let _ = writeln!(s, "\t\tallow_length_flex");
                }
                let _ = writeln!(s, "\t}}");
            }
            if flags & 0x02 != 0 {
                let _ = writeln!(s, "\tis_rigid\n\t{{\n\t\tlength {}\n\t\ttip_mass {}\n\t}}", fmt(f(0)), fmt(f(1)));
            }
            if flags & 0x40 != 0 {
                let _ = writeln!(s, "\thas_base_spring\n\t{{");
                let _ = writeln!(s, "\t\tbase_mass {}\n\t\tstiffness {}\n\t\tdamping {}", fmt(f(17)), fmt(f(18)), fmt(f(19)));
                let _ = writeln!(s, "\t\tleft_constraint {} {}\n\t\tleft_friction {}", fmt(f(20)), fmt(f(21)), fmt(f(22)));
                let _ = writeln!(s, "\t\tup_constraint {} {}\n\t\tup_friction {}", fmt(f(23)), fmt(f(24)), fmt(f(25)));
                let _ = writeln!(s, "\t\tforward_constraint {} {}\n\t\tforward_friction {}", fmt(f(26)), fmt(f(27)), fmt(f(28)));
                let _ = writeln!(s, "\t}}");
            }
            if flags & 0x80 != 0 {
                let _ = writeln!(s, "\tis_boing\n\t{{");
                let _ = writeln!(s, "\t\timpact_speed {}\n\t\timpact_angle {}\n\t\tdamping_rate {}\n\t\tfrequency {}\n\t\tamplitude {}", fmt(f(29)), deg(f(30)), fmt(f(31)), fmt(f(32)), fmt(f(33)));
                let _ = writeln!(s, "\t}}");
            }
            s.push_str("}\n\n");
        }
        s
    }
}

/// True when the animation's last frame equals its first (studiomdl's loop fix-up is then a no-op).
fn loop_frames_match(mdl: &Mdl, ai: usize, ani: Option<&[u8]>) -> bool {
    let n = mdl.anims[ai].numframes;
    if n <= 1 {
        return true;
    }
    let a = mdl.decode_frame(ai, 0, ani);
    let b = mdl.decode_frame(ai, n - 1, ani);
    a.iter().zip(b.iter()).enumerate().all(|(i, (x, y))| {
        // root bones carry extracted motion, which studiomdl handles separately
        mdl.bones[i].parent < 0 || ((x.0 - y.0).length() < 0.01 && x.1.dot(y.1).abs() > 0.99999)
    })
}

/// World transforms of every bone at frame 0 of the first sequence (bind pose if none).
pub fn first_sequence_world(mdl: &Mdl, ani: Option<&[u8]>) -> Vec<Affine3A> {
    let anim = mdl.seqs.first().and_then(|s| s.anim_indices.first()).map(|&i| i.max(0) as usize).filter(|&i| i < mdl.anims.len());
    let Some(ai) = anim else { return bind_world(mdl) };
    let mut pose = mdl.decode_frame(ai, 0, ani);
    if mdl.anims[ai].flags & STUDIO_DELTA != 0 {
        for (i, b) in mdl.bones.iter().enumerate() {
            pose[i] = (b.pos + pose[i].0, (b.quat * pose[i].1).normalize());
        }
    }
    let mut out: Vec<Affine3A> = Vec::with_capacity(pose.len());
    for (i, (p, q)) in pose.iter().enumerate() {
        let local = Affine3A::from_rotation_translation(*q, *p);
        let parent = mdl.bones[i].parent;
        out.push(if parent >= 0 && (parent as usize) < out.len() { out[parent as usize] * local } else { local });
    }
    out
}

/// Source-space SMD frames for an animation (root motion re-applied, deltas composed on the bind pose).
pub fn anim_frames(mdl: &Mdl, ai: usize, ani: Option<&[u8]>) -> Vec<Vec<(Vec3, Vec3)>> {
    let ad = &mdl.anims[ai];
    let delta = ad.flags & STUDIO_DELTA != 0;
    let mut frames = Vec::new();
    for f in 0..ad.numframes.max(1) {
        let mut pose = mdl.decode_frame(ai, f, ani);
        if delta {
            for (i, b) in mdl.bones.iter().enumerate() {
                pose[i] = (b.pos + pose[i].0, (b.quat * pose[i].1).normalize());
            }
        } else if !ad.movements.is_empty() {
            let (mpos, myaw) = mdl.movement_at(ai, f as f32);
            let mq = Quat::from_rotation_z(myaw.to_radians());
            for (i, b) in mdl.bones.iter().enumerate() {
                if b.parent < 0 {
                    pose[i] = (mq * pose[i].0 + mpos, (mq * pose[i].1).normalize());
                }
            }
        }
        frames.push(pose_to_smd(mdl, &pose, true));
    }
    frames
}

/// Convert flex rule RPN to an infix expression.
/// Approximate the `flexpair` split width. The MDL only keeps per-vertex side
/// weights, not the original width, so this returns 1 unit when the flex has a
/// blended seam and 0 for a hard left/right split.
fn estimate_split(model: &Model, desc: i32) -> f32 {
    let mut max_partial = 0.0f32;
    for mesh in &model.meshes {
        for fx in mesh.flexes.iter().filter(|f| f.flexdesc == desc) {
            for va in &fx.verts {
                if va.side > 2 && va.side < 253 {
                    let t = (va.side as f32 / 255.0 - 0.5).abs() * 2.0;
                    max_partial = max_partial.max(t);
                }
            }
        }
    }
    if max_partial > 0.0 { 1.0 } else { 0.0 }
}

/// Returns `None` for rules that use operators with no QC syntax (DME eyelid / NWAY rules,
/// which studiomdl generates itself from `eyelid` / DMX definitions).
fn flex_rule_expr(mdl: &Mdl, rule: &FlexRule) -> Option<String> {
    if rule.ops.iter().any(|o| !matches!(o.0, 1..=8 | 13..=16 | 18 | 19)) {
        return None;
    }
    let mut st: Vec<String> = Vec::new();
    let ctrl = |i: i32| mdl.flex_controllers.get(i as usize).map(|c| c.name.clone()).unwrap_or(format!("ctrl{i}"));
    let flex = |i: i32| mdl.flex_descs.get(i as usize).map(|c| format!("%{c}")).unwrap_or(format!("%flex{i}"));
    for &(op, idx, val) in &rule.ops {
        match op {
            1 => st.push(fmt(val)),
            2 => st.push(ctrl(idx)),
            3 => st.push(flex(idx)),
            4..=7 => {
                let b = st.pop().unwrap_or_default();
                let a = st.pop().unwrap_or_default();
                let o = ["+", "-", "*", "/"][(op - 4) as usize];
                st.push(format!("({a} {o} {b})"));
            }
            8 => {
                let a = st.pop().unwrap_or_default();
                st.push(format!("(-{a})"));
            }
            13 | 14 => {
                let b = st.pop().unwrap_or_default();
                let a = st.pop().unwrap_or_default();
                st.push(format!("{}({a}, {b})", if op == 13 { "max" } else { "min" }));
            }
            15 => st.push(format!("min(max(-{}, 0), 1)", ctrl(idx))),
            16 => st.push(format!("min(max({}, 0), 1)", ctrl(idx))),
            18 => {
                let n = idx.max(0) as usize;
                let mut args: Vec<String> = (0..n).filter_map(|_| st.pop()).collect();
                args.reverse();
                st.push(format!("({})", args.join(" * ")));
            }
            19 => {
                let n = idx.max(0) as usize;
                let mut args: Vec<String> = (0..n + 1).filter_map(|_| st.pop()).collect();
                args.reverse();
                if args.is_empty() {
                    st.push("0".into());
                } else {
                    let first = args.remove(0);
                    st.push(format!("({first} * (1 - {}))", if args.is_empty() { "0".into() } else { args.join(" * ") }));
                }
            }
            _ => return None,
        }
    }
    Some(st.pop().unwrap_or_else(|| "0".into()))
}

/// Decompile a model into `out_dir`.
pub fn decompile(input: &DecompileInput, out_dir: &Path, opts: &DecompileOptions, log: &mut dyn FnMut(String)) -> Result<DecompileReport, String> {
    let mdl = Mdl::parse(input.mdl.clone()).map_err(|e| format!("MDL parse error: {e}"))?;
    log(format!("Read {} (version {}, {} bones, {} sequences, {} animations)", mdl.name, mdl.version, mdl.bones.len(), mdl.seqs.len(), mdl.anims.len()));
    std::fs::create_dir_all(out_dir).map_err(|e| format!("Could not create {}: {e}", out_dir.display()))?;
    let vvd = input.vvd.as_deref().and_then(|d| Vvd::parse(d).map_err(|e| log(format!("VVD error: {e}"))).ok());
    let vtx = input.vtx.as_deref().and_then(|d| Vtx::parse(d).map_err(|e| log(format!("VTX error: {e}"))).ok());
    if let Some(v) = &vvd {
        if v.checksum != mdl.checksum {
            log("WARNING: VVD checksum does not match the MDL".into());
        }
    }
    if let Some(v) = &vtx {
        if v.checksum != mdl.checksum {
            log("WARNING: VTX checksum does not match the MDL".into());
        }
    }
    let mut cx = Ctx {
        mdl: &mdl,
        input,
        opts,
        out_dir: out_dir.to_path_buf(),
        report: DecompileReport::default(),
        vvd,
        vtx,
        used_names: HashSet::new(),
        model_files: HashMap::new(),
        lod_files: Vec::new(),
        phy: None,
    };
    let qc_name = format!("{}.qc", input.stem);
    cx.used_names.insert(qc_name.to_ascii_lowercase());

    log("Writing reference meshes...".into());
    cx.write_meshes()?;
    log("Writing physics mesh...".into());
    let phys_file = cx.write_physics()?;
    log("Writing animations...".into());
    cx.write_anims()?;
    log("Writing flexes...".into());
    let vtas = cx.write_vta()?;
    let vrd = cx.write_vrd()?;

    if opts.write_qc {
        log("Writing QC...".into());
        let qc = build_qc(&mut cx, phys_file.as_deref(), &vtas, vrd.as_deref());
        if opts.group_into_qci {
            let mut main = String::new();
            for (section, text) in qc {
                if text.trim().is_empty() {
                    continue;
                }
                if section == "header" {
                    main.push_str(&text);
                    continue;
                }
                let f = format!("{}_{}.qci", input.stem, section);
                cx.write(&f, &join_braces(&text))?;
                let _ = writeln!(main, "$include \"{f}\"");
            }
            cx.write(&qc_name, &join_braces(&main))?;
        } else {
            let all: String = qc.into_iter().map(|(_, t)| t).collect::<Vec<_>>().join("\n");
            cx.write(&qc_name, &join_braces(&all))?;
        }
        cx.report.qc_path = Some(out_dir.join(&qc_name));
    }
    for w in &cx.report.warnings {
        log(format!("WARNING: {w}"));
    }
    log(format!("Done: {} file(s) written to {}", cx.report.files.len(), out_dir.display()));
    Ok(cx.report)
}

/// studiomdl only reads command options on the same line as the command,
/// so a block-opening brace must not sit on a line of its own.
fn join_braces(text: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for line in text.lines() {
        if line.trim() == "{" {
            if let Some(prev) = out.last_mut() {
                let p = prev.trim();
                // $shadowlod reads an (ignored) switch value from its own line, so its brace must stay below
                if !p.is_empty() && !p.ends_with('{') && !p.ends_with('}') && !p.starts_with("//") && p != "$shadowlod" {
                    prev.push_str(" {");
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    let mut s = out.join("\n");
    s.push('\n');
    s
}

fn build_qc(cx: &mut Ctx, phys_file: Option<&str>, vtas: &HashMap<(usize, usize), (String, Vec<(i32, usize, i32)>)>, vrd: Option<&str>) -> Vec<(&'static str, String)> {
    let mdl = cx.mdl;
    let mut sections: Vec<(&'static str, String)> = Vec::new();
    let mut h = String::new();
    let _ = writeln!(h, "// Decompiled by Golden Stunstick {} on {}", env!("CARGO_PKG_VERSION"), chrono::Local::now().format("%Y-%m-%d %H:%M"));
    let _ = writeln!(h, "// Source model: {} (MDL v{}, checksum {})\n", mdl.name, mdl.version, mdl.checksum);
    let model_name = mdl.name.replace('\\', "/");
    let _ = writeln!(h, "$modelname {}", quote(model_name.strip_prefix("models/").unwrap_or(&model_name)));
    for inc in &mdl.include_models {
        let n = inc.name.replace('\\', "/");
        let _ = writeln!(h, "$includemodel {}", quote(n.strip_prefix("models/").unwrap_or(&n)));
    }
    sections.push(("header", h));

    // ---- model / bodygroups
    let mut m = String::new();
    let world = bind_world(mdl);
    let r = rot_to_smd();
    for (bi, bp) in mdl.bodyparts.iter().enumerate() {
        let single = bp.models.len() == 1;
        if single {
            let Some(file) = cx.model_files.get(&(bi, 0)).cloned() else {
                let _ = writeln!(m, "$bodygroup {}\n{{\n\tblank\n}}", quote(&bp.name));
                continue;
            };
            let model = &bp.models[0];
            let vta = vtas.get(&(bi, 0));
            let has_extra = vta.is_some() || !model.eyeballs.is_empty();
            if !has_extra {
                let _ = writeln!(m, "$body {} {}", quote(&bp.name), quote(&file));
                continue;
            }
            let _ = writeln!(m, "$model {} {}\n{{", quote(&bp.name), quote(&file));
            for (ei, e) in model.eyeballs.iter().enumerate() {
                let bw = world.get(e.bone.max(0) as usize).copied().unwrap_or(Affine3A::IDENTITY);
                let pos = r * bw.transform_point3(e.org);
                // the eyeball's own material is the mesh tagged as eyeball (materialtype 1) with this index
                let eye_mesh = model.meshes.iter().find(|me| me.material_type == 1 && me.material_param == ei as i32);
                let tex = eye_mesh
                    .and_then(|me| mdl.mesh_texture(me.material, 0))
                    .or_else(|| mdl.textures.get(e.texture.max(0) as usize))
                    .map(|t| t.name.clone())
                    .unwrap_or_default();
                let ename = if e.name.is_empty() { if pos.x >= 0.0 { format!("eye_{ei}_right") } else { format!("eye_{ei}_left") } } else { e.name.clone() };
                let bone = mdl.bones.get(e.bone.max(0) as usize).map(|b| b.name.clone()).unwrap_or_default();
                let angle = e.zoffset.atan().to_degrees();
                let iris = if e.iris_scale.abs() > 1e-9 { 1.0 / e.iris_scale } else { 1.0 };
                let _ = writeln!(m, "\teyeball {} {} {} {} {} {} \"pupil\" {}", quote(&ename), quote(&bone), fmt3(pos), quote(&tex), fmt(e.radius * 2.0), fmt(angle), fmt(iris));
            }
            if let Some((vta_file, flexes)) = vta {
                let _ = writeln!(m, "\tflexfile {}\n\t{{\n\t\tdefaultflex frame 0", quote(vta_file));
                let mut declared: HashSet<i32> = HashSet::new();
                for (desc, frame, pair) in flexes {
                    let dn = mdl.flex_descs.get(*desc as usize).cloned().unwrap_or(format!("flex{desc}"));
                    let pn = mdl.flex_descs.get(*pair as usize).cloned().unwrap_or_default();
                    declared.insert(*desc);
                    if *pair != 0 && dn.len() > 1 && dn.ends_with('L') && pn == format!("{}R", &dn[..dn.len() - 1]) {
                        declared.insert(*pair);
                        let split = estimate_split(model, *desc);
                        let _ = writeln!(m, "\t\tflexpair {} {} frame {}", quote(&dn[..dn.len() - 1]), fmt(split), frame);
                    } else {
                        let _ = writeln!(m, "\t\tflex {} frame {}", quote(&dn), frame);
                    }
                }
                // flexes driven only by rules (no vertex data) still have to be declared
                for (di, dn) in mdl.flex_descs.iter().enumerate() {
                    // eyelid helper flexes are generated by studiomdl itself
                    if dn.starts_with('%') || dn.starts_with("dummy_eyelid") {
                        continue;
                    }
                    if !declared.contains(&(di as i32)) {
                        let _ = writeln!(m, "\t\tflex {} frame 0", quote(dn));
                    }
                }
                let _ = writeln!(m, "\t}}");
                for c in &mdl.flex_controllers {
                    let _ = writeln!(m, "\tflexcontroller {} range {} {} {}", c.kind, fmt(c.min), fmt(c.max), quote(&c.name));
                }
                for rule in &mdl.flex_rules {
                    let dn = mdl.flex_descs.get(rule.flex as usize).cloned().unwrap_or(format!("flex{}", rule.flex));
                    match flex_rule_expr(mdl, rule) {
                        Some(e) => {
                            let _ = writeln!(m, "\t%{} = {}", dn, e);
                        }
                        None => {
                            let _ = writeln!(m, "\t// %{dn}: eyelid/DME rule (generated by studiomdl from eyelid data, no QC expression)");
                        }
                    }
                }
            }
            let _ = writeln!(m, "}}");
        } else {
            let _ = writeln!(m, "$bodygroup {}\n{{", quote(&bp.name));
            for (mi, _) in bp.models.iter().enumerate() {
                match cx.model_files.get(&(bi, mi)) {
                    Some(f) => {
                        let _ = writeln!(m, "\tstudio {}", quote(f));
                    }
                    None => {
                        let _ = writeln!(m, "\tblank");
                    }
                }
            }
            let _ = writeln!(m, "}}");
        }
    }
    for (lod, reps, switch) in &cx.lod_files {
        let shadow = mdl.flags & 0x40 != 0 && *lod == cx.lod_files.len();
        if shadow {
            let _ = writeln!(m, "\n$shadowlod\n{{");
        } else {
            let _ = writeln!(m, "\n$lod {}\n{{", fmt(*switch));
        }
        for (a, b) in reps {
            let _ = writeln!(m, "\treplacemodel {} {}", quote(a), quote(b));
        }
        let _ = writeln!(m, "}}");
        let _ = lod;
    }
    sections.push(("model", m));

    // ---- materials & misc
    let mut t = String::new();
    if !mdl.surfaceprop.is_empty() {
        let _ = writeln!(t, "$surfaceprop {}", quote(&mdl.surfaceprop));
    }
    if mdl.contents != 1 {
        let _ = writeln!(t, "$contents {}", contents_names(mdl.contents).iter().map(|s| quote(s)).collect::<Vec<_>>().join(" "));
    }
    let flag_cmds = [(0x4, "$opaque"), (0x8, "$mostlyopaque"), (0x10, "$staticprop"), (0x200, "$obsolete"), (0x800, "$noforcedfade"), (0x1000, "$forcephonemecrossfade"), (0x10000, "$ambientboost"), (0x20000, "$donotcastshadows"), (0x40000, "$casttextureshadows")];
    for (bit, cmd) in flag_cmds {
        if mdl.flags & bit != 0 {
            let _ = writeln!(t, "{cmd}");
        }
    }
    if mdl.flags & 0x2000 != 0 {
        let _ = writeln!(t, "$constantdirectionallight {}", fmt(mdl.constdirectionallightdot as f32 / 255.0));
    }
    let _ = writeln!(t, "$eyeposition {}", fmt3(rot_qc_point() * mdl.eyeposition));
    let _ = writeln!(t, "$illumposition {}", fmt3(rot_qc_point() * mdl.illumposition));
    if mdl.max_eye_deflection.abs() > 1e-6 && mdl.max_eye_deflection < 1.0 {
        let _ = writeln!(t, "$maxeyedeflection {}", fmt(mdl.max_eye_deflection.clamp(-1.0, 1.0).acos().to_degrees()));
    }
    if mdl.illum_position_attachment > 0 {
        if let Some(a) = mdl.attachments.get((mdl.illum_position_attachment - 1) as usize) {
            let _ = writeln!(t, "$illumpositionattachment {}", quote(&a.name));
        }
    }
    for cd in &mdl.cdmaterials {
        let _ = writeln!(t, "$cdmaterials {}", quote(cd));
    }
    if mdl.skins.len() > 1 {
        let ncol = mdl.skin_ref.max(0) as usize;
        let cols: Vec<usize> = (0..ncol).filter(|&c| mdl.skins.iter().any(|fam| fam.get(c) != mdl.skins[0].get(c))).collect();
        if !cols.is_empty() {
            let _ = writeln!(t, "\n$texturegroup \"skinfamilies\"\n{{");
            for fam in &mdl.skins {
                let names: Vec<String> = cols.iter().map(|&c| quote(&mdl.textures.get(fam[c] as usize).map(|x| x.name.clone()).unwrap_or_default())).collect();
                let _ = writeln!(t, "\t{{ {} }}", names.join(" "));
            }
            let _ = writeln!(t, "}}");
        }
    }
    if !mdl.keyvalues.trim().is_empty() {
        let kv = mdl.keyvalues.trim();
        let inner = match (kv.find('{'), kv.rfind('}')) {
            (Some(a), Some(b)) if b > a && kv[..a].trim().eq_ignore_ascii_case("mdlkeyvalue") => kv[a + 1..b].to_string(),
            _ => kv.to_string(),
        };
        let _ = writeln!(t, "\n$keyvalues\n{{\n{}\n}}", inner.trim_matches('\n'));
    }
    sections.push(("textures", t));

    // ---- skeleton
    let mut s = String::new();
    let has_weightlists = mdl.seqs.iter().any(|s| !s.weightlist.is_empty() && s.weightlist.iter().any(|w| (*w - 1.0).abs() > 1e-6));
    // $weightlist resolves bone names against bones already known, so declare the skeleton first
    let need_define = bones_kept_by_studiomdl(mdl).iter().any(|k| !k) || has_weightlists;
    if need_define && !cx.opts.definebones {
        let _ = writeln!(s, "// Some bones are not used by any mesh, attachment or hitbox; $definebone keeps studiomdl from removing them");
    }
    if cx.opts.definebones || need_define {
        let pose: Vec<(Vec3, Quat)> = mdl.bones.iter().map(|b| (b.pos, b.quat)).collect();
        let smd = pose_to_smd(mdl, &pose, false);
        for (i, b) in mdl.bones.iter().enumerate() {
            let parent = if b.parent >= 0 { mdl.bones[b.parent as usize].name.clone() } else { String::new() };
            let (p, e) = smd[i];
            let _ = writeln!(s, "$definebone {} {} {} {} {} {} 0 0 0 0 0 0", quote(&b.name), quote(&parent), fmt3(p), fmt(e.y.to_degrees()), fmt(e.z.to_degrees()), fmt(e.x.to_degrees()));
        }
    }
    for c in &mdl.bone_controllers {
        let types = [(0x1, "X"), (0x2, "Y"), (0x4, "Z"), (0x8, "XR"), (0x10, "YR"), (0x20, "ZR")];
        let ty = types.iter().filter(|(b, _)| c.kind & b != 0).map(|(_, n)| *n).collect::<Vec<_>>().join(" ");
        let bone = mdl.bones.get(c.bone.max(0) as usize).map(|b| b.name.clone()).unwrap_or_default();
        let _ = writeln!(s, "$controller {} {} {} {} {}", c.inputfield, quote(&bone), ty, fmt(c.start), fmt(c.end));
    }
    for a in &mdl.attachments {
        let bone = mdl.bones.get(a.bone.max(0) as usize).map(|b| b.name.clone()).unwrap_or_default();
        let m3 = Mat3::from_cols(
            Vec3::new(a.local[0][0], a.local[1][0], a.local[2][0]),
            Vec3::new(a.local[0][1], a.local[1][1], a.local[2][1]),
            Vec3::new(a.local[0][2], a.local[1][2], a.local[2][2]),
        );
        let pos = Vec3::new(a.local[0][3], a.local[1][3], a.local[2][3]);
        let e = quat_to_euler(Quat::from_mat3(&m3));
        let mut line = format!("$attachment {} {} {} rigid", quote(&a.name), quote(&bone), fmt3(pos));
        if a.flags & 0x10000 != 0 {
            line.push_str(" world_align");
        }
        if e.length() > 1e-6 {
            let _ = write!(line, " rotate {} {} {}", fmt(e.y.to_degrees()), fmt(e.z.to_degrees()), fmt(e.x.to_degrees()));
        }
        let _ = writeln!(s, "{line}");
    }
    // $bbox = movement hull, $cbox = view clipping box
    if mdl.hull_min != Vec3::ZERO || mdl.hull_max != Vec3::ZERO {
        let _ = writeln!(s, "\n$bbox {} {}", fmt3(mdl.hull_min), fmt3(mdl.hull_max));
    }
    if mdl.view_bbmin != Vec3::ZERO || mdl.view_bbmax != Vec3::ZERO {
        let _ = writeln!(s, "$cbox {} {}", fmt3(mdl.view_bbmin), fmt3(mdl.view_bbmax));
    }
    for b in mdl.bones.iter().filter(|b| b.flags & 0x40000 != 0) {
        let _ = writeln!(s, "$bonemerge {}", quote(&b.name));
    }
    let auto_hb = mdl.flags & 0x1 != 0 && cx.opts.skip_hitboxes_if_auto;
    if !auto_hb {
        for set in &mdl.hitbox_sets {
            let _ = writeln!(s, "\n$hboxset {}", quote(&set.name));
            for hb in &set.hitboxes {
                let bone = mdl.bones.get(hb.bone.max(0) as usize).map(|b| b.name.clone()).unwrap_or_default();
                let mut line = format!("$hbox {} {} {} {}", hb.group, quote(&bone), fmt3(hb.min), fmt3(hb.max));
                if !hb.name.is_empty() {
                    let _ = write!(line, " {}", quote(&hb.name));
                }
                let _ = writeln!(s, "{line}");
            }
        }
    }
    for c in &mdl.ik_chains {
        let Some(&(last, _)) = c.links.last() else { continue };
        let knee = c.links.iter().map(|l| l.1).find(|k| k.length() > 1e-6).unwrap_or(Vec3::ZERO);
        let bone = mdl.bones.get(last.max(0) as usize).map(|b| b.name.clone()).unwrap_or_default();
        let _ = writeln!(s, "$ikchain {} {} knee {}", quote(&c.name), quote(&bone), fmt3(knee));
    }
    if let Some(vrd) = vrd {
        let _ = writeln!(s, "\n$proceduralbones {}", quote(vrd));
    }
    s.push('\n');
    s.push_str(&cx.jigglebones());
    sections.push(("bones", s));

    // ---- animation
    let mut a = String::new();
    for p in &mdl.pose_params {
        let mut line = format!("$poseparameter {} {} {}", quote(&p.name), fmt(p.start), fmt(p.end));
        if p.loop_range.abs() > 1e-6 {
            let _ = write!(line, " loop {}", fmt(p.loop_range));
        }
        let _ = writeln!(a, "{line}");
    }
    let sectioned: Vec<i32> = mdl.anims.iter().filter(|x| x.sectionframes > 0).map(|x| x.numframes).collect();
    if let (Some(sf), Some(minf)) = (mdl.anims.iter().find(|x| x.sectionframes > 0).map(|x| x.sectionframes), sectioned.iter().min()) {
        let _ = writeln!(a, "$sectionframes {} {}", sf, minf);
    }
    if !mdl.anim_block_name.is_empty() && !mdl.anim_blocks.is_empty() {
        let max = mdl.anim_blocks.iter().map(|(s, e)| (e - s).max(0)).max().unwrap_or(0);
        let _ = writeln!(a, "$animblocksize {}", (max + 1023) / 1024);
    }
    // anim names
    let mut anim_names: HashMap<usize, String> = HashMap::new();
    let mut used: HashSet<String> = mdl.seqs.iter().map(|s| s.name.to_ascii_lowercase()).collect();
    for (ai, ad) in mdl.anims.iter().enumerate() {
        let base = format!("a_{}", sanitize(&ad.name));
        let mut n = base.clone();
        let mut k = 2;
        while used.contains(&n.to_ascii_lowercase()) {
            n = format!("{base}_{k}");
            k += 1;
        }
        used.insert(n.to_ascii_lowercase());
        anim_names.insert(ai, n);
    }
    let refpose_name = "a_gs_refpose".to_string();
    if let Some(rp) = &cx.report.refpose_anim {
        let _ = writeln!(a, "\n// Bind pose, used to rebuild delta (additive) animations exactly\n$animation {} {}", quote(&refpose_name), quote(rp));
    }
    // weightlists
    let mut weightlists: Vec<(String, Vec<f32>)> = Vec::new();
    let mut seq_wl: HashMap<usize, String> = HashMap::new();
    for (si, sq) in mdl.seqs.iter().enumerate() {
        if sq.weightlist.is_empty() || sq.weightlist.iter().all(|w| (*w - 1.0).abs() < 1e-6) {
            continue;
        }
        let name = match weightlists.iter().find(|(_, w)| w == &sq.weightlist) {
            Some((n, _)) => n.clone(),
            None => {
                let n = format!("weights_{}", weightlists.len() + 1);
                weightlists.push((n.clone(), sq.weightlist.clone()));
                n
            }
        };
        seq_wl.insert(si, name);
    }
    // weightlists are declared before the animations (the skeleton is already declared with
    // $definebone) and attached to the animations: studiomdl takes a sequence's weights from
    // its animations.
    let mut wl_text = String::new();
    let mut anim_wl: HashMap<usize, String> = HashMap::new();
    for (si, name) in &seq_wl {
        for &ai in &mdl.seqs[*si].anim_indices {
            anim_wl.entry(ai.max(0) as usize).or_insert_with(|| name.clone());
        }
    }
    for (n, w) in &weightlists {
        let _ = writeln!(wl_text, "\n$weightlist {}\n{{", quote(n));
        for (i, b) in mdl.bones.iter().enumerate() {
            let _ = writeln!(wl_text, "\t{} {}", quote(&b.name), fmt(w.get(i).copied().unwrap_or(1.0)));
        }
        let _ = writeln!(wl_text, "}}");
    }
    a.push_str(&wl_text);
    for (ai, ad) in mdl.anims.iter().enumerate() {
        let Some(file) = cx.report.anim_files.get(&ai) else { continue };
        let _ = writeln!(a, "\n$animation {} {}\n{{", quote(&anim_names[&ai]), quote(file));
        let _ = writeln!(a, "\tfps {}", fmt(ad.fps));
        if let Some(w) = anim_wl.get(&ai) {
            let _ = writeln!(a, "\tweightlist {}", quote(w));
        }
        // `loop` on an $animation makes studiomdl overwrite the last frame with the first;
        // the sequence-level `loop` keeps the frames exact and still flags the animation.
        let used_by_looping_seq = mdl.seqs.iter().any(|s| s.flags & STUDIO_LOOPING != 0 && s.anim_indices.iter().any(|&i| i as usize == ai));
        if ad.flags & STUDIO_LOOPING != 0 {
            if !used_by_looping_seq || loop_frames_match(mdl, ai, cx.input.ani.as_deref()) {
                let _ = writeln!(a, "\tloop");
            } else {
                let _ = writeln!(a, "\t// looping, but the last frame differs from the first: `loop` is set on the sequence only, because on an $animation studiomdl would overwrite the last frame");
            }
        }
        if ad.flags & STUDIO_SNAP != 0 {
            let _ = writeln!(a, "\tsnap");
        }
        if ad.flags & STUDIO_POST != 0 {
            let _ = writeln!(a, "\tpost");
        }
        if ad.flags & STUDIO_DELTA != 0 && cx.report.refpose_anim.is_some() {
            let _ = writeln!(a, "\tsubtract {} 0", quote(&refpose_name));
        }
        // explicit walkframes pin each root-motion piece's end frame (studiomdl otherwise
        // re-detects the range and can end a piece early)
        for mv in &ad.movements {
            let f = motion_flags(mv.motionflags);
            if !f.is_empty() {
                let _ = writeln!(a, "\twalkframe {} {}", mv.endframe, f);
            }
        }
        // with $ikchain present studiomdl invents IK rules unless told not to; the exact
        // original rules are written below instead
        if !mdl.ik_chains.is_empty() {
            let _ = writeln!(a, "\tnoautoik");
        }
        let span = (ad.numframes - 1).max(0) as f32;
        let fr = |v: f32| fmt(((v * span).clamp(-1.0, span.max(0.0)) * 1000.0).round() / 1000.0);
        for r in &ad.ikrules {
            let Some(chain) = mdl.ik_chains.get(r.chain.max(0) as usize) else { continue };
            let kind = match r.kind {
                1 => format!("touch {}", quote(&mdl.bones.get(r.bone.max(0) as usize).map(|b| b.name.clone()).unwrap_or_default())),
                2 => "world".to_string(),
                3 => "footstep".to_string(),
                4 => "release".to_string(),
                5 => format!("attachment {}", quote(&r.attachment)),
                6 => "unlatch".to_string(),
                k => format!("// unknown ik rule type {k}"),
            };
            let mut line = format!("\tikrule {} {kind}", quote(&chain.name));
            if r.kind != 4 {
                let _ = write!(line, " range {} {} {} {}", fr(r.start), fr(r.peak), fr(r.tail), fr(r.end));
            }
            if r.kind == 3 {
                let _ = write!(line, " contact {}", fr(r.contact));
            }
            if r.height.abs() > 1e-6 {
                let _ = write!(line, " height {}", fmt(r.height));
            }
            if r.radius.abs() > 1e-6 {
                let _ = write!(line, " radius {}", fmt(r.radius));
            }
            if r.floor.abs() > 1e-6 {
                let _ = write!(line, " floor {}", fmt(r.floor));
            }
            if r.slot != r.chain && r.slot >= 0 {
                let _ = write!(line, " target {}", r.slot);
            }
            let _ = writeln!(a, "{line}");
        }
        if ad.numikrules as usize > ad.ikrules.len() {
            let _ = writeln!(a, "\t// note: {} ik rule(s) stored in an external animation block were not read", ad.numikrules as usize - ad.ikrules.len());
        }
        let _ = writeln!(a, "}}");
    }
    // sequences
    for (si, sq) in mdl.seqs.iter().enumerate() {
        // placeholders for sequences that live in an $includemodel
        if sq.flags & STUDIO_OVERRIDE != 0 && (sq.groupsize[0] == 0 || sq.anim_indices.iter().any(|&i| i < 0 || i as usize >= mdl.anims.len())) {
            let _ = writeln!(a, "$declaresequence {}", quote(&sq.name));
            continue;
        }
        let _ = writeln!(a, "\n$sequence {}\n{{", quote(&sq.name));
        let names: Vec<String> = sq.anim_indices.iter().map(|&i| anim_names.get(&(i.max(0) as usize)).map(|n| quote(n)).unwrap_or_else(|| "\"\"".into())).collect();
        let _ = writeln!(a, "\t{}", names.join(" "));
        let total = sq.anim_indices.len();
        if total > 1 {
            let _ = writeln!(a, "\tblendwidth {}", sq.groupsize[0].max(1));
            for k in 0..2 {
                if sq.paramindex[k] >= 0 {
                    if let Some(p) = mdl.pose_params.get(sq.paramindex[k] as usize) {
                        let _ = writeln!(a, "\tblend {} {} {}", quote(&p.name), fmt(sq.paramstart[k]), fmt(sq.paramend[k]));
                    }
                }
            }
        }
        if !sq.activity_name.is_empty() {
            let _ = writeln!(a, "\tactivity {} {}", quote(&sq.activity_name), sq.actweight);
        }
        for m in &sq.activity_modifiers {
            let _ = writeln!(a, "\tactivitymodifier {}", quote(m));
        }
        if (sq.fadeintime - 0.2).abs() > 1e-4 {
            let _ = writeln!(a, "\tfadein {}", fmt(sq.fadeintime));
        }
        if (sq.fadeouttime - 0.2).abs() > 1e-4 {
            let _ = writeln!(a, "\tfadeout {}", fmt(sq.fadeouttime));
        }
        let fl = [(STUDIO_LOOPING, "loop"), (STUDIO_SNAP, "snap"), (STUDIO_DELTA, "delta"), (STUDIO_AUTOPLAY, "autoplay"), (STUDIO_POST, "post"), (STUDIO_REALTIME, "realtime"), (STUDIO_HIDDEN, "hidden"), (STUDIO_WORLD, "worldspace"), (STUDIO_NOFORCELOOP, "noforceloop")];
        for (bit, n) in fl {
            if sq.flags & bit != 0 {
                let _ = writeln!(a, "\t{n}");
            }
        }
        if sq.flags & STUDIO_CYCLEPOSE != 0 {
            if let Some(p) = mdl.pose_params.get(sq.cycleposeindex.max(0) as usize) {
                let _ = writeln!(a, "\tcyclepose {}", quote(&p.name));
            }
        }
        if sq.entrynode != 0 || sq.exitnode != 0 {
            let nn = |i: i32| mdl.node_names.get((i - 1).max(0) as usize).cloned().unwrap_or_default();
            if sq.entrynode == sq.exitnode {
                let _ = writeln!(a, "\tnode {}", quote(&nn(sq.entrynode)));
            } else {
                let kw = if sq.nodeflags & 1 != 0 { "rtransition" } else { "transition" };
                let _ = writeln!(a, "\t{kw} {} {}", quote(&nn(sq.entrynode)), quote(&nn(sq.exitnode)));
            }
        }
        let frames = sq.anim_indices.first().and_then(|&i| mdl.anims.get(i.max(0) as usize)).map(|x| x.numframes).unwrap_or(1);
        for ev in &sq.events {
            let frame = ev.cycle * (frames - 1).max(1) as f32;
            let id = if !ev.name.is_empty() { ev.name.clone() } else { ev.event.to_string() };
            let _ = writeln!(a, "\t{{ event {} {} {} }}", id, fmt((frame * 1000.0).round() / 1000.0), quote(&ev.options));
        }
        for al in &sq.autolayers {
            let target = mdl.seqs.get(al.sequence.max(0) as usize).map(|s| s.name.clone()).unwrap_or_default();
            if al.start == 0.0 && al.peak == 0.0 && al.tail == 0.0 && al.end == 0.0 {
                let _ = writeln!(a, "\taddlayer {}", quote(&target));
            } else {
                let scale = if al.flags & 0x4000 != 0 { 1.0 } else { (frames - 1).max(1) as f32 };
                let mut line = format!("\tblendlayer {} {} {} {} {}", quote(&target), fmt(al.start * scale), fmt(al.peak * scale), fmt(al.tail * scale), fmt(al.end * scale));
                if al.flags & 0x40 != 0 {
                    line.push_str(" spline");
                }
                if al.flags & 0x80 != 0 {
                    line.push_str(" xfade");
                }
                if al.flags & 0x200 != 0 {
                    line.push_str(" noblend");
                }
                if al.flags & 0x1000 != 0 {
                    line.push_str(" local");
                }
                if al.flags & 0x4000 != 0 {
                    if let Some(p) = mdl.pose_params.get(al.pose.max(0) as usize) {
                        let _ = write!(line, " poseparameter {}", quote(&p.name));
                    }
                }
                let _ = writeln!(a, "{line}");
            }
        }
        if let Some(w) = seq_wl.get(&si) {
            let _ = writeln!(a, "\tweightlist {}", quote(w));
        }
        for l in &sq.iklocks {
            if let Some(c) = mdl.ik_chains.get(l.chain.max(0) as usize) {
                let _ = writeln!(a, "\tiklock {} {} {}", quote(&c.name), fmt(l.pos_weight), fmt(l.local_q_weight));
            }
        }
        if !sq.keyvalues.trim().is_empty() {
            let _ = writeln!(a, "\tkeyvalues\n\t{{\n{}\n\t}}", sq.keyvalues.trim());
        }
        let _ = writeln!(a, "}}");
    }
    sections.push(("animations", a));

    // ---- collision
    let mut c = String::new();
    if let (Some(file), Some(phy)) = (phys_file, &cx.phy) {
        let edit = phy.block("editparams");
        let ragdoll = phy.solids.len() > 1 || phy.blocks("ragdollconstraint").next().is_some();
        let cmd = if ragdoll { "$collisionjoints" } else { "$collisionmodel" };
        let _ = writeln!(c, "{cmd} {}\n{{", quote(file));
        let infos = phy.solid_infos();
        // studiomdl gives solid i the mass  total * vol_i * bias_i / sum(vol_j * bias_j).
        // If the stored masses don't follow that (they were edited after compiling, e.g. in
        // HLMV), solve for biases that reproduce them exactly.
        let num = |kv: &crate::formats::keyvalues::Kv, k: &str| kv.get_str(k).and_then(|s| s.trim().parse::<f64>().ok());
        let total = edit.and_then(|e| e.get_str("totalmass")).and_then(|s| s.trim().parse::<f64>().ok());
        let masses: Vec<Option<(f64, f64, f64)>> = infos.iter().map(|(_, _, kv)| Some((num(kv, "mass")?, num(kv, "volume")?, num(kv, "massbias").unwrap_or(1.0)))).collect();
        let mut exact_biases: Option<(f64, Vec<f64>)> = None;
        if ragdoll && masses.iter().all(|m| m.is_some()) && !masses.is_empty() {
            let m: Vec<(f64, f64, f64)> = masses.iter().flatten().copied().collect();
            let denom: f64 = m.iter().map(|(_, v, b)| v * b).sum();
            let consistent = total.map(|t| denom > 0.0 && m.iter().all(|(mass, v, b)| (t * v * b / denom - mass).abs() <= 0.51)).unwrap_or(false);
            if !consistent && m.iter().all(|(_, v, _)| *v > 0.0) {
                let sum: f64 = m.iter().map(|(mass, _, _)| mass).sum();
                exact_biases = Some((sum, m.iter().map(|(mass, v, _)| mass / v * 1000.0).collect()));
            }
        }
        if let Some((sum, _)) = &exact_biases {
            let _ = writeln!(c, "\t// per-bone masses were edited after compiling; $mass and $jointmassbias below reproduce them exactly");
            let _ = writeln!(c, "\t$mass {}", fmt(*sum as f32));
        } else if let Some(v) = edit.and_then(|e| e.get_str("totalmass")) {
            let _ = writeln!(c, "\t$mass {v}");
        }
        if let Some(e) = edit {
            if let Some(v) = e.get_str("rootname") {
                if !v.is_empty() {
                    let _ = writeln!(c, "\t$rootbone {}", quote(v));
                }
            }
            if e.get_str("concave") == Some("1") {
                let _ = writeln!(c, "\t$concave");
            }
            for jm in e.children().iter().filter(|k| k.key.eq_ignore_ascii_case("jointmerge")) {
                if let Some(v) = jm.str() {
                    let parts: Vec<&str> = v.split(',').collect();
                    if parts.len() == 2 {
                        let _ = writeln!(c, "\t$jointmerge {} {}", quote(parts[0].trim()), quote(parts[1].trim()));
                    }
                }
            }
        }
        if let Some((_, _, first)) = infos.first() {
            for (k, qk) in [("inertia", "$inertia"), ("damping", "$damping"), ("rotdamping", "$rotdamping")] {
                if let Some(v) = first.get_str(k) {
                    let _ = writeln!(c, "\t{qk} {v}");
                }
            }
            // per-joint overrides
            for (_, name, kv) in infos.iter().skip(1) {
                for (k, qk) in [("inertia", "$jointinertia"), ("damping", "$jointdamping"), ("rotdamping", "$jointrotdamping")] {
                    if let (Some(v), Some(d)) = (kv.get_str(k), first.get_str(k)) {
                        if v != d {
                            let _ = writeln!(c, "\t{qk} {} {v}", quote(name));
                        }
                    }
                }
            }
        }
        for (k, (_, name, kv)) in infos.iter().enumerate() {
            if let Some((_, biases)) = &exact_biases {
                let _ = writeln!(c, "\t$jointmassbias {} {:.6}", quote(name), biases[k]);
            } else if let Some(mb) = kv.get_str("massbias") {
                let _ = writeln!(c, "\t$jointmassbias {} {mb}", quote(name));
            }
        }
        for rc in phy.blocks("ragdollconstraint") {
            let child: usize = rc.get_str("child").and_then(|s| s.parse().ok()).unwrap_or(0);
            let name = infos.iter().find(|i| i.0 == child).map(|i| i.1.clone()).unwrap_or_default();
            for ax in ["x", "y", "z"] {
                let g = |k: &str| rc.get_str(&format!("{ax}{k}")).unwrap_or("0").to_string();
                let _ = writeln!(c, "\t$jointconstrain {} {ax} limit {} {} {}", quote(&name), g("min"), g("max"), g("friction"));
            }
        }
        if let Some(cr) = phy.block("collisionrules") {
            if cr.get_str("selfcollisions") == Some("0") {
                let _ = writeln!(c, "\t$noselfcollisions");
            }
            for p in cr.children().iter().filter(|k| k.key.eq_ignore_ascii_case("collisionpair")) {
                if let Some(v) = p.str() {
                    let parts: Vec<usize> = v.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                    if parts.len() == 2 {
                        let a1 = infos.iter().find(|i| i.0 == parts[0]).map(|i| i.1.clone()).unwrap_or_default();
                        let a2 = infos.iter().find(|i| i.0 == parts[1]).map(|i| i.1.clone()).unwrap_or_default();
                        let _ = writeln!(c, "\t$jointcollide {} {}", quote(&a1), quote(&a2));
                    }
                }
            }
        }
        let _ = writeln!(c, "}}");
        // per-bone collision surface properties differing from the model's
        for (_, name, kv) in &infos {
            if let Some(sp) = kv.get_str("surfaceprop") {
                if !sp.eq_ignore_ascii_case(&mdl.surfaceprop) {
                    let _ = writeln!(c, "$jointsurfaceprop {} {}", quote(name), quote(sp));
                }
            }
        }
        // anything else in the text section (break pieces, custom blocks) round-trips through $collisiontext
        let known = ["solid", "ragdollconstraint", "collisionrules", "editparams", "materialtable", "vehicle"];
        let extra: Vec<crate::formats::keyvalues::Kv> = phy.kv.iter().filter(|k| !known.iter().any(|n| k.key.eq_ignore_ascii_case(n))).cloned().collect();
        if !extra.is_empty() {
            let _ = writeln!(c, "\n$collisiontext\n{{\n{}}}", crate::formats::keyvalues::write(&extra, 1));
        }
    }
    sections.push(("collision", c));
    sections
}

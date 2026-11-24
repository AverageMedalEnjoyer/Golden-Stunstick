//! Decompile accuracy verification.
//!
//! Two levels:
//! 1. `self_check` re-reads every SMD the decompiler wrote and compares it
//!    with the data decoded straight from the MDL (skeleton, frames, meshes).
//! 2. `compare_models` compares the original MDL with one recompiled from the
//!    decompiled QC by studiomdl (bones, attachments, hitboxes, meshes,
//!    sequences, per-frame animation data, physics...).

use crate::decompile::{self, DecompileReport};
use crate::formats::mdl::{Mdl, STUDIO_DELTA, STUDIO_FRAMEANIM, Vtx, Vvd};
use crate::formats::phy::Phy;
use crate::formats::smd::Smd;
use glam::{Quat, Vec3};
use std::path::Path;

#[derive(Clone, Debug)]
pub struct Check {
    pub category: String,
    pub ok: bool,
    pub detail: String,
}

fn check(out: &mut Vec<Check>, category: &str, ok: bool, detail: impl Into<String>) {
    out.push(Check { category: category.into(), ok, detail: detail.into() });
}

/// Angle between two rotations in degrees. Uses atan2 in f64: acos(dot) in f32
/// cannot resolve angles below ~0.06 degrees.
fn qangle_err(a: Quat, b: Quat) -> f32 {
    let qa = glam::DQuat::from_xyzw(a.x as f64, a.y as f64, a.z as f64, a.w as f64).normalize();
    let qb = glam::DQuat::from_xyzw(b.x as f64, b.y as f64, b.z as f64, b.w as f64).normalize();
    let d = qa.inverse() * qb;
    let v = glam::DVec3::new(d.x, d.y, d.z).length();
    (2.0 * v.atan2(d.w.abs())).to_degrees() as f32
}

/// Compare the decompiled SMD files with values decoded directly from the MDL.
pub fn self_check(mdl: &Mdl, ani: Option<&[u8]>, vvd: Option<&[u8]>, vtx: Option<&[u8]>, report: &DecompileReport, out_dir: &Path) -> Vec<Check> {
    let mut out = Vec::new();
    // animations
    let mut worst_pos = 0.0f32;
    let mut worst_rot = 0.0f32;
    let mut bad_frames = 0;
    let mut read_fail = Vec::new();
    for (ai, rel) in &report.anim_files {
        let p = out_dir.join(rel);
        let Ok(text) = std::fs::read_to_string(&p) else {
            read_fail.push(rel.clone());
            continue;
        };
        let smd = match Smd::parse(&text) {
            Ok(s) => s,
            Err(e) => {
                read_fail.push(format!("{rel}: {e}"));
                continue;
            }
        };
        if smd.nodes.len() != mdl.bones.len() || smd.nodes.iter().zip(&mdl.bones).any(|(n, b)| n.name != b.name || n.parent != b.parent) {
            read_fail.push(format!("{rel}: node list differs from MDL skeleton"));
            continue;
        }
        let expected = decompile::anim_frames(mdl, *ai, ani);
        if smd.frames.len() != expected.len() {
            read_fail.push(format!("{rel}: {} frames written, {} expected", smd.frames.len(), expected.len()));
            continue;
        }
        for (f, (got, exp)) in smd.frames.iter().zip(expected.iter()).enumerate() {
            let _ = f;
            for (g, e) in got.iter().zip(exp.iter()) {
                let dp = (g.0 - e.0).length();
                let dr = qangle_err(crate::formats::mdl::euler_to_quat(g.1), crate::formats::mdl::euler_to_quat(e.1));
                worst_pos = worst_pos.max(dp);
                worst_rot = worst_rot.max(dr);
                if dp > 0.01 || dr > 0.05 {
                    bad_frames += 1;
                    if std::env::var("GS_DEBUG_FRAMES").is_ok() {
                        eprintln!("frame mismatch {rel}: got {g:?} expected {e:?} dr {dr}");
                    }
                }
            }
        }
    }
    check(
        &mut out,
        "Animation SMDs",
        read_fail.is_empty() && bad_frames == 0,
        if read_fail.is_empty() {
            format!("{} animation file(s) re-read; worst position error {:.5}, worst rotation error {:.4} deg", report.anim_files.len(), worst_pos, worst_rot)
        } else {
            format!("Problems: {}", read_fail.join("; "))
        },
    );

    // meshes: total triangle count vs VTX LOD0
    if let (Some(vvd), Some(vtx)) = (vvd.and_then(|d| Vvd::parse(d).ok()), vtx.and_then(|d| Vtx::parse(d).ok())) {
        let mut expected_tris = 0usize;
        for (bi, bp) in mdl.bodyparts.iter().enumerate() {
            for (mi, m) in bp.models.iter().enumerate() {
                if m.meshes.is_empty() || m.num_vertices == 0 {
                    continue;
                }
                if let Some(l) = vtx.bodyparts.get(bi).and_then(|b| b.get(mi)).and_then(|x| x.lods.first()) {
                    expected_tris += l.iter().map(|me| me.tris.len()).sum::<usize>();
                }
            }
        }
        let mut written = 0usize;
        let mut weights_ok = true;
        let lods: Vec<&std::path::PathBuf> = report.files.iter().filter(|f| f.extension().map(|e| e == "smd").unwrap_or(false)).collect();
        for f in lods {
            let name = f.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if name.contains("_lod") || name.contains("_physics") || report.anim_files.values().any(|a| f.ends_with(a)) || name.starts_with("gs_refpose") {
                continue;
            }
            if let Ok(smd) = std::fs::read_to_string(f).map_err(|e| e.to_string()).and_then(|t| Smd::parse(&t)) {
                written += smd.triangles.len();
                for t in &smd.triangles {
                    for v in &t.verts {
                        let s: f32 = v.links.iter().map(|l| l.1).sum();
                        if !v.links.is_empty() && (s - 1.0).abs() > 0.02 {
                            weights_ok = false;
                        }
                    }
                }
            }
        }
        check(&mut out, "Reference meshes", written == expected_tris, format!("{written} triangles written, {expected_tris} in the model's LOD0 ({} vertices in VVD)", vvd.vertices.len()));
        check(&mut out, "Vertex weights", weights_ok, if weights_ok { "All vertex weights sum to 1" } else { "Some vertex weights do not sum to 1" });
    }
    out
}

fn tol_vec(a: Vec3, b: Vec3, t: f32) -> bool {
    (a - b).length() <= t
}

/// Root bones moved by the animation's extracted movement (piecewise root motion), the
/// same transform the decompiler applies when it writes SMD frames.
fn with_root_motion(m: &Mdl, ai: usize, frame: i32, mut pose: Vec<(Vec3, Quat)>) -> Vec<(Vec3, Quat)> {
    let ad = &m.anims[ai];
    if ad.flags & STUDIO_DELTA != 0 || ad.movements.is_empty() {
        return pose;
    }
    let (mp, yaw) = m.movement_at(ai, frame as f32);
    let mq = Quat::from_rotation_z(yaw.to_radians());
    for (i, b) in m.bones.iter().enumerate() {
        if b.parent < 0 {
            pose[i] = (mq * pose[i].0 + mp, (mq * pose[i].1).normalize());
        }
    }
    pose
}

/// "66.2 units forward" style summary of a movement piece's cumulative position/yaw.
fn fmt_move(p: Vec3, yaw: f32) -> String {
    let mut s = format!("{:.2} units", p.length());
    if yaw.abs() > 0.05 {
        s.push_str(&format!(", yaw {yaw:.1} deg"));
    }
    s
}

/// Compare an original model with one recompiled from the decompiled QC.
#[allow(clippy::too_many_arguments)]
pub fn compare_models(a: &Mdl, a_ani: Option<&[u8]>, b: &Mdl, b_ani: Option<&[u8]>, a_vtx: Option<&Vtx>, b_vtx: Option<&Vtx>, a_vvd: Option<&Vvd>, b_vvd: Option<&Vvd>, a_phy: Option<&Phy>, b_phy: Option<&Phy>) -> Vec<Check> {
    let mut out = Vec::new();
    // ---- file format: a studiomdl from a newer engine branch (L4D2, CS:GO...) writes version 49+,
    // which version-48 games cannot load. Older originals (44-47) recompiled as 48 are fine.
    if b.version > a.version.max(48) {
        check(
            &mut out,
            "MDL version",
            false,
            format!(
                "The original is MDL version {} but this studiomdl wrote version {}. A game that loads version {} models (Source SDK 2013, TF2, HL2, ...) cannot load the recompiled model. Use the studiomdl.exe from the same engine branch as the game, e.g. Source SDK Base 2013's bin folder.",
                a.version, b.version, a.version
            ),
        );
    }
    // ---- bones
    let mut diffs = Vec::new();
    if a.bones.len() != b.bones.len() {
        diffs.push(format!("bone count {} vs {}", a.bones.len(), b.bones.len()));
    }
    for (x, y) in a.bones.iter().zip(&b.bones) {
        if x.name != y.name {
            diffs.push(format!("bone name '{}' vs '{}'", x.name, y.name));
            continue;
        }
        let pn = |m: &Mdl, p: i32| if p >= 0 { m.bones[p as usize].name.clone() } else { "<root>".into() };
        if pn(a, x.parent) != pn(b, y.parent) {
            diffs.push(format!("'{}' parent {} vs {}", x.name, pn(a, x.parent), pn(b, y.parent)));
        }
        if !tol_vec(x.pos, y.pos, 0.01) {
            diffs.push(format!("'{}' position {:?} vs {:?}", x.name, x.pos, y.pos));
        }
        if qangle_err(x.quat, y.quat) > 0.1 {
            diffs.push(format!("'{}' rotation differs by {:.3} deg", x.name, qangle_err(x.quat, y.quat)));
        }
    }
    check(&mut out, "Skeleton", diffs.is_empty(), if diffs.is_empty() { format!("{} bones identical (names, parents, bind pose)", a.bones.len()) } else { summarize(&diffs) });

    // ---- attachments
    let mut diffs = Vec::new();
    if a.attachments.len() != b.attachments.len() {
        diffs.push(format!("count {} vs {}", a.attachments.len(), b.attachments.len()));
    }
    for (x, y) in a.attachments.iter().zip(&b.attachments) {
        let bn = |m: &Mdl, i: i32| m.bones.get(i.max(0) as usize).map(|b| b.name.clone()).unwrap_or_default();
        if x.name != y.name || bn(a, x.bone) != bn(b, y.bone) {
            diffs.push(format!("'{}'@{} vs '{}'@{}", x.name, bn(a, x.bone), y.name, bn(b, y.bone)));
        }
        let mut md = 0.0f32;
        for r in 0..3 {
            for c in 0..4 {
                md = md.max((x.local[r][c] - y.local[r][c]).abs());
            }
        }
        if md > 0.01 {
            diffs.push(format!("'{}' transform differs (max {md:.4})", x.name));
        }
    }
    check(&mut out, "Attachments", diffs.is_empty(), if diffs.is_empty() { format!("{} attachments identical", a.attachments.len()) } else { summarize(&diffs) });

    // ---- hitboxes
    let mut diffs = Vec::new();
    if a.hitbox_sets.len() != b.hitbox_sets.len() {
        diffs.push(format!("set count {} vs {}", a.hitbox_sets.len(), b.hitbox_sets.len()));
    }
    for (x, y) in a.hitbox_sets.iter().zip(&b.hitbox_sets) {
        if x.hitboxes.len() != y.hitboxes.len() {
            diffs.push(format!("set '{}' has {} vs {} boxes", x.name, x.hitboxes.len(), y.hitboxes.len()));
            continue;
        }
        for (h, k) in x.hitboxes.iter().zip(&y.hitboxes) {
            if h.bone != k.bone || h.group != k.group || !tol_vec(h.min, k.min, 0.01) || !tol_vec(h.max, k.max, 0.01) {
                diffs.push(format!("box on bone {} differs", h.bone));
            }
        }
    }
    check(&mut out, "Hitboxes", diffs.is_empty(), if diffs.is_empty() { "Hitbox sets identical".to_string() } else { summarize(&diffs) });

    // ---- body / meshes
    let mut diffs = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    if a.bodyparts.len() != b.bodyparts.len() {
        diffs.push(format!("bodypart count {} vs {}", a.bodyparts.len(), b.bodyparts.len()));
    }
    for (bi, (x, y)) in a.bodyparts.iter().zip(&b.bodyparts).enumerate() {
        if x.name != y.name || x.models.len() != y.models.len() {
            diffs.push(format!("bodypart '{}' ({} models) vs '{}' ({} models)", x.name, x.models.len(), y.name, y.models.len()));
            continue;
        }
        for (mi, (m, n)) in x.models.iter().zip(&y.models).enumerate() {
            if m.meshes.len() != n.meshes.len() {
                diffs.push(format!("'{}' model {mi}: {} vs {} meshes", x.name, m.meshes.len(), n.meshes.len()));
            }
            let geo_a = model_geometry(a, a_vtx, a_vvd, bi, mi);
            let geo_b = model_geometry(b, b_vtx, b_vvd, bi, mi);
            let geometry_same = geo_a.is_some() && (geo_a == geo_b || (geo_b.is_some() && geo_a.as_ref().map(|v| v.len()) == geo_b.as_ref().map(|v| v.len()) && unmatched_tris(geo_a.as_ref().unwrap(), geo_b.as_ref().unwrap()) == 0));
            if m.num_vertices != n.num_vertices {
                if geometry_same {
                    notes.push(format!("'{}' model {mi}: triangles identical, but the original compiler welded vertices differently ({} vs {} unique vertices)", x.name, m.num_vertices, n.num_vertices));
                } else {
                    diffs.push(format!("'{}' model {mi}: {} vs {} vertices", x.name, m.num_vertices, n.num_vertices));
                }
            }
            if geo_a.is_some() && geo_b.is_some() && !geometry_same {
                let (ga, gb) = (geo_a.clone().unwrap_or_default(), geo_b.clone().unwrap_or_default());
                let missing = unmatched_tris(&ga, &gb);
                if missing > 0 || ga.len() != gb.len() {
                    diffs.push(format!("'{}' model {mi}: {missing} of {} triangle(s) differ in position/UV", x.name, ga.len()));
                } else if m.num_vertices == n.num_vertices {
                    notes.push(format!("'{}' model {mi}: some vertices differ by float rounding only", x.name));
                }
            }
            if let (Some(va), Some(vb)) = (a_vtx, b_vtx) {
                let ta: usize = va.bodyparts.get(bi).and_then(|p| p.get(mi)).and_then(|q| q.lods.first()).map(|l| l.iter().map(|z| z.tris.len()).sum()).unwrap_or(0);
                let tb: usize = vb.bodyparts.get(bi).and_then(|p| p.get(mi)).and_then(|q| q.lods.first()).map(|l| l.iter().map(|z| z.tris.len()).sum()).unwrap_or(0);
                if ta != tb {
                    diffs.push(format!("'{}' model {mi}: {ta} vs {tb} triangles", x.name));
                }
                let la = va.bodyparts.get(bi).and_then(|p| p.get(mi)).map(|q| q.lods.len()).unwrap_or(0);
                let lb = vb.bodyparts.get(bi).and_then(|p| p.get(mi)).map(|q| q.lods.len()).unwrap_or(0);
                if la != lb {
                    diffs.push(format!("'{}' model {mi}: {la} vs {lb} LODs", x.name));
                }
            }
        }
    }
    check(
        &mut out,
        "Meshes & bodygroups",
        diffs.is_empty(),
        if diffs.is_empty() {
            if notes.is_empty() { "Bodygroups, meshes, triangles (positions + UVs) identical".to_string() } else { format!("Geometry identical. Note: {}", summarize(&notes)) }
        } else {
            summarize(&diffs)
        },
    );

    // ---- materials
    let mut diffs = Vec::new();
    let mut mat_notes = Vec::new();
    let used = |m: &Mdl| {
        let mut v: Vec<String> = Vec::new();
        for bp in &m.bodyparts {
            for md in &bp.models {
                for me in &md.meshes {
                    for f in 0..m.skins.len().max(1) {
                        if let Some(t) = m.mesh_texture(me.material, f) {
                            let n = t.name.to_ascii_lowercase();
                            if !v.contains(&n) {
                                v.push(n);
                            }
                        }
                    }
                }
            }
        }
        v.sort();
        v
    };
    let tn = |m: &Mdl| m.textures.iter().map(|t| t.name.to_ascii_lowercase()).collect::<Vec<_>>();
    if used(a) != used(b) {
        diffs.push(format!("materials used by meshes {:?} vs {:?}", used(a), used(b)));
    } else if tn(a) != tn(b) {
        let extra: Vec<String> = tn(a).into_iter().filter(|t| !used(a).contains(t)).collect();
        mat_notes.push(format!("the original also lists material(s) no mesh uses: {extra:?}"));
    }
    let cd = |m: &Mdl| m.cdmaterials.iter().map(|t| t.to_ascii_lowercase().replace('\\', "/")).collect::<Vec<_>>();
    if cd(a) != cd(b) {
        diffs.push(format!("$cdmaterials {:?} vs {:?}", cd(a), cd(b)));
    }
    let skin_names = |m: &Mdl| m.skins.iter().map(|f| f.iter().map(|&i| m.textures.get(i as usize).map(|t| t.name.to_ascii_lowercase()).unwrap_or_default()).collect::<Vec<_>>()).collect::<Vec<_>>();
    let used_cols = |m: &Mdl| -> Vec<Vec<String>> {
        let cols: Vec<i32> = {
            let mut c: Vec<i32> = m.bodyparts.iter().flat_map(|bp| bp.models.iter()).flat_map(|md| md.meshes.iter()).map(|me| me.material).collect();
            c.sort();
            c.dedup();
            c
        };
        (0..m.skins.len().max(1)).map(|f| cols.iter().map(|&c| m.mesh_texture(c, f).map(|t| t.name.to_ascii_lowercase()).unwrap_or_default()).collect()).collect()
    };
    if skin_names(a) != skin_names(b) && used_cols(a) != used_cols(b) {
        diffs.push(format!("skin families differ ({} vs {})", a.skins.len(), b.skins.len()));
    }
    check(
        &mut out,
        "Materials & skins",
        diffs.is_empty(),
        if diffs.is_empty() {
            let base = format!("{} materials used by meshes, {} skin families identical", used(a).len(), a.skins.len());
            if mat_notes.is_empty() { base } else { format!("{base}. Note: {}", mat_notes.join("; ")) }
        } else {
            summarize(&diffs)
        },
    );

    // ---- sequences
    let mut diffs = Vec::new();
    if a.seqs.len() != b.seqs.len() {
        diffs.push(format!("sequence count {} vs {}", a.seqs.len(), b.seqs.len()));
    }
    for (x, y) in a.seqs.iter().zip(&b.seqs) {
        if !x.name.eq_ignore_ascii_case(&y.name) {
            diffs.push(format!("sequence '{}' vs '{}'", x.name, y.name));
            continue;
        }
        if x.activity_name != y.activity_name || x.actweight != y.actweight {
            diffs.push(format!("'{}' activity {} {} vs {} {}", x.name, x.activity_name, x.actweight, y.activity_name, y.actweight));
        }
        let mask = 0x1 | 0x2 | 0x4 | 0x8 | 0x10 | 0x100 | 0x400 | 0x4000 | 0x8000;
        if x.flags & mask != y.flags & mask {
            diffs.push(format!("'{}' flags {:#x} vs {:#x}", x.name, x.flags & mask, y.flags & mask));
        }
        if x.groupsize != y.groupsize {
            diffs.push(format!("'{}' blend grid {:?} vs {:?}", x.name, x.groupsize, y.groupsize));
        }
        if (x.fadeintime - y.fadeintime).abs() > 1e-3 || (x.fadeouttime - y.fadeouttime).abs() > 1e-3 {
            diffs.push(format!("'{}' fade times differ", x.name));
        }
        if x.events.len() != y.events.len() {
            diffs.push(format!("'{}' {} vs {} events", x.name, x.events.len(), y.events.len()));
        } else {
            for (e, f) in x.events.iter().zip(&y.events) {
                if (e.cycle - f.cycle).abs() > 0.01 || e.options != f.options || (e.name != f.name && e.event != f.event) {
                    diffs.push(format!("'{}' event {} at {:.3} vs {} at {:.3}", x.name, e.name, e.cycle, f.name, f.cycle));
                }
            }
        }
        if x.autolayers.len() != y.autolayers.len() {
            diffs.push(format!("'{}' {} vs {} layers", x.name, x.autolayers.len(), y.autolayers.len()));
        }
        if x.weightlist.len() == y.weightlist.len() && x.weightlist.iter().zip(&y.weightlist).any(|(p, q)| (p - q).abs() > 1e-3) {
            diffs.push(format!("'{}' weightlist differs", x.name));
        }
    }
    check(&mut out, "Sequences", diffs.is_empty(), if diffs.is_empty() { format!("{} sequences identical (names, activities, flags, blends, events, layers)", a.seqs.len()) } else { summarize(&diffs) });

    // ---- animation data, matched through sequences
    let mut diffs = Vec::new();
    let mut worst_p = 0.0f32;
    let mut worst_r = 0.0f32;
    let mut worst_p_at = String::new();
    let mut loop_notes: Vec<String> = Vec::new();
    let mut loop_frame_diffs: Vec<String> = Vec::new();
    let mut split_notes: Vec<String> = Vec::new();
    let mut frameanim_skipped: Vec<String> = Vec::new();
    let mut worst_r_at = String::new();
    let mut compared = 0usize;
    let mut done = std::collections::HashSet::new();
    let bone_map: Vec<Option<usize>> = a.bones.iter().map(|x| b.bone_index(&x.name)).collect();
    for (x, y) in a.seqs.iter().zip(&b.seqs) {
        for (&ia, &ib) in x.anim_indices.iter().zip(&y.anim_indices) {
            let (ia, ib) = (ia.max(0) as usize, ib.max(0) as usize);
            if !done.insert(ia) || ia >= a.anims.len() || ib >= b.anims.len() {
                continue;
            }
            let (da, db) = (&a.anims[ia], &b.anims[ib]);
            if da.numframes != db.numframes {
                diffs.push(format!("'{}' {} vs {} frames", da.name, da.numframes, db.numframes));
                continue;
            }
            if (da.fps - db.fps).abs() > 1e-3 {
                diffs.push(format!("'{}' fps {} vs {}", da.name, da.fps, db.fps));
            }
            if (da.flags & STUDIO_FRAMEANIM) != (db.flags & STUDIO_FRAMEANIM) {
                // the frames cannot be decoded side by side, so comparing them would only report noise
                frameanim_skipped.push(da.name.clone());
                continue;
            }
            if (da.flags & STUDIO_DELTA) != (db.flags & STUDIO_DELTA) {
                diffs.push(format!("'{}' delta flag differs", da.name));
            }
            if (da.flags & 1) != (db.flags & 1) {
                // studiomdl cannot set an animation's loop flag without rewriting its last frame
                if x.flags & 1 != 0 && y.flags & 1 != 0 {
                    loop_notes.push(da.name.clone());
                } else {
                    diffs.push(format!("'{}' loop flag differs", da.name));
                }
            }
            if da.movements.len() != db.movements.len() {
                diffs.push(format!("'{}' {} vs {} movement pieces", da.name, da.movements.len(), db.movements.len()));
            } else {
                for (m, n) in da.movements.iter().zip(&db.movements) {
                    if m.endframe != n.endframe || m.motionflags != n.motionflags {
                        diffs.push(format!("'{}' movement piece ends at frame {} ({}) vs {} ({})", da.name, m.endframe, decompile::motion_flags(m.motionflags), n.endframe, decompile::motion_flags(n.motionflags)));
                    } else if !tol_vec(m.position, n.position, 0.05) || (m.angle - n.angle).abs() > 0.05 {
                        // Same pieces, different distances. The per-frame comparison below runs in
                        // world space, so it still fails if the animation itself moves differently.
                        split_notes.push(format!("{} frame {}: {} vs {}", da.name, m.endframe, fmt_move(m.position, m.angle), fmt_move(n.position, n.angle)));
                    }
                }
            }
            let step = (da.numframes / 40).max(1);
            let mut f = 0;
            let mut frames: Vec<i32> = Vec::new();
            while f < da.numframes {
                frames.push(f);
                f += step;
            }
            if frames.last() != Some(&(da.numframes - 1)) && da.numframes > 1 {
                frames.push(da.numframes - 1);
            }
            for f in frames {
                // root bones are compared in world space (local + extracted movement): compilers
                // may split the same motion differently between the bone and the movement track
                let pa = with_root_motion(a, ia, f, a.decode_frame(ia, f, a_ani));
                let pb = with_root_motion(b, ib, f, b.decode_frame(ib, f, b_ani));
                let loop_fixup_frame = da.flags & 1 != 0 && f == da.numframes - 1 && da.numframes > 1;
                for (bi, m) in bone_map.iter().enumerate() {
                    let Some(m) = m else { continue };
                    let dp = (pa[bi].0 - pb[*m].0).length();
                    let dr = qangle_err(pa[bi].1, pb[*m].1);
                    if loop_fixup_frame && (dp > 0.1 || dr > 1.0) {
                        // studiomdl blends a looping animation's last frame toward its first
                        loop_frame_diffs.push(format!("{} bone {}", da.name, a.bones[bi].name));
                        continue;
                    }
                    if dp > worst_p {
                        worst_p = dp;
                        worst_p_at = format!("{} frame {f} bone {}", da.name, a.bones[bi].name);
                    }
                    if dr > worst_r {
                        worst_r = dr;
                        worst_r_at = format!("{} frame {f} bone {}", da.name, a.bones[bi].name);
                    }
                }
                compared += 1;
            }
        }
    }
    if !frameanim_skipped.is_empty() {
        diffs.push(format!(
            "{} animation(s) were not compared because one model stores them in the frame-anim encoding (written by newer-branch studiomdl builds such as Left 4 Dead 2 / CS:GO, not loadable by Source SDK 2013 games): {}",
            frameanim_skipped.len(),
            summarize(&frameanim_skipped)
        ));
    }
    let anim_ok = diffs.is_empty() && worst_p < 0.1 && worst_r < 1.0;
    let mut notes = String::new();
    if !loop_notes.is_empty() {
        notes.push_str(&format!(
            ". Note: {} looping animation(s) whose last frame differs from the first keep exact frames with `loop` on their sequence instead of the animation (studiomdl would otherwise overwrite the last frame): {}",
            loop_notes.len(),
            summarize(&loop_notes)
        ));
    }
    if !loop_frame_diffs.is_empty() {
        notes.push_str(&format!(". Note: studiomdl's loop fix-up changed the last frame of: {}", summarize(&loop_frame_diffs)));
    }
    if !split_notes.is_empty() {
        notes.push_str(&format!(
            ". Note: {} movement piece(s) split the same root motion differently (the animation looks identical, but the entity's movement distance differs; this studiomdl always measures a piece from its start frame to its end frame, the original compiler did not): {}",
            split_notes.len(),
            summarize(&split_notes)
        ));
    }
    check(
        &mut out,
        "Animation data",
        anim_ok,
        format!(
            "{compared} frames compared; worst bone position error {worst_p:.4} units{}, worst rotation error {worst_r:.3} deg{}{}{notes}",
            if worst_p > 0.001 { format!(" ({worst_p_at})") } else { String::new() },
            if worst_r > 0.001 { format!(" ({worst_r_at})") } else { String::new() },
            if diffs.is_empty() { String::new() } else { format!("; {}", summarize(&diffs)) }
        ),
    );

    // ---- misc header
    let mut diffs = Vec::new();
    if !tol_vec(a.eyeposition, b.eyeposition, 0.01) {
        diffs.push(format!("eyeposition {:?} vs {:?}", a.eyeposition, b.eyeposition));
    }
    if !tol_vec(a.illumposition, b.illumposition, 0.01) {
        diffs.push(format!("illumposition {:?} vs {:?}", a.illumposition, b.illumposition));
    }
    if !tol_vec(a.hull_min, b.hull_min, 0.01) || !tol_vec(a.hull_max, b.hull_max, 0.01) {
        diffs.push("$cbox (hull) differs".into());
    }
    if !tol_vec(a.view_bbmin, b.view_bbmin, 0.01) || !tol_vec(a.view_bbmax, b.view_bbmax, 0.01) {
        diffs.push("$bbox (view box) differs".into());
    }
    if !a.surfaceprop.eq_ignore_ascii_case(&b.surfaceprop) {
        diffs.push(format!("surfaceprop '{}' vs '{}'", a.surfaceprop, b.surfaceprop));
    }
    if a.contents != b.contents {
        diffs.push(format!("contents {:#x} vs {:#x}", a.contents, b.contents));
    }
    let fm = 0x4 | 0x8 | 0x10 | 0x200 | 0x800 | 0x1000 | 0x2000 | 0x10000 | 0x20000 | 0x40000;
    if a.flags & fm != b.flags & fm {
        diffs.push(format!("model flags {:#x} vs {:#x}", a.flags & fm, b.flags & fm));
    }
    let pp = |m: &Mdl| m.pose_params.iter().map(|p| format!("{} {:.2} {:.2}", p.name, p.start, p.end)).collect::<Vec<_>>();
    if pp(a) != pp(b) {
        diffs.push(format!("pose parameters {:?} vs {:?}", pp(a), pp(b)));
    }
    let ik = |m: &Mdl| m.ik_chains.iter().map(|c| c.name.clone()).collect::<Vec<_>>();
    if ik(a) != ik(b) {
        diffs.push("ik chains differ".into());
    }
    let inc = |m: &Mdl| m.include_models.iter().map(|c| c.name.to_ascii_lowercase()).collect::<Vec<_>>();
    if inc(a) != inc(b) {
        diffs.push(format!("include models {:?} vs {:?}", inc(a), inc(b)));
    }
    if a.flex_descs.len() != b.flex_descs.len() || a.flex_controllers.len() != b.flex_controllers.len() {
        let extra: Vec<&String> = b.flex_descs.iter().filter(|n| !a.flex_descs.contains(n)).collect();
        let missing: Vec<&String> = a.flex_descs.iter().filter(|n| !b.flex_descs.contains(n)).collect();
        diffs.push(format!("flexes {}/{} vs {}/{} (descs/controllers), missing {missing:?}, extra {extra:?}", a.flex_descs.len(), a.flex_controllers.len(), b.flex_descs.len(), b.flex_controllers.len()));
    }
    let mut prop_notes = Vec::new();
    if (a.mass - b.mass).abs() > 0.01 {
        // studiomdl writes $mass into both the header and the .phy; when the original's two
        // disagree we reproduce the .phy (what ragdolls/physics actually use).
        let phy_mass = |p: Option<&Phy>| p.and_then(|p| p.block("editparams")).and_then(|e| e.get_str("totalmass")).and_then(|s| s.parse::<f32>().ok());
        match (phy_mass(a_phy), phy_mass(b_phy)) {
            (Some(x), Some(y)) if (x - y).abs() < 0.01 => prop_notes.push(format!("header mass {} vs {} (the original header and .phy disagree; the .phy total mass {} is reproduced)", a.mass, b.mass, x)),
            _ if solid_masses_match(a_phy, b_phy) => prop_notes.push(format!(
                "header mass {} vs {}: the original's per-bone ragdoll masses were edited after compiling, so the decompiled QC reproduces every bone's mass exactly instead (the header value is not used for ragdoll physics)",
                a.mass, b.mass
            )),
            _ => diffs.push(format!("mass {} vs {}", a.mass, b.mass)),
        }
    }
    check(
        &mut out,
        "Model properties",
        diffs.is_empty(),
        if diffs.is_empty() {
            let base = "Eye/illum position, boxes, surfaceprop, contents, flags, pose params, IK chains, includes and flexes identical".to_string();
            if prop_notes.is_empty() { base } else { format!("{base}. Note: {}", prop_notes.join("; ")) }
        } else {
            summarize(&diffs)
        },
    );

    // ---- physics
    match (a_phy, b_phy) {
        (None, None) => {}
        (Some(_), None) => check(&mut out, "Physics", false, "Original has a .phy but the recompiled model does not"),
        (None, Some(_)) => check(&mut out, "Physics", false, "Recompiled model has a .phy but the original does not"),
        (Some(p), Some(q)) => {
            let mut diffs = Vec::new();
            if p.solids.len() != q.solids.len() {
                diffs.push(format!("{} vs {} solids", p.solids.len(), q.solids.len()));
            }
            // match solids by bone name (studiomdl may order ragdoll solids differently)
            let pi = p.solid_infos();
            let qi = q.solid_infos();
            let pairs: Vec<(usize, usize, String)> = (0..p.solids.len())
                .map(|i| {
                    let name = pi.iter().find(|x| x.0 == i).map(|x| x.1.clone()).unwrap_or_default();
                    let j = qi.iter().find(|x| x.1.eq_ignore_ascii_case(&name)).map(|x| x.0).unwrap_or(i);
                    (i, j, name)
                })
                .collect();
            for (i, j, name) in pairs {
                let (Some(s), Some(t)) = (p.solids.get(i), q.solids.get(j)) else { continue };
                let i = if name.is_empty() { i.to_string() } else { format!("{i} ({name})") };
                if s.convexes.len() != t.convexes.len() {
                    diffs.push(format!("solid {i}: {} vs {} convex pieces", s.convexes.len(), t.convexes.len()));
                }
                let bb = |s: &crate::formats::phy::Solid| {
                    let mut mn = Vec3::splat(f32::MAX);
                    let mut mx = Vec3::splat(f32::MIN);
                    for c in &s.convexes {
                        for tri in &c.tris {
                            for v in tri {
                                mn = mn.min(*v);
                                mx = mx.max(*v);
                            }
                        }
                    }
                    (mn, mx)
                };
                let (a1, a2) = bb(s);
                let (b1, b2) = bb(t);
                if !tol_vec(a1, b1, 0.5) || !tol_vec(a2, b2, 0.5) {
                    diffs.push(format!("solid {i}: bounds {a1:?}-{a2:?} vs {b1:?}-{b2:?}"));
                }
                let sm = |infos: &[(usize, String, &crate::formats::keyvalues::Kv)], k: usize| infos.iter().find(|x| x.0 == k).and_then(|x| x.2.get_str("mass")).and_then(|s| s.parse::<f32>().ok());
                if let (Some(ma), Some(mb)) = (sm(&pi, pairs_i(&i)), sm(&qi, j)) {
                    if (ma - mb).abs() > 0.51 {
                        diffs.push(format!("solid {i}: mass {ma} vs {mb}"));
                    }
                }
            }
            check(&mut out, "Physics", diffs.is_empty(), if diffs.is_empty() { format!("{} solid(s) match (pieces and bounds)", p.solids.len()) } else { summarize(&diffs) });
        }
    }
    out
}

/// Sorted list of LOD0 triangles (rounded positions + UVs, rotation-normalized) for one body model.
type TriKey = [i64; 15];
fn model_geometry(m: &Mdl, vtx: Option<&Vtx>, vvd: Option<&Vvd>, bi: usize, mi: usize) -> Option<Vec<TriKey>> {
    let (vtx, vvd) = (vtx?, vvd?);
    let model = m.bodyparts.get(bi)?.models.get(mi)?;
    let lod = vtx.bodyparts.get(bi)?.get(mi)?.lods.first()?;
    let mut out = Vec::new();
    for (me, mesh) in model.meshes.iter().enumerate() {
        let Some(vm) = lod.get(me) else { continue };
        let base = (model.first_vertex + mesh.vertex_offset) as usize;
        for t in &vm.tris {
            let mut vs: Vec<[i64; 5]> = Vec::with_capacity(3);
            for &i in t {
                let v = vvd.vertices.get(base + i as usize)?;
                vs.push([(v.pos.x * 100.0).round() as i64, (v.pos.y * 100.0).round() as i64, (v.pos.z * 100.0).round() as i64, (v.uv[0] * 1000.0).round() as i64, (v.uv[1] * 1000.0).round() as i64]);
            }
            let k = (0..3).min_by_key(|&k| vs[k]).unwrap_or(0);
            let mut key = [0i64; 15];
            for j in 0..3 {
                key[j * 5..j * 5 + 5].copy_from_slice(&vs[(k + j) % 3]);
            }
            out.push(key);
        }
    }
    out.sort();
    Some(out)
}

/// Number of triangles of `a` with no counterpart in `b`, allowing 0.02 units / 0.002 UV of rounding.
fn unmatched_tris(a: &[TriKey], b: &[TriKey]) -> usize {
    let mut ua: Vec<&TriKey> = a.iter().filter(|t| b.binary_search(t).is_err()).collect();
    let mut ub: Vec<&TriKey> = b.iter().filter(|t| a.binary_search(t).is_err()).collect();
    if ua.len() > 20_000 {
        return ua.len();
    }
    // same triangle if any rotation of its vertices (winding preserved) is within tolerance
    let close = |x: &TriKey, y: &TriKey| (0..3).any(|r| (0..15).all(|k| (x[k] - y[(k + r * 5) % 15]).abs() <= 2));
    ua.retain(|t| {
        if let Some(i) = ub.iter().position(|u| close(t, u)) {
            ub.swap_remove(i);
            false
        } else {
            true
        }
    });
    if std::env::var("GS_DEBUG_TRIS").is_ok() {
        for t in &ua {
            eprintln!("unmatched a: {t:?}");
        }
        for t in &ub {
            eprintln!("unmatched b: {t:?}");
        }
    }
    ua.len()
}

/// Every solid (matched by bone name) has the same mass in both .phy files.
fn solid_masses_match(a: Option<&Phy>, b: Option<&Phy>) -> bool {
    let (Some(a), Some(b)) = (a, b) else { return false };
    let ia = a.solid_infos();
    let ib = b.solid_infos();
    !ia.is_empty()
        && ia.iter().all(|(_, name, kv)| {
            let ma = kv.get_str("mass").and_then(|s| s.trim().parse::<f32>().ok());
            let mb = ib.iter().find(|x| x.1.eq_ignore_ascii_case(name)).and_then(|x| x.2.get_str("mass")).and_then(|s| s.trim().parse::<f32>().ok());
            matches!((ma, mb), (Some(x), Some(y)) if (x - y).abs() <= 0.51)
        })
}

fn pairs_i(label: &str) -> usize {
    label.split_whitespace().next().and_then(|s| s.parse().ok()).unwrap_or(0)
}

fn summarize(d: &[String]) -> String {
    let mut s = d.iter().take(8).cloned().collect::<Vec<_>>().join("; ");
    if d.len() > 8 {
        s.push_str(&format!("; ... and {} more", d.len() - 8));
    }
    s
}

/// Recompile a decompiled QC with studiomdl into a throw-away game folder and compare
/// the result with the original model.
pub fn roundtrip(
    input: &crate::decompile::DecompileInput,
    qc: &Path,
    studiomdl: &Path,
    fs: Option<&crate::gamefs::GameFs>,
    work: &Path,
    log: &mut dyn FnMut(String),
) -> Result<Vec<Check>, String> {
    use crate::compile::{self, CompileJob, CompileMsg};
    let orig = Mdl::parse(input.mdl.clone()).map_err(|e| e.to_string())?;
    let game = work.join("gs_verify_game");
    let _ = std::fs::remove_dir_all(&game);
    compile::make_temp_game(&game, fs).map_err(|e| format!("Could not create the verification game folder: {e}"))?;
    let job = CompileJob { studiomdl: studiomdl.to_path_buf(), game_dir: game.clone(), qc: qc.to_path_buf(), extra_args: vec!["-nox360".into()] };
    let (tx, rx) = std::sync::mpsc::channel();
    let j2 = job.clone();
    let handle = std::thread::spawn(move || compile::run(&j2, &tx));
    let mut lines = Vec::new();
    let mut success = false;
    for m in rx {
        match m {
            CompileMsg::Line(l) => {
                if l.contains("ERROR") || l.contains("WARNING") {
                    log(format!("studiomdl: {l}"));
                }
                lines.push(l);
            }
            CompileMsg::Finished { success: s, .. } => success = s,
        }
    }
    let _ = handle.join();
    if !success {
        let issues = compile::analyze_output(&lines, qc);
        let msg = issues.first().map(|i| i.message.clone()).unwrap_or_else(|| "studiomdl failed".into());
        return Err(format!("Recompiling the decompiled QC failed: {msg}"));
    }
    let model_rel = {
        let n = orig.name.replace('\\', "/");
        if n.to_ascii_lowercase().starts_with("models/") { n } else { format!("models/{n}") }
    };
    let new_path = game.join(&model_rel);
    let new = Mdl::parse(std::fs::read(&new_path).map_err(|e| format!("Recompiled model not found at {}: {e}", new_path.display()))?).map_err(|e| e.to_string())?;
    let base = new_path.with_extension("");
    let read = |ext: &str| std::fs::read(format!("{}{ext}", base.display())).ok();
    let vtx_a = input.vtx.as_deref().and_then(|d| Vtx::parse(d).ok());
    let vtx_b = read(".dx90.vtx").and_then(|d| Vtx::parse(&d).ok());
    let vvd_a = input.vvd.as_deref().and_then(|d| Vvd::parse(d).ok());
    let vvd_b = read(".vvd").and_then(|d| Vvd::parse(&d).ok());
    let phy_a = input.phy.as_deref().and_then(|d| Phy::parse(d).ok());
    let phy_b = read(".phy").and_then(|d| Phy::parse(&d).ok());
    let ani_b = read(".ani");
    Ok(compare_models(&orig, input.ani.as_deref(), &new, ani_b.as_deref(), vtx_a.as_ref(), vtx_b.as_ref(), vvd_a.as_ref(), vvd_b.as_ref(), phy_a.as_ref(), phy_b.as_ref()))
}

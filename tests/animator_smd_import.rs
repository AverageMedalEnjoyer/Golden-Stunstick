//! Animator "Import SMD": exact round trip on the same skeleton, missing / extra /
//! reparented bones, and an SMD from a differently proportioned skeleton.
//! Uses the Male_07 (john) QC and alyx's SMDs from the FC TF SDK asset sources; skipped
//! when they are not there.

use glam::{Quat, Vec3};
use golden_stunstick::anim::{self, AnimDoc, Ease, Keyframe, Rig};
use golden_stunstick::formats::smd::Smd;
use std::path::PathBuf;

const DIR: &str = r"D:\GITREPOS\fc_tfsdk_assetsrc\content (ingame)\fc_tfsdk\models\player\john\John_XSI";

fn rig() -> Option<Rig> {
    let qc = PathBuf::from(DIR).join("Male_07.qc");
    if !qc.is_file() {
        println!("skipped: {} not found", qc.display());
        return None;
    }
    Some(Rig::load(&qc).unwrap())
}

fn ang(a: Quat, b: Quat) -> f32 {
    a.normalize().angle_between(b.normalize()).to_degrees()
}

/// A 20-frame test animation on the rig: a few bones swing, the root moves.
fn test_doc(rig: &Rig) -> AnimDoc {
    let mut doc = AnimDoc::new(rig);
    doc.frames = 20;
    let bones: Vec<usize> = ["ValveBiped.Bip01_L_UpperArm", "ValveBiped.Bip01_R_Thigh", "ValveBiped.Bip01_Spine2", "ValveBiped.Bip01_Head1"].iter().filter_map(|n| rig.bone_index(n)).collect();
    assert_eq!(bones.len(), 4);
    for (k, f) in [0u32, 10, 19].iter().enumerate() {
        let mut pose = rig.bind.clone();
        for (j, &b) in bones.iter().enumerate() {
            pose[b].1 = (Quat::from_rotation_z(0.3 * (k as f32 + 1.0) * if j % 2 == 0 { 1.0 } else { -1.0 }) * pose[b].1).normalize();
        }
        let root = rig.bones.iter().position(|b| b.parent < 0).unwrap();
        pose[root].0 += Vec3::new(k as f32 * 5.0, 0.0, 0.0);
        doc.keys.insert(*f, Keyframe { pose, ease: Ease::Smooth });
    }
    doc
}

#[test]
fn smd_round_trip_is_exact() {
    let Some(rig) = rig() else { return };
    let doc = test_doc(&rig);
    let smd = Smd::parse(&doc.export_smd(&rig)).unwrap();
    let m = anim::match_smd(&smd, &rig);
    assert!(m.diffs.is_empty(), "same skeleton should match: {:?}", m.diffs);
    let imported = anim::import_smd(&smd, &rig, &m, 30.0, false);
    assert_eq!(imported.frames, 20);
    let mut worst = (0.0f32, 0.0f32);
    for f in 0..20 {
        let a = rig.world(&doc.sample(f as f32, &rig.bind));
        let b = rig.world(&imported.sample(f as f32, &rig.bind));
        for i in 0..a.len() {
            worst.0 = worst.0.max((Vec3::from(a[i].translation) - Vec3::from(b[i].translation)).length());
            let (_, qa, _) = a[i].to_scale_rotation_translation();
            let (_, qb, _) = b[i].to_scale_rotation_translation();
            worst.1 = worst.1.max(ang(qa, qb));
        }
    }
    println!("round trip: worst position error {:.5}, rotation error {:.4} deg", worst.0, worst.1);
    // SMD stores Euler angles: f32 conversion near +-90 degree pitch costs a few hundredths of a degree
    assert!(worst.0 < 0.01 && worst.1 < 0.1);
    // half frames interpolate between the per-frame keys
    let mid = imported.sample(9.5, &rig.bind);
    assert!(mid.iter().all(|(p, q)| p.is_finite() && q.is_finite()));
}

#[test]
fn smd_with_missing_extra_and_reparented_bones() {
    let Some(rig) = rig() else { return };
    let doc = test_doc(&rig);
    let mut smd = Smd::parse(&doc.export_smd(&rig)).unwrap();
    let hand = smd.nodes.iter().position(|n| n.name == "ValveBiped.Bip01_R_Hand").unwrap();
    let spine = smd.nodes.iter().position(|n| n.name == "ValveBiped.Bip01_Spine2").unwrap();
    let pelvis = smd.nodes.iter().position(|n| n.name == "ValveBiped.Bip01_Pelvis").unwrap();
    // 1) reparent the right hand under Spine2, keeping its world transform in every frame
    for fr in smd.frames.iter_mut() {
        let local: Vec<(Vec3, Quat)> = fr.iter().map(|(p, r)| (*p, golden_stunstick::formats::mdl::euler_to_quat(*r))).collect();
        let mut world: Vec<glam::Affine3A> = Vec::new();
        for (i, n) in smd.nodes.iter().enumerate() {
            let l = glam::Affine3A::from_rotation_translation(local[i].1, local[i].0);
            world.push(if n.parent >= 0 { world[n.parent as usize] * l } else { l });
        }
        let (_, q, p) = (world[spine].inverse() * world[hand]).to_scale_rotation_translation();
        fr[hand] = (p, golden_stunstick::formats::mdl::quat_to_euler(q));
    }
    smd.nodes[hand].parent = spine as i32;
    // 2) an extra bone the model doesn't have
    smd.nodes.push(golden_stunstick::formats::smd::SmdNode { name: "prop_gun".into(), parent: pelvis as i32 });
    for fr in smd.frames.iter_mut() {
        fr.push((Vec3::new(1.0, 2.0, 3.0), Vec3::ZERO));
    }
    // 3) a missing bone: rename the head so the model's head is not found
    let head = smd.nodes.iter().position(|n| n.name == "ValveBiped.Bip01_Head1").unwrap();
    smd.nodes[head].name = "head_renamed".into();
    let m = anim::match_smd(&smd, &rig);
    for d in &m.diffs {
        println!("diff: {d}");
    }
    assert!(m.diffs.iter().any(|d| d.contains("not in the SMD") && d.contains("Head1")));
    assert!(m.diffs.iter().any(|d| d.contains("not in the model") && d.contains("prop_gun")));
    assert!(m.diffs.iter().any(|d| d.contains("R_Hand") && d.contains("Spine2")));
    assert!(!m.other_proportions);
    let imported = anim::import_smd(&smd, &rig, &m, 30.0, false);
    let rh = rig.bone_index("ValveBiped.Bip01_R_Hand").unwrap();
    let rhead = rig.bone_index("ValveBiped.Bip01_Head1").unwrap();
    for f in [0u32, 7, 19] {
        let a = rig.world(&doc.sample(f as f32, &rig.bind));
        let b = rig.world(&imported.sample(f as f32, &rig.bind));
        let err = (Vec3::from(a[rh].translation) - Vec3::from(b[rh].translation)).length();
        assert!(err < 0.01, "reparented hand should stay where the SMD puts it (frame {f}: {err})");
        assert!(ang(imported.sample(f as f32, &rig.bind)[rhead].1, rig.bind[rhead].1) < 1e-3, "missing head keeps the reference pose");
    }
}

#[test]
fn smd_from_other_proportions_keeps_model_bone_lengths() {
    let Some(rig) = rig() else { return };
    let path = PathBuf::from(DIR).join(r"alyx\alyx_anims\ragdoll.smd");
    let Ok(text) = std::fs::read_to_string(&path) else {
        println!("skipped: {} not found", path.display());
        return;
    };
    let smd = Smd::parse(&text).unwrap();
    let m = anim::match_smd(&smd, &rig);
    for d in &m.diffs {
        println!("diff: {d}");
    }
    let doc = anim::import_smd(&smd, &rig, &m, 30.0, false);
    let pose = doc.sample(0.0, &rig.bind);
    let world = rig.world(&pose);
    let bind_world = rig.world(&rig.bind);
    let mut worst_len = 0.0f32;
    let mut rot_err = 0.0f32;
    for (r, b) in rig.bones.iter().enumerate() {
        if b.parent < 0 {
            continue;
        }
        let p = b.parent as usize;
        let l = (Vec3::from(world[r].translation) - Vec3::from(world[p].translation)).length();
        let lb = (Vec3::from(bind_world[r].translation) - Vec3::from(bind_world[p].translation)).length();
        if m.other_proportions || m.map[r].is_none() {
            worst_len = worst_len.max((l - lb).abs());
        }
        if let (Some(i), false) = (m.map[r], m.reparented[r]) {
            let q = golden_stunstick::formats::mdl::euler_to_quat(smd.frames[0][i].1);
            rot_err = rot_err.max(ang(q, pose[r].1));
        }
    }
    println!("other proportions: {}, root scale {:.3}; worst bone length change {worst_len:.4}, worst rotation error {rot_err:.4} deg", m.other_proportions, m.root_scale);
    assert!(worst_len < 0.01, "model bone lengths must be kept");
    assert!(rot_err < 0.05, "matched bones take the SMD's rotations");
}

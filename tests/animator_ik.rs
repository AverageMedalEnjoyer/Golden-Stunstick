//! Easy-rig IK on a real rig (the FC SDK john QC). Skipped when the source files are absent.

use glam::{Quat, Vec3};
use golden_stunstick::anim::{detect_chains, solve_ik, Rig};
use std::path::PathBuf;

fn pos(rig: &Rig, pose: &[(Vec3, Quat)], i: usize) -> Vec3 {
    rig.world(pose)[i].translation.into()
}

fn rot(rig: &Rig, pose: &[(Vec3, Quat)], i: usize) -> Quat {
    rig.world(pose)[i].to_scale_rotation_translation().1.normalize()
}

#[test]
fn easy_rig_two_bone_ik() {
    let qc = PathBuf::from(r"D:\GITREPOS\fc_tfsdk_assetsrc\content (ingame)\fc_tfsdk\models\player\john\John_XSI\Male_07.qc");
    if !qc.is_file() {
        println!("skipped: {} not found", qc.display());
        return;
    }
    let rig = Rig::load(&qc).unwrap();
    let chains = detect_chains(&rig);
    let names: Vec<&str> = chains.iter().map(|c| c.name.as_str()).collect();
    println!("chains: {names:?}");
    assert_eq!(chains.len(), 4, "two arms + two legs (no wrist helper duplicates)");

    // character forward = foot -> toe
    let foot = rig.bone_index("ValveBiped.Bip01_L_Foot").unwrap();
    let toe = rig.children(foot)[0];
    let fw = {
        let v = pos(&rig, &rig.bind, toe) - pos(&rig, &rig.bind, foot);
        Vec3::new(v.x, v.y, 0.0).normalize()
    };

    for c in &chains {
        let n = c.bones.len();
        let (ia, ib, ic) = (c.bones[n - 3], c.bones[n - 2], c.bones[n - 1]);
        let is_leg = c.name.contains("leg");
        let start = pos(&rig, &rig.bind, ic);
        let eff_rot = rot(&rig, &rig.bind, ic);
        // straight up (the case that used to need a detour), forward, sideways
        for (label, offset) in [("up", Vec3::Z * 12.0), ("forward", fw * 10.0), ("side", fw.cross(Vec3::Z) * 8.0), ("up+fwd", Vec3::Z * 8.0 + fw * 8.0)] {
            let mut pose = rig.bind.clone();
            let target = start + offset;
            // drag there in small steps, like the viewport does
            for k in 1..=20 {
                let cur = pos(&rig, &pose, ic);
                let step = start + offset * (k as f32 / 20.0);
                solve_ik(&rig, &mut pose, &c.bones, cur + (step - cur));
            }
            let got = pos(&rig, &pose, ic);
            let (a, b) = (pos(&rig, &pose, ia), pos(&rig, &pose, ib));
            let d = (got - a).normalize();
            let bend = (b - a) - d * (b - a).dot(d);
            let toward = bend.normalize_or_zero().dot(fw);
            let rot_err = rot(&rig, &pose, ic).normalize().angle_between(eff_rot.normalize()).to_degrees();
            println!("{:<40} {label:<8} miss {:.4}  bend·forward {:+.2}  hand rot change {:.3}°", c.name, (got - target).length(), toward, rot_err);
            let reach = (pos(&rig, &rig.bind, ib) - pos(&rig, &rig.bind, ia)).length() + (pos(&rig, &rig.bind, ic) - pos(&rig, &rig.bind, ib)).length();
            if (target - a).length() < reach * 0.999 {
                assert!((got - target).length() < 0.02, "{} {label}: effector reaches the target", c.name);
            } else {
                // out of reach: fully stretched toward it
                assert!((got - a).normalize().dot((target - a).normalize()) > 0.9999, "{} {label}: stretches toward the target", c.name);
                assert!(((got - a).length() - reach).abs() < 0.05);
                continue;
            }
            assert!(rot_err < 0.2, "effector keeps its world rotation");
            if is_leg {
                assert!(toward > 0.3, "{} {label}: knee points forward", c.name);
            } else {
                assert!(toward < 0.0, "{} {label}: elbow points back", c.name);
            }
            // bone lengths are untouched (rotation only)
            let lb = (pos(&rig, &rig.bind, ib) - pos(&rig, &rig.bind, ia)).length();
            assert!(((b - a).length() - lb).abs() < 1e-3);
        }
        // no drift: wiggle away and come back -> same pose as solving straight to the start
        let mut direct = rig.bind.clone();
        solve_ik(&rig, &mut direct, &c.bones, start);
        let mut wiggle = rig.bind.clone();
        for k in 0..60 {
            let t = k as f32 * 0.3;
            solve_ik(&rig, &mut wiggle, &c.bones, start + Vec3::new(t.sin() * 6.0, t.cos() * 4.0, (t * 0.7).sin() * 7.0));
        }
        solve_ik(&rig, &mut wiggle, &c.bones, start);
        for &j in &c.bones {
            let e = direct[j].1.normalize().angle_between(wiggle[j].1.normalize()).to_degrees();
            assert!(e < 0.05, "{}: bone {} drifted {e}°", c.name, rig.bones[j].name);
        }
    }
}

//! Model Viewer pose parameters and auto-layers on the FC SDK john_freeman model
//! (pose parameters and aim layers come from the $includemodel hl2mp_anims).
//! Skipped when the mod is not installed.

use glam::Quat;
use golden_stunstick::gamefs::GameFs;
use golden_stunstick::scene::{ModelOrigin, ModelScene};
use std::path::PathBuf;

#[test]
fn john_pose_params_drive_blends_and_aim_layers() {
    let gameinfo = PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Source SDK Base 2013 Multiplayer\FCSDK\gameinfo.txt");
    if !gameinfo.is_file() {
        println!("skipped: {} not found", gameinfo.display());
        return;
    }
    let fs = GameFs::mount(&gameinfo, None).unwrap();
    if fs.find("models/player/john/john_freeman.mdl").is_none() {
        println!("skipped: john_freeman.mdl not in the mounted game");
        return;
    }
    let scene = ModelScene::load(Some(&fs), ModelOrigin::Game("models/player/john/john_freeman.mdl")).unwrap();
    let names: Vec<&str> = scene.pose_params.iter().map(|p| p.name.as_str()).collect();
    println!("pose parameters: {names:?}");
    for want in ["body_yaw", "body_pitch", "move_x", "move_y"] {
        assert!(names.iter().any(|n| n.eq_ignore_ascii_case(want)), "{want} should be listed");
    }
    let idx = |n: &str| names.iter().position(|x| x.eq_ignore_ascii_case(n)).unwrap();
    let seq = scene.seqs.iter().position(|s| s.name.eq_ignore_ascii_case("run_ar2")).expect("run_ar2");
    let used: Vec<&str> = scene.sequence_pose_params(seq).iter().map(|&i| names[i]).collect();
    println!("run_ar2 uses: {used:?}");
    let base: Vec<f32> = scene.pose_params.iter().map(|p| if p.start <= 0.0 && p.end >= 0.0 { 0.0 } else { (p.start + p.end) * 0.5 }).collect();
    let pose_with = |name: &str, v: f32| {
        let mut pp = base.clone();
        pp[idx(name)] = v;
        scene.sequence_pose(seq, 5.0, &pp)
    };
    let bone = |n: &str| scene.bones().iter().position(|b| b.name.eq_ignore_ascii_case(n)).unwrap();
    let ang = |a: Quat, b: Quat| a.normalize().angle_between(b.normalize()).to_degrees();
    let reference = scene.sequence_pose(seq, 5.0, &base);
    // body_yaw / body_pitch turn the upper body through the aim layer
    let spine = bone("ValveBiped.Bip01_Spine2");
    let yaw = pose_with("body_yaw", 45.0);
    let pitch = pose_with("body_pitch", 45.0);
    let yaw_change = (0..reference.len()).map(|i| ang(reference[i].1, yaw[i].1)).fold(0.0, f32::max);
    println!("body_yaw 45: spine2 turns {:.1} deg, largest bone change {:.1} deg", ang(reference[spine].1, yaw[spine].1), yaw_change);
    println!("body_pitch 45: spine2 turns {:.1} deg", ang(reference[spine].1, pitch[spine].1));
    assert!(yaw_change > 5.0, "body_yaw should visibly change the pose");
    // move_x / move_y pick the run direction blend
    let thigh = bone("ValveBiped.Bip01_L_Thigh");
    let mx = pose_with("move_x", -1.0);
    let my = pose_with("move_y", 1.0);
    println!("move_x -1: thigh {:.1} deg, move_y 1: thigh {:.1} deg", ang(reference[thigh].1, mx[thigh].1), ang(reference[thigh].1, my[thigh].1));
    assert!(ang(reference[thigh].1, mx[thigh].1) > 2.0 || ang(reference[thigh].1, my[thigh].1) > 2.0, "move_x/move_y should change the legs");
}

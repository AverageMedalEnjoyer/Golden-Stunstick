//! Export an animation from the Animator core (SMD and DMX) and make sure
//! studiomdl compiles both. Needs TF2 + a decompiled heavy from tests/roundtrip.rs.

use golden_stunstick::anim::{AnimDoc, Ease, Keyframe, Rig, detect_chains, solve_ik};
use golden_stunstick::compile::{self, CompileJob, CompileMsg};
use golden_stunstick::formats::mdl::Mdl;
use glam::{Quat, Vec3};
use std::path::PathBuf;

#[test]
#[ignore = "needs TF2 and the roundtrip test output"]
fn exported_animations_compile() {
    let tf = PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2");
    let dir = std::env::temp_dir().join("gs_roundtrip").join("models_player_heavy.mdl").join("decompiled");
    let qc = dir.join("heavy.qc");
    if !qc.is_file() || !tf.join("bin/studiomdl.exe").is_file() {
        return;
    }
    let rig = Rig::load(&qc).expect("rig");
    let mut doc = AnimDoc::new(&rig);
    doc.frames = 20;
    let chains = detect_chains(&rig);
    assert!(chains.len() >= 4, "expected arm and leg chains, got {}", chains.len());
    let mut pose = rig.bind.clone();
    let hand = *chains[0].bones.last().unwrap();
    let target = Vec3::from(rig.world(&pose)[hand].translation) + Vec3::new(5.0, 5.0, 5.0);
    solve_ik(&rig, &mut pose, &chains[0].bones, target);
    if let Some(head) = rig.bone_index("bip_head") {
        pose[head].1 = (pose[head].1 * Quat::from_rotation_z(0.6)).normalize();
    }
    doc.keys.insert(19, Keyframe { pose, ease: Ease::Smooth });
    std::fs::write(dir.join("gs_test_anim.smd"), doc.export_smd(&rig)).unwrap();
    std::fs::write(dir.join("gs_test_anim.dmx"), doc.export_dmx(&rig)).unwrap();
    let body = rig.meshes.first().map(|m| m.file.file_name().unwrap().to_string_lossy().to_string()).unwrap();
    for ext in ["smd", "dmx"] {
        let test_qc = dir.join(format!("gs_anim_test_{ext}.qc"));
        std::fs::write(&test_qc, format!("$modelname \"gstest/anim_{ext}.mdl\"\n$body body \"{body}\"\n$cdmaterials \"models/player/hvyweapon/\"\n$sequence \"idle\" \"gs_test_anim.{ext}\" fps 30\n")).unwrap();
        let game = std::env::temp_dir().join("gs_anim_export_game");
        compile::make_temp_game(&game, None).unwrap();
        let job = CompileJob { studiomdl: tf.join("bin/studiomdl.exe"), game_dir: game.clone(), qc: test_qc.clone(), extra_args: vec![] };
        let (tx, rx) = std::sync::mpsc::channel();
        compile::run(&job, &tx);
        drop(tx);
        let mut ok = false;
        let mut lines = vec![];
        for m in rx {
            match m {
                CompileMsg::Line(l) => lines.push(l),
                CompileMsg::Finished { success, .. } => ok = success,
            }
        }
        if !ok {
            for l in lines.iter().rev().take(15) {
                println!("{l}");
            }
        }
        assert!(ok, "{ext} export failed to compile");
        let mdl = Mdl::parse(std::fs::read(game.join(format!("models/gstest/anim_{ext}.mdl"))).unwrap()).unwrap();
        assert_eq!(mdl.anims[0].numframes, 20, "{ext}: frame count");
        // last frame of the compiled animation should show the edited head rotation
        let head = mdl.bone_index("bip_head").unwrap();
        let f0 = mdl.decode_frame(0, 0, None);
        let f19 = mdl.decode_frame(0, 19, None);
        let turned = f0[head].1.angle_between(f19[head].1);
        println!("{ext}: head turned {:.3} rad between frame 0 and 19", turned);
        assert!((turned - 0.6).abs() < 0.05, "{ext}: head rotation not preserved ({turned})");
    }
}

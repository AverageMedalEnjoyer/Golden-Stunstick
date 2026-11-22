//! Decompile -> studiomdl -> compare round trip against real TF2 models.
//! Skipped when TF2 (and its studiomdl) is not installed.

use golden_stunstick::compile::{self, CompileJob, CompileMsg};
use golden_stunstick::decompile::{DecompileInput, DecompileOptions, decompile};
use golden_stunstick::formats::mdl::{Mdl, Vtx};
use golden_stunstick::formats::phy::Phy;
use golden_stunstick::gamefs::GameFs;
use golden_stunstick::verify;
use std::path::PathBuf;

fn tf2_dir() -> Option<PathBuf> {
    let p = std::env::var("GS_TF2_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2"));
    (p.join("tf").join("gameinfo.txt").is_file() && p.join("bin").join("studiomdl.exe").is_file()).then_some(p)
}

fn roundtrip(model: &str) -> bool {
    let Some(tf) = tf2_dir() else { return true };
    let fs = GameFs::mount(&tf.join("tf").join("gameinfo.txt"), None).unwrap();
    let input = DecompileInput::from_gamefs(&fs, model).unwrap();
    let work = std::env::temp_dir().join("gs_roundtrip").join(model.replace('/', "_"));
    let _ = std::fs::remove_dir_all(&work);
    let out = work.join("decompiled");
    let mut log = |s: String| println!("  {s}");
    let report = decompile(&input, &out, &DecompileOptions::default(), &mut log).expect("decompile");
    let orig = Mdl::parse(input.mdl.clone()).unwrap();
    for c in verify::self_check(&orig, input.ani.as_deref(), input.vvd.as_deref(), input.vtx.as_deref(), &report, &out) {
        println!("  [self {}] {}: {}", if c.ok { "OK" } else { "FAIL" }, c.category, c.detail);
    }
    let game = work.join("game");
    compile::make_temp_game(&game, Some(&fs)).unwrap();
    let job = CompileJob { studiomdl: tf.join("bin").join("studiomdl.exe"), game_dir: game.clone(), qc: report.qc_path.clone().unwrap(), extra_args: vec!["-nox360".into()] };
    let (tx, rx) = std::sync::mpsc::channel();
    compile::run(&job, &tx);
    drop(tx);
    let mut lines = Vec::new();
    let mut success = false;
    for m in rx {
        match m {
            CompileMsg::Line(l) => lines.push(l),
            CompileMsg::Finished { success: s, .. } => success = s,
        }
    }
    if !success {
        for l in lines.iter().rev().take(25).collect::<Vec<_>>().into_iter().rev() {
            println!("  | {l}");
        }
        for i in compile::analyze_output(&lines, &job.qc) {
            println!("  issue: {:?} line {} {} -> {}", i.severity, i.line, i.message, i.suggestion);
        }
        return false;
    }
    let new_path = game.join(model);
    let new = Mdl::parse(std::fs::read(&new_path).expect("compiled mdl")).unwrap();
    let base = model.trim_end_matches(".mdl");
    let vtx_a = input.vtx.as_deref().and_then(|d| Vtx::parse(d).ok());
    let vtx_b = std::fs::read(game.join(format!("{base}.dx90.vtx"))).ok().and_then(|d| Vtx::parse(&d).ok());
    let phy_a = input.phy.as_deref().and_then(|d| Phy::parse(d).ok());
    let phy_b = std::fs::read(game.join(format!("{base}.phy"))).ok().and_then(|d| Phy::parse(&d).ok());
    let ani_b = std::fs::read(game.join(format!("{base}.ani"))).ok();
    let vvd_a = input.vvd.as_deref().and_then(|d| golden_stunstick::formats::mdl::Vvd::parse(d).ok());
    let vvd_b = std::fs::read(game.join(format!("{base}.vvd"))).ok().and_then(|d| golden_stunstick::formats::mdl::Vvd::parse(&d).ok());
    let checks = verify::compare_models(&orig, input.ani.as_deref(), &new, ani_b.as_deref(), vtx_a.as_ref(), vtx_b.as_ref(), vvd_a.as_ref(), vvd_b.as_ref(), phy_a.as_ref(), phy_b.as_ref());
    let mut all = true;
    for c in &checks {
        println!("  [{}] {}: {}", if c.ok { "OK" } else { "FAIL" }, c.category, c.detail);
        all &= c.ok;
    }
    all
}

#[test]
#[ignore = "slow: runs studiomdl; use cargo test -- --ignored"]
fn roundtrip_models() {
    let models = std::env::var("GS_RT_MODELS").unwrap_or_else(|_| "models/props_2fort/oildrum.mdl,models/weapons/w_models/w_shotgun.mdl,models/weapons/c_models/c_shotgun/c_shotgun.mdl,models/player/heavy.mdl".into());
    let mut ok = true;
    for m in models.split(',') {
        println!("=== {m}");
        ok &= roundtrip(m.trim());
    }
    assert!(ok);
}

//! Model Viewer event sounds against real game content: every sound event of a few models
//! resolves through the soundscripts and its wave decodes. Skipped when a game is missing.
//! (Nothing is played; only lookup and decoding are tested.)

use golden_stunstick::gamefs::GameFs;
use golden_stunstick::scene::{ModelOrigin, ModelScene};
use golden_stunstick::sound::{self, Resolved, SoundScripts};
use std::path::PathBuf;

fn check(gameinfo: &str, models: &[&str], must: &[&str]) {
    let gi = PathBuf::from(gameinfo);
    if !gi.is_file() {
        println!("skipped: {gameinfo} not found");
        return;
    }
    let fs = GameFs::mount(&gi, None).unwrap();
    let t = std::time::Instant::now();
    let scripts = SoundScripts::load(&fs);
    println!("{} soundscript files, {} sounds in {:?}", scripts.files, scripts.entries.len(), t.elapsed());
    for m in must {
        let e = scripts.get(m).unwrap_or_else(|| panic!("{m} should be a gamesound"));
        let w = &e.waves[0];
        let bytes = fs.read(w).unwrap_or_else(|| panic!("{w} missing"));
        let pcm = sound::decode(w, &bytes).unwrap_or_else(|e| panic!("{w}: {e}"));
        println!("  {m} -> {w}: {} ch, {} Hz, {} samples", pcm.channels, pcm.rate, pcm.samples.len());
        assert!(pcm.samples.len() > 100);
        assert!(pcm.samples.iter().any(|s| s.abs() > 0.01), "{w} decoded to silence");
    }
    for m in models {
        let Ok(scene) = ModelScene::load(Some(&fs), ModelOrigin::Game(m)) else { continue };
        let (mut events, mut found, mut decoded) = (0, 0, 0);
        let mut missing = Vec::new();
        for src in &scene.sources {
            for sq in &src.mdl.seqs {
                for e in &sq.events {
                    let names = sound::event_sounds(e.event, &e.name, &e.options, sq.name.to_ascii_lowercase().contains("run"));
                    if names.is_empty() {
                        continue;
                    }
                    events += 1;
                    let Some(r) = names.iter().find_map(|n| sound::resolve(n, Some(&scripts), &fs)) else {
                        missing.push(names[0].clone());
                        continue;
                    };
                    found += 1;
                    let wave = match r {
                        Resolved::Script(s) => s.waves[0].clone(),
                        Resolved::Wave(w) => w,
                    };
                    let wave = sound::expand_gender(&wave, m);
                    if fs.read(&wave).map(|b| sound::decode(&wave, &b).is_ok()).unwrap_or(false) {
                        decoded += 1;
                    } else {
                        missing.push(format!("{wave} (decode)"));
                    }
                }
            }
        }
        missing.sort();
        missing.dedup();
        println!("{m}: {events} sound events, {found} resolved, {decoded} decoded; unresolved: {missing:?}");
        if events > 0 {
            assert!(found * 10 >= events * 8, "{m}: most sound events should resolve");
            assert!(decoded * 10 >= found * 9, "{m}: resolved waves should decode");
        }
    }
}

/// Opens the default audio device and plays a silent buffer (inaudible): checks the output
/// path works without panicking. Reports, rather than fails, when there is no device.
#[test]
fn audio_device_opens() {
    let mut p = sound::Player::default();
    let pcm = sound::Pcm { channels: 1, rate: 22050, samples: vec![0.0; 2205] };
    p.play_pcm(&pcm, 0.0, 1.0);
    std::thread::sleep(std::time::Duration::from_millis(200));
    match p.error() {
        Some(e) => println!("no audio device: {e}"),
        None => println!("audio device opened and accepted a buffer"),
    }
}

#[test]
fn hl2_npc_event_sounds() {
    check(
        r"C:\Program Files (x86)\Steam\steamapps\common\Source SDK Base 2013 Multiplayer\FCSDK\gameinfo.txt",
        &["models/combine_soldier.mdl", "models/humans/group01/male_07.mdl", "models/zombie/classic.mdl", "models/antlion.mdl"],
        &["NPC_Citizen.RunFootstepLeft", "Zombie.FootstepLeft"],
    );
}

#[test]
fn tf2_event_sounds() {
    check(
        r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf\gameinfo.txt",
        &["models/player/heavy.mdl", "models/player/scout.mdl", "models/weapons/c_models/c_scout_arms.mdl"],
        &["Concrete.StepLeft", "Concrete.StepRight"],
    );
}

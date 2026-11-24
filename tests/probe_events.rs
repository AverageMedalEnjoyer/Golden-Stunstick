//! Probe: list the animation events of a few real models (ignored; for development).

use golden_stunstick::gamefs::GameFs;
use golden_stunstick::scene::{ModelOrigin, ModelScene};
use std::collections::BTreeMap;
use std::path::PathBuf;

#[test]
#[ignore]
fn probe_events() {
    let games = [
        (r"C:\Program Files (x86)\Steam\steamapps\common\Source SDK Base 2013 Multiplayer\FCSDK\gameinfo.txt", vec!["models/player/john/john_freeman.mdl", "models/combine_soldier.mdl", "models/humans/group01/male_07.mdl", "models/zombie/classic.mdl", "models/antlion.mdl"]),
        (r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2\tf\gameinfo.txt", vec!["models/player/heavy.mdl", "models/player/scout.mdl", "models/bots/heavy/bot_heavy.mdl", "models/weapons/c_models/c_scout_arms.mdl"]),
    ];
    for (gi, models) in games {
        let gi = PathBuf::from(gi);
        if !gi.is_file() {
            println!("skip {}", gi.display());
            continue;
        }
        let fs = GameFs::mount(&gi, None).unwrap();
        println!("manifest: {}", fs.exists("scripts/game_sounds_manifest.txt"));
        for m in models {
            let Ok(scene) = ModelScene::load(Some(&fs), ModelOrigin::Game(m)) else {
                println!("{m}: failed");
                continue;
            };
            let mut counts: BTreeMap<String, (usize, String)> = BTreeMap::new();
            for src in &scene.sources {
                for sq in &src.mdl.seqs {
                    for e in &sq.events {
                        let k = format!("{} / {} / type {}", e.event, e.name, e.kind);
                        let c = counts.entry(k).or_insert((0, String::new()));
                        c.0 += 1;
                        if c.1.is_empty() {
                            c.1 = format!("{} @ {}", e.options, sq.name);
                        }
                    }
                }
            }
            println!("== {m} ({} sources)", scene.sources.len());
            for (k, (n, ex)) in counts {
                println!("   {n:4}x {k}   e.g. '{ex}'");
            }
        }
    }
}

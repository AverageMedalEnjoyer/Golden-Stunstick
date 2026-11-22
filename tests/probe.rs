//! Probe studiomdl's coordinate handling with a synthetic model.
use golden_stunstick::compile::{self, CompileJob, CompileMsg};
use golden_stunstick::formats::mdl::{Mdl, quat_to_euler};
use golden_stunstick::formats::phy::Phy;
use std::path::PathBuf;

fn box_tris(min: [f32; 3], max: [f32; 3], mat: &str) -> String {
    let c = |i: usize| [if i & 1 != 0 { max[0] } else { min[0] }, if i & 2 != 0 { max[1] } else { min[1] }, if i & 4 != 0 { max[2] } else { min[2] }];
    let faces = [[0, 2, 3, 1], [4, 5, 7, 6], [0, 1, 5, 4], [2, 6, 7, 3], [0, 4, 6, 2], [1, 3, 7, 5]];
    let mut s = String::new();
    for f in faces {
        for t in [[f[0], f[1], f[2]], [f[0], f[2], f[3]]] {
            s.push_str(mat);
            s.push('\n');
            for v in t {
                let p = c(v);
                s.push_str(&format!("0 {} {} {} 0 0 1 0 0 1 0 1\n", p[0], p[1], p[2]));
            }
        }
    }
    s
}

#[test]
#[ignore]
fn probe() {
    let tf = PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2");
    let dir = std::env::temp_dir().join("gs_probe");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let rot = std::env::var("GS_PROBE_ROT").unwrap_or("0 0 0".into());
    let skel = format!("version 1\nnodes\n0 \"root\" -1\nend\nskeleton\ntime 0\n0 0 0 0 {rot}\nend\n");
    std::fs::write(dir.join("ref.smd"), format!("{skel}triangles\n{}end\n", box_tris([0.0, 0.0, 0.0], [10.0, 20.0, 30.0], "mat"))).unwrap();
    std::fs::write(dir.join("phys.smd"), format!("{skel}triangles\n{}end\n", box_tris([0.0, 0.0, 0.0], [10.0, 20.0, 30.0], "phy"))).unwrap();
    std::fs::write(dir.join("anim.smd"), "version 1\nnodes\n0 \"root\" -1\nend\nskeleton\ntime 0\n0 1 2 3 0 0 0.5\ntime 1\n0 1 2 3 0 0 0.5\nend\n").unwrap();
    let extra = if std::env::var("GS_PROBE_STATIC").is_ok() { "$staticprop\n" } else { "" };
    std::fs::write(
        dir.join("probe.qc"),
        extra.to_string() + "$modelname \"gsprobe/probe.mdl\"\n$body body \"ref.smd\"\n$cdmaterials \"x\"\n$illumposition 1 2 3\n$eyeposition 4 5 6\n$sequence idle \"anim.smd\" fps 30\n$collisionmodel \"phys.smd\" {\n$mass 10\n}\n",
    )
    .unwrap();
    let game = dir.join("game");
    compile::make_temp_game(&game, None).unwrap();
    let job = CompileJob { studiomdl: tf.join("bin").join("studiomdl.exe"), game_dir: game.clone(), qc: dir.join("probe.qc"), extra_args: vec![] };
    let (tx, rx) = std::sync::mpsc::channel();
    compile::run(&job, &tx);
    drop(tx);
    for m in rx {
        if let CompileMsg::Line(l) = m {
            if l.contains("ERROR") || l.contains("WARN") {
                println!("{l}");
            }
        }
    }
    let mdl = Mdl::parse(std::fs::read(game.join("models/gsprobe/probe.mdl")).unwrap()).unwrap();
    let b = &mdl.bones[0];
    println!("SMD root rot {rot}");
    println!("bone pos {:?} rot {:?}", b.pos, quat_to_euler(b.quat));
    let f = mdl.decode_frame(0, 0, None);
    println!("anim frame0 pos {:?} rot {:?}", f[0].0, quat_to_euler(f[0].1));
    println!("illum {:?} eye {:?} hull {:?} {:?}", mdl.illumposition, mdl.eyeposition, mdl.hull_min, mdl.hull_max);
    let vvd = golden_stunstick::formats::mdl::Vvd::parse(&std::fs::read(game.join("models/gsprobe/probe.vvd")).unwrap()).unwrap();
    let mn = vvd.vertices.iter().fold((glam::Vec3::splat(1e9), glam::Vec3::splat(-1e9)), |(a, b), x| (a.min(x.pos), b.max(x.pos)));
    println!("vvd bounds {mn:?}");
    let phy = Phy::parse(&std::fs::read(game.join("models/gsprobe/probe.phy")).unwrap()).unwrap();
    let mut a = glam::Vec3::splat(1e9);
    let mut z = glam::Vec3::splat(-1e9);
    for s in &phy.solids {
        for c in &s.convexes {
            for t in &c.tris {
                for v in t {
                    a = a.min(*v);
                    z = z.max(*v);
                }
            }
        }
    }
    println!("phy bounds (my conversion) {a:?} {z:?}");
    println!("{}", phy.text);
}

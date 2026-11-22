//! Tests against a real Team Fortress 2 install. Skipped automatically when
//! TF2 is not installed at the default Steam location (or GS_TF2_DIR).

use golden_stunstick::formats::mdl::{Mdl, Vtx, Vvd};
use golden_stunstick::formats::vtf;
use golden_stunstick::gamefs::GameFs;
use std::path::PathBuf;

fn tf2_dir() -> Option<PathBuf> {
    let p = std::env::var("GS_TF2_DIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\common\Team Fortress 2"));
    p.join("tf").join("gameinfo.txt").is_file().then_some(p)
}

#[test]
fn mount_and_parse_heavy() {
    let Some(tf) = tf2_dir() else { return };
    let fs = GameFs::mount(&tf.join("tf").join("gameinfo.txt"), None).expect("mount");
    for l in &fs.report {
        println!("{} -> {} [{}] {}", l.raw, l.resolved, l.ok, l.note);
    }
    let models = fs.list_models();
    println!("{} models", models.len());
    assert!(models.len() > 1000);

    for path in ["models/player/heavy.mdl", "models/weapons/w_models/w_shotgun.mdl", "models/props_2fort/oildrum.mdl"] {
        let data = fs.read(path).unwrap_or_else(|| panic!("{path} missing"));
        let mdl = Mdl::parse(data).expect("parse mdl");
        println!(
            "{path}: v{} bones={} seqs={} anims={} tex={} bodyparts={} includes={:?} animblock={:?} flexes={}",
            mdl.version,
            mdl.bones.len(),
            mdl.seqs.len(),
            mdl.anims.len(),
            mdl.textures.len(),
            mdl.bodyparts.len(),
            mdl.include_models.iter().map(|i| i.name.clone()).collect::<Vec<_>>(),
            mdl.anim_block_name,
            mdl.flex_descs.len()
        );
        let base = path.trim_end_matches(".mdl");
        let vvd = Vvd::parse(&fs.read(&format!("{base}.vvd")).unwrap()).expect("vvd");
        let vtx = Vtx::parse(&fs.read(&format!("{base}.dx90.vtx")).unwrap()).expect("vtx");
        assert_eq!(vvd.checksum, mdl.checksum);
        assert_eq!(vtx.checksum, mdl.checksum);
        let mut tris = 0;
        for (bi, bp) in mdl.bodyparts.iter().enumerate() {
            for (mi, m) in bp.models.iter().enumerate() {
                for (me, mesh) in m.meshes.iter().enumerate() {
                    for t in &vtx.bodyparts[bi][mi].lods[0][me].tris {
                        for &v in t {
                            let gi = (m.first_vertex + mesh.vertex_offset) as usize + v as usize;
                            assert!(gi < vvd.vertices.len());
                        }
                        tris += 1;
                    }
                }
            }
        }
        println!("  verts={} tris={}", vvd.vertices.len(), tris);
        assert!(tris > 0);
        for (ai, _) in mdl.anims.iter().enumerate().take(5) {
            let f = mdl.decode_frame(ai, 0, None);
            for (p, q) in &f {
                assert!(p.is_finite() && q.is_finite());
                assert!((q.length() - 1.0).abs() < 0.01, "bad quat {q:?}");
            }
        }
        // a texture
        if let Some(t) = mdl.textures.first() {
            if let Some(vmt) = fs.find_material(&mdl.cdmaterials, &t.name) {
                println!("  material {vmt}");
            }
        }
    }
    let vtf_data = fs.read("materials/models/player/heavy/heavy_red.vtf").or_else(|| fs.read("materials/models/player/hvyweapon/hvyweapon_red.vtf")).expect("vtf");
    let img = vtf::decode(&vtf_data, 1024).expect("decode vtf");
    assert!(img.width > 0 && img.pixels.len() == (img.width * img.height * 4) as usize);
}

/// Multi-strip VTX (john_freeman) and binary DMX meshes from the user's mod, if present.
#[test]
fn john_freeman_vtx_and_dmx() {
    let dir = PathBuf::from(r"C:\Program Files (x86)\Steam\steamapps\sourcemods\fc_tfsdk\models\player\john");
    if dir.join("john_freeman.mdl").is_file() {
        let mdl = Mdl::parse(std::fs::read(dir.join("john_freeman.mdl")).unwrap()).unwrap();
        let vvd = Vvd::parse(&std::fs::read(dir.join("john_freeman.vvd")).unwrap()).unwrap();
        for f in ["john_freeman.dx90.vtx", "john_freeman.dx80.vtx", "john_freeman.sw.vtx"] {
            let vtx = Vtx::parse(&std::fs::read(dir.join(f)).unwrap()).unwrap_or_else(|e| panic!("{f}: {e}"));
            let mut tris = 0;
            for (bi, bp) in mdl.bodyparts.iter().enumerate() {
                for (mi, m) in bp.models.iter().enumerate() {
                    for (me, mesh) in m.meshes.iter().enumerate() {
                        for t in &vtx.bodyparts[bi][mi].lods[0][me].tris {
                            for &v in t {
                                assert!((v as i32) < mesh.num_vertices, "{f}: vertex {v} >= {}", mesh.num_vertices);
                            }
                            tris += 1;
                        }
                    }
                }
            }
            println!("{f}: {tris} triangles, {} vertices", vvd.vertices.len());
            assert!(tris > 0);
        }
    }
    let qc = PathBuf::from(r"D:\GITREPOS\fc_tfsdk_assetsrc\content (ingame)\fc_tfsdk\models\player\john\John_XSI\Male_07.qc");
    if qc.is_file() {
        let rig = golden_stunstick::anim::Rig::load(&qc).unwrap();
        for w in &rig.warnings {
            println!("warning: {w}");
        }
        for m in &rig.meshes {
            let mats: std::collections::BTreeSet<&str> = m.tris.iter().map(|t| t.material.as_str()).collect();
            println!("mesh {} ({}): {} triangles, materials {:?}", m.file.display(), m.body, m.tris.len(), mats);
        }
        let dmx = rig.meshes.iter().find(|m| m.file.extension().unwrap() == "dmx").expect("DMX head mesh loaded");
        assert!(dmx.tris.len() > 1000);
        assert!(dmx.bone_map.iter().all(|b| b.is_some()), "every DMX joint maps onto the rig");
        let head = rig.bone_index("ValveBiped.Bip01_Head1").unwrap();
        let hw: glam::Vec3 = rig.world(&rig.bind)[head].translation.into();
        let mut top = f32::MIN;
        for t in &dmx.tris {
            for v in &t.verts {
                top = top.max(v.pos.z);
            }
        }
        println!("head bone z {:.2}, DMX mesh top z {:.2}", hw.z, top);
        assert!((top - hw.z).abs() < 15.0, "head mesh should sit at the head bone");
    }
}

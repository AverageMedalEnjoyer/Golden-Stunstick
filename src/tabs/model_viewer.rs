//! Model Viewer tab: an HLMV++-style viewer for models in the mounted game.

use crate::app::Shared;
use crate::gamefs::{GameFs, ModelEntry};
use crate::render::{self, FrameData, OrbitCamera, RenderMode};
use crate::scene::{ModelOrigin, ModelScene};
use eframe::egui::{self, Color32, RichText};
use glam::{Affine3A, Quat, Vec3};
use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;

#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum Panel {
    #[default]
    Render,
    Sequence,
    Body,
    Flexes,
    Bones,
    Attachments,
    Hitboxes,
    Physics,
    Merge,
    Materials,
    Info,
}

struct Merged {
    scene: ModelScene,
    to_main: Vec<Option<usize>>,
    visible: bool,
    skin: usize,
}

/// TF2 paint colours (name, rgb).
const PAINTS: &[(&str, [u8; 3])] = &[
    ("A Color Similar to Slate", [47, 79, 79]),
    ("A Deep Commitment to Purple", [125, 64, 113]),
    ("A Distinctive Lack of Hue", [20, 20, 20]),
    ("A Mann's Mint", [188, 221, 179]),
    ("After Eight", [45, 45, 36]),
    ("Aged Moustache Grey", [126, 126, 126]),
    ("An Extraordinary Abundance of Tinge", [230, 230, 230]),
    ("Australium Gold", [231, 181, 59]),
    ("Color No. 216-190-216", [216, 190, 216]),
    ("Dark Salmon Injustice", [233, 150, 122]),
    ("Drably Olive", [128, 128, 0]),
    ("Indubitably Green", [114, 158, 66]),
    ("Mann Co. Orange", [207, 115, 54]),
    ("Muskelmannbraun", [165, 117, 69]),
    ("Noble Hatter's Violet", [81, 56, 74]),
    ("Peculiarly Drab Tincture", [197, 175, 145]),
    ("Pink as Hell", [255, 105, 180]),
    ("Radigan Conagher Brown", [105, 77, 58]),
    ("The Bitter Taste of Defeat and Lime", [50, 205, 50]),
    ("The Color of a Gentlemann's Business Pants", [240, 230, 140]),
    ("Ye Olde Rustic Colour", [124, 108, 87]),
    ("Zepheniah's Greed", [66, 79, 59]),
    ("Team Spirit (RED)", [184, 56, 59]),
    ("Team Spirit (BLU)", [88, 133, 162]),
];

pub struct ModelViewerTab {
    scene: Option<ModelScene>,
    merged: Vec<Merged>,
    loading: Option<Receiver<Result<ModelScene, String>>>,
    loading_label: String,
    merge_loading: Option<Receiver<Result<ModelScene, String>>>,
    camera: OrbitCamera,
    panel: Panel,
    // render options
    mode: RenderMode,
    wire_overlay: bool,
    ground: bool,
    show_bones: bool,
    bone_names: bool,
    show_attachments: bool,
    attachment_names: bool,
    show_hitboxes: bool,
    hitbox_set: usize,
    show_normals: bool,
    show_physics: bool,
    show_bbox: bool,
    backface_cull: bool,
    light_yaw: f32,
    light_pitch: f32,
    ambient: f32,
    custom_bg: Option<Color32>,
    // sequence
    seq: usize,
    frame: f32,
    playing: bool,
    looping: bool,
    speed: f32,
    seq_filter: String,
    pose_params: Vec<f32>,
    last_tick: Option<Instant>,
    // body
    bodygroups: Vec<usize>,
    skin: usize,
    lod: usize,
    paint: Option<usize>,
    custom_paint: [u8; 3],
    use_custom_paint: bool,
    // flex
    flex_values: Vec<f32>,
    // selection
    sel_bone: Option<usize>,
    sel_attachment: Option<usize>,
    sel_hitbox: Option<usize>,
    // open dialog
    open_dialog: bool,
    open_for_merge: bool,
    models: Option<Arc<Vec<ModelEntry>>>,
    filter_name: String,
    filter_folder: String,
    filter_source: Option<usize>,
    only_overridden: bool,
    pending_screenshot: bool,
    wanted_seq: Option<String>,
}

impl Default for ModelViewerTab {
    fn default() -> Self {
        ModelViewerTab {
            scene: None,
            merged: Vec::new(),
            loading: None,
            loading_label: String::new(),
            merge_loading: None,
            camera: OrbitCamera::default(),
            panel: Panel::Render,
            mode: RenderMode::Textured,
            wire_overlay: false,
            ground: true,
            show_bones: false,
            bone_names: false,
            show_attachments: false,
            attachment_names: false,
            show_hitboxes: false,
            hitbox_set: 0,
            show_normals: false,
            show_physics: false,
            show_bbox: false,
            backface_cull: false,
            light_yaw: 40.0,
            light_pitch: 35.0,
            ambient: 0.35,
            custom_bg: None,
            seq: 0,
            frame: 0.0,
            playing: true,
            looping: true,
            speed: 1.0,
            seq_filter: String::new(),
            pose_params: Vec::new(),
            last_tick: None,
            bodygroups: Vec::new(),
            skin: 0,
            lod: 0,
            paint: None,
            custom_paint: [255, 255, 255],
            use_custom_paint: false,
            flex_values: Vec::new(),
            sel_bone: None,
            sel_attachment: None,
            sel_hitbox: None,
            open_dialog: false,
            open_for_merge: false,
            models: None,
            filter_name: String::new(),
            filter_folder: String::new(),
            filter_source: None,
            only_overridden: false,
            pending_screenshot: false,
            wanted_seq: None,
        }
    }
}

fn spawn_load(fs: Option<Arc<GameFs>>, path: String, from_disk: bool) -> Receiver<Result<ModelScene, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let origin = if from_disk { ModelOrigin::Disk(std::path::Path::new(&path)) } else { ModelOrigin::Game(&path) };
        let _ = tx.send(ModelScene::load(fs.as_deref(), origin));
    });
    rx
}

impl ModelViewerTab {
    fn on_loaded(&mut self, s: ModelScene, sh: &mut Shared) {
        let m = s.main();
        self.bodygroups = vec![0; m.bodyparts.len()];
        self.pose_params = m.pose_params.iter().map(|p| if p.start <= 0.0 && p.end >= 0.0 { 0.0 } else { (p.start + p.end) * 0.5 }).collect();
        self.flex_values = m.flex_controllers.iter().map(|_| 0.0).collect();
        self.skin = 0;
        self.lod = 0;
        self.hitbox_set = 0;
        self.sel_bone = None;
        self.sel_attachment = None;
        self.sel_hitbox = None;
        // prefer a full-body (non-delta) idle / stand sequence, like HLMV does
        let pick = |pred: &dyn Fn(&crate::scene::SeqRef) -> bool| s.seqs.iter().position(|q| !q.delta && pred(q));
        self.seq = self
            .wanted_seq
            .take()
            .and_then(|w| s.seqs.iter().position(|q| q.name.eq_ignore_ascii_case(&w)))
            .or_else(|| pick(&|q| q.name.to_ascii_lowercase().starts_with("stand_")))
            .or_else(|| pick(&|q| q.activity.contains("IDLE") || q.name.to_ascii_lowercase().contains("idle")))
            .or_else(|| pick(&|_| true))
            .unwrap_or(0);
        self.frame = 0.0;
        let (mn, mx) = s.frame_bounds();
        self.camera = OrbitCamera { yaw: 0.0, ..OrbitCamera::default() };
        self.camera.frame_bounds(mn, mx);
        for w in &s.warnings {
            sh.error(w.clone());
        }
        if let Ok(mut r) = sh.renderer.lock() {
            if let Some(r) = r.as_mut() {
                for up in &s.uploads {
                    r.queue_upload(crate::render::TextureUpload { key: up.key.clone(), width: up.width, height: up.height, rgba: up.rgba.clone(), clamp: up.clamp });
                }
            }
        }
        self.merged.clear();
        self.scene = Some(s);
    }

    fn queue_textures(&self, sh: &Shared) {
        // the renderer is created lazily on first paint; make sure textures reach it
        let Ok(mut r) = sh.renderer.lock() else { return };
        let Some(r) = r.as_mut() else { return };
        let mut push = |s: &ModelScene| {
            for up in &s.uploads {
                if !r.has_texture(&up.key) {
                    r.queue_upload(crate::render::TextureUpload { key: up.key.clone(), width: up.width, height: up.height, rgba: up.rgba.clone(), clamp: up.clamp });
                }
            }
        };
        if let Some(s) = &self.scene {
            push(s);
        }
        for m in &self.merged {
            push(&m.scene);
        }
    }

    /// Start loading a model from the mounted game (used by the --model command line option).
    pub fn request_model(&mut self, sh: &Shared, path: &str, seq: Option<String>) {
        self.wanted_seq = seq;
        self.loading_label = path.to_string();
        self.loading = Some(spawn_load(sh.fs.clone(), path.to_string(), false));
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        // async loads
        if let Some(rx) = &self.loading {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            if let Ok(res) = rx.try_recv() {
                self.loading = None;
                match res {
                    Ok(s) => self.on_loaded(s, sh),
                    Err(e) => sh.error(e),
                }
            }
        }
        if let Some(rx) = &self.merge_loading {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
            if let Ok(res) = rx.try_recv() {
                self.merge_loading = None;
                match res {
                    Ok(s) => {
                        if let Some(main) = &self.scene {
                            let to_main = s.bones().iter().map(|b| main.main().bone_index(&b.name)).collect();
                            self.merged.push(Merged { scene: s, to_main, visible: true, skin: 0 });
                        }
                    }
                    Err(e) => sh.error(e),
                }
            }
        }
        self.queue_textures(sh);
        if let Some(r) = sh.renderer.lock().ok().and_then(|mut r| r.as_mut().and_then(|r| r.last_message.take())) {
            sh.notify(r);
        }

        self.toolbar(ui, sh);
        ui.separator();
        egui::Panel::bottom("viewer_bottom").resizable(true).default_size(270.0).min_size(120.0).frame(egui::Frame::new()).show(ui, |ui| {
            self.panel_tabs(ui);
            ui.separator();
            egui::ScrollArea::vertical().id_salt("viewer_panel").auto_shrink([false, false]).show(ui, |ui| self.panel_contents(ui, sh));
        });
        egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| self.viewport(ui, sh));
        self.open_dialog_ui(ui.ctx(), sh);
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        ui.horizontal(|ui| {
            if ui.button("Open Model...").clicked() {
                self.open_dialog = true;
                self.open_for_merge = false;
            }
            if ui.button("Open from disk...").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("Studio model", &["mdl"]).pick_file() {
                    self.loading_label = p.display().to_string();
                    self.loading = Some(spawn_load(sh.fs.clone(), p.display().to_string(), true));
                }
            }
            if let Some(s) = &self.scene {
                if ui.button("Reload").clicked() {
                    let disk = s.mdl_source == "disk";
                    self.loading_label = s.path.clone();
                    self.loading = Some(spawn_load(sh.fs.clone(), s.path.clone(), disk));
                }
                if ui.button("Reset camera").clicked() {
                    let (mn, mx) = s.frame_bounds();
                    self.camera = OrbitCamera { yaw: 0.0, ..OrbitCamera::default() };
                    self.camera.frame_bounds(mn, mx);
                }
                if ui.button("Screenshot").clicked() {
                    self.pending_screenshot = true;
                }
                ui.separator();
                ui.label(RichText::new(&s.path).strong());
                ui.label(RichText::new(format!("from {}", s.mdl_source)).weak());
            }
            if self.loading.is_some() || self.merge_loading.is_some() {
                ui.spinner();
                ui.label(format!("Loading {}...", self.loading_label));
            }
            if sh.fs.is_none() {
                ui.label(RichText::new("(game content still mounting)").weak());
            }
        });
    }

    fn panel_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (p, l) in [
                (Panel::Render, "Render"),
                (Panel::Sequence, "Sequence"),
                (Panel::Body, "Body / Skin / Paint"),
                (Panel::Flexes, "Flexes"),
                (Panel::Bones, "Bones"),
                (Panel::Attachments, "Attachments"),
                (Panel::Hitboxes, "Hitboxes"),
                (Panel::Physics, "Physics"),
                (Panel::Merge, "Bonemerge"),
                (Panel::Materials, "Materials"),
                (Panel::Info, "Model Info"),
            ] {
                ui.selectable_value(&mut self.panel, p, l);
            }
        });
    }

    fn panel_contents(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        let Some(scene) = &self.scene else {
            ui.label("Open a model with \"Open Model...\" (from the mounted game) or \"Open from disk...\".");
            return;
        };
        match self.panel {
            Panel::Render => {
                ui.horizontal_wrapped(|ui| {
                    ui.label("Mode:");
                    ui.selectable_value(&mut self.mode, RenderMode::Wireframe, "Wireframe");
                    ui.selectable_value(&mut self.mode, RenderMode::Flat, "Flat shaded");
                    ui.selectable_value(&mut self.mode, RenderMode::Smooth, "Smooth shaded");
                    ui.selectable_value(&mut self.mode, RenderMode::Textured, "Textured");
                    ui.separator();
                    ui.checkbox(&mut self.wire_overlay, "Wireframe overlay");
                    ui.checkbox(&mut self.backface_cull, "Backface culling");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.ground, "Ground");
                    ui.checkbox(&mut self.show_bones, "Bones");
                    ui.checkbox(&mut self.bone_names, "Bone names");
                    ui.checkbox(&mut self.show_attachments, "Attachments");
                    ui.checkbox(&mut self.attachment_names, "Attachment names");
                    ui.checkbox(&mut self.show_hitboxes, "Hitboxes");
                    ui.checkbox(&mut self.show_normals, "Normals");
                    ui.checkbox(&mut self.show_physics, "Physics model");
                    ui.checkbox(&mut self.show_bbox, "Bounding boxes");
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Light yaw");
                    ui.add(egui::Slider::new(&mut self.light_yaw, -180.0..=180.0).suffix("°"));
                    ui.label("pitch");
                    ui.add(egui::Slider::new(&mut self.light_pitch, -89.0..=89.0).suffix("°"));
                    ui.label("ambient");
                    ui.add(egui::Slider::new(&mut self.ambient, 0.0..=1.0));
                });
                ui.horizontal_wrapped(|ui| {
                    ui.label("Field of view");
                    ui.add(egui::Slider::new(&mut self.camera.fov, 20.0..=120.0).suffix("°"));
                    let mut custom = self.custom_bg.is_some();
                    if ui.checkbox(&mut custom, "Custom background").changed() {
                        self.custom_bg = if custom { Some(sh.theme.viewport_bg) } else { None };
                    }
                    if let Some(c) = &mut self.custom_bg {
                        ui.color_edit_button_srgba(c);
                    }
                });
                ui.label(RichText::new("Mouse: left-drag rotate, right-drag zoom, middle/shift-drag pan, wheel zoom.").small().weak());
            }
            Panel::Sequence => self.sequence_panel(ui),
            Panel::Body => {
                let m = scene.main();
                egui::Grid::new("bodygroups").num_columns(2).show(ui, |ui| {
                    for (i, bp) in m.bodyparts.iter().enumerate() {
                        if bp.models.len() < 2 {
                            continue;
                        }
                        ui.label(&bp.name);
                        let sel = self.bodygroups.get(i).copied().unwrap_or(0);
                        egui::ComboBox::from_id_salt(("bg", i)).selected_text(format!("{sel}: {}", bp.models.get(sel).map(|x| if x.name.is_empty() { "blank".to_string() } else { x.name.clone() }).unwrap_or_default())).show_ui(ui, |ui| {
                            for (k, md) in bp.models.iter().enumerate() {
                                let label = format!("{k}: {}", if md.name.is_empty() || md.meshes.is_empty() { "blank".to_string() } else { md.name.clone() });
                                if ui.selectable_label(sel == k, label).clicked() {
                                    self.bodygroups[i] = k;
                                }
                            }
                        });
                        ui.end_row();
                    }
                    ui.label("Skin");
                    let n = m.skins.len().max(1);
                    ui.add(egui::Slider::new(&mut self.skin, 0..=n - 1));
                    ui.end_row();
                    let nlods = scene.vtx.as_ref().map(|v| v.num_lods.max(1) as usize).unwrap_or(1);
                    ui.label("LOD");
                    ui.add(egui::Slider::new(&mut self.lod, 0..=nlods - 1));
                    ui.end_row();
                });
                ui.separator();
                let paintable = scene.materials.iter().any(|m| m.paintable);
                ui.label(RichText::new(if paintable { "Paint (TF2 paintable material detected)" } else { "Paint (no paintable material on this model)" }).strong());
                ui.horizontal_wrapped(|ui| {
                    if ui.selectable_label(self.paint.is_none() && !self.use_custom_paint, "No paint").clicked() {
                        self.paint = None;
                        self.use_custom_paint = false;
                    }
                    for (i, (name, c)) in PAINTS.iter().enumerate() {
                        let col = Color32::from_rgb(c[0], c[1], c[2]);
                        let r = ui.add(egui::Button::new(RichText::new("■").color(col)).selected(self.paint == Some(i))).on_hover_text(*name);
                        if r.clicked() {
                            self.paint = Some(i);
                            self.use_custom_paint = false;
                        }
                    }
                    ui.checkbox(&mut self.use_custom_paint, "Custom");
                    if self.use_custom_paint {
                        ui.color_edit_button_srgb(&mut self.custom_paint);
                    }
                });
            }
            Panel::Flexes => {
                let m = scene.main();
                if m.flex_controllers.is_empty() {
                    ui.label("This model has no flex controllers.");
                } else {
                    if ui.button("Reset all").clicked() {
                        self.flex_values.iter_mut().for_each(|v| *v = 0.0);
                    }
                    egui::Grid::new("flexes").num_columns(4).show(ui, |ui| {
                        for (i, c) in m.flex_controllers.iter().enumerate() {
                            ui.label(&c.name);
                            if let Some(v) = self.flex_values.get_mut(i) {
                                ui.add(egui::Slider::new(v, c.min..=c.max.max(c.min + 0.001)));
                            }
                            if i % 2 == 1 {
                                ui.end_row();
                            }
                        }
                    });
                }
            }
            Panel::Bones => {
                let m = scene.main();
                ui.label(format!("{} bones (click to highlight)", m.bones.len()));
                ui.horizontal_top(|ui| {
                    egui::ScrollArea::vertical().id_salt("bone_list").max_height(200.0).show(ui, |ui| {
                        ui.set_min_width(260.0);
                        for (i, b) in m.bones.iter().enumerate() {
                            if ui.selectable_label(self.sel_bone == Some(i), format!("{i}: {}", b.name)).clicked() {
                                self.sel_bone = Some(i);
                            }
                        }
                    });
                    if let Some(b) = self.sel_bone.and_then(|i| m.bones.get(i)) {
                        ui.vertical(|ui| {
                            ui.label(RichText::new(&b.name).strong());
                            ui.label(format!("parent: {}", if b.parent >= 0 { m.bones[b.parent as usize].name.clone() } else { "(root)".into() }));
                            ui.label(format!("position: {:.3} {:.3} {:.3}", b.pos.x, b.pos.y, b.pos.z));
                            let e = crate::formats::mdl::quat_to_euler(b.quat);
                            ui.label(format!("rotation (deg): {:.2} {:.2} {:.2}", e.x.to_degrees(), e.y.to_degrees(), e.z.to_degrees()));
                            ui.label(format!("flags: {:#x}  procedural type: {}  surfaceprop: {}", b.flags, b.proctype, b.surfaceprop));
                        });
                    }
                });
            }
            Panel::Attachments => {
                let m = scene.main();
                if m.attachments.is_empty() {
                    ui.label("No attachments.");
                }
                for (i, a) in m.attachments.iter().enumerate() {
                    let bone = m.bones.get(a.bone.max(0) as usize).map(|b| b.name.as_str()).unwrap_or("");
                    if ui.selectable_label(self.sel_attachment == Some(i), format!("{}  (bone {bone}, offset {:.2} {:.2} {:.2})", a.name, a.local[0][3], a.local[1][3], a.local[2][3])).clicked() {
                        self.sel_attachment = Some(i);
                        self.show_attachments = true;
                    }
                }
            }
            Panel::Hitboxes => {
                let m = scene.main();
                if m.hitbox_sets.is_empty() {
                    ui.label("No hitboxes.");
                    return;
                }
                ui.horizontal(|ui| {
                    ui.label("Set");
                    for (i, s) in m.hitbox_sets.iter().enumerate() {
                        ui.selectable_value(&mut self.hitbox_set, i, &s.name);
                    }
                    ui.checkbox(&mut self.show_hitboxes, "Show");
                });
                if let Some(set) = m.hitbox_sets.get(self.hitbox_set) {
                    for (i, h) in set.hitboxes.iter().enumerate() {
                        let bone = m.bones.get(h.bone.max(0) as usize).map(|b| b.name.as_str()).unwrap_or("");
                        if ui.selectable_label(self.sel_hitbox == Some(i), format!("group {}  {bone}  min ({:.1} {:.1} {:.1}) max ({:.1} {:.1} {:.1})", h.group, h.min.x, h.min.y, h.min.z, h.max.x, h.max.y, h.max.z)).clicked() {
                            self.sel_hitbox = Some(i);
                            self.show_hitboxes = true;
                        }
                    }
                }
            }
            Panel::Physics => match &scene.phy {
                None => {
                    ui.label("This model has no .phy collision model.");
                }
                Some(phy) => {
                    ui.checkbox(&mut self.show_physics, "Show physics model");
                    ui.label(format!("{} solid(s)", phy.solids.len()));
                    for (i, name, kv) in phy.solid_infos() {
                        let pieces = phy.solids.get(i).map(|s| s.convexes.len()).unwrap_or(0);
                        ui.label(format!("{i}: {name}  mass {}  surfaceprop {}  {pieces} convex piece(s)", kv.get_str("mass").unwrap_or("?"), kv.get_str("surfaceprop").unwrap_or("?")));
                    }
                    if let Some(e) = phy.block("editparams") {
                        ui.label(format!("total mass {}  root {}  concave {}", e.get_str("totalmass").unwrap_or("?"), e.get_str("rootname").unwrap_or(""), e.get_str("concave").unwrap_or("0")));
                    }
                }
            },
            Panel::Merge => {
                ui.label("Bonemerge extra models (weapons, cosmetics) onto this model by matching bone names.");
                ui.horizontal(|ui| {
                    if ui.button("Add model from game...").clicked() {
                        self.open_dialog = true;
                        self.open_for_merge = true;
                    }
                    if ui.button("Add model from disk...").clicked() {
                        if let Some(p) = rfd::FileDialog::new().add_filter("Studio model", &["mdl"]).pick_file() {
                            self.merge_loading = Some(spawn_load(sh.fs.clone(), p.display().to_string(), true));
                        }
                    }
                });
                let mut remove = None;
                for (i, m) in self.merged.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut m.visible, &m.scene.path);
                        let matched = m.to_main.iter().filter(|x| x.is_some()).count();
                        ui.label(RichText::new(format!("{matched}/{} bones matched", m.to_main.len())).weak());
                        let n = m.scene.main().skins.len().max(1);
                        if n > 1 {
                            ui.add(egui::Slider::new(&mut m.skin, 0..=n - 1).text("skin"));
                        }
                        if ui.button("Remove").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    self.merged.remove(i);
                }
            }
            Panel::Materials => {
                egui::Grid::new("materials").striped(true).num_columns(4).show(ui, |ui| {
                    ui.label(RichText::new("Material").strong());
                    ui.label(RichText::new("VMT (source)").strong());
                    ui.label(RichText::new("$basetexture (source)").strong());
                    ui.label(RichText::new("Notes").strong());
                    ui.end_row();
                    for m in &scene.materials {
                        ui.label(&m.name);
                        ui.label(format!("{} ({})", m.vmt_path.clone().unwrap_or("-".into()), m.vmt_source.clone().unwrap_or_default()));
                        ui.label(format!("{} ({})", m.base_texture.clone().unwrap_or("-".into()), m.base_texture_source.clone().unwrap_or_default()));
                        let mut notes = vec![m.shader.clone()];
                        if m.translucent {
                            notes.push("translucent".into());
                        }
                        if m.alphatest.is_some() {
                            notes.push("alphatest".into());
                        }
                        if m.paintable {
                            notes.push("paintable".into());
                        }
                        match &m.error {
                            Some(e) => ui.colored_label(Color32::from_rgb(255, 120, 120), e),
                            None => ui.label(notes.join(", ")),
                        };
                        ui.end_row();
                    }
                });
                ui.label(RichText::new(format!("$cdmaterials: {}", scene.main().cdmaterials.join(", "))).weak());
            }
            Panel::Info => {
                let m = scene.main();
                egui::Grid::new("info").num_columns(2).striped(true).show(ui, |ui| {
                    let mut row = |k: &str, v: String| {
                        ui.label(RichText::new(k).strong());
                        ui.label(v);
                        ui.end_row();
                    };
                    row("Name", m.name.clone());
                    row("File", format!("{} (from {})", scene.path, scene.mdl_source));
                    row("Version", format!("{}", m.version));
                    row("Checksum", format!("{}", m.checksum));
                    row("Bones", format!("{}", m.bones.len()));
                    row("Sequences", format!("{} local, {} total incl. $includemodel", m.seqs.len(), scene.seqs.len()));
                    row("Animations", format!("{}", m.anims.len()));
                    row("Body parts", format!("{}", m.bodyparts.len()));
                    row("Materials / skins", format!("{} / {}", m.textures.len(), m.skins.len()));
                    row("Attachments", format!("{}", m.attachments.len()));
                    row("Hitbox sets", format!("{}", m.hitbox_sets.len()));
                    row("Flexes / controllers", format!("{} / {}", m.flex_descs.len(), m.flex_controllers.len()));
                    row("Pose parameters", m.pose_params.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", "));
                    row("IK chains", m.ik_chains.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", "));
                    row("Include models", m.include_models.iter().map(|p| p.name.clone()).collect::<Vec<_>>().join(", "));
                    row("Surface prop", m.surfaceprop.clone());
                    row("Mass", format!("{}", m.mass));
                    row("Flags", format!("{:#x}{}", m.flags, if m.flags & 0x10 != 0 { " (static prop)" } else { "" }));
                    row("Eye position", format!("{:.2} {:.2} {:.2}", m.eyeposition.x, m.eyeposition.y, m.eyeposition.z));
                    row("Hull", format!("{:?} - {:?}", m.hull_min, m.hull_max));
                    let verts = scene.vvd.as_ref().map(|v| v.vertices.len()).unwrap_or(0);
                    row("Vertices (LOD0)", format!("{verts}"));
                    row("LODs", format!("{}", scene.vtx.as_ref().map(|v| v.num_lods).unwrap_or(0)));
                });
                if !m.keyvalues.is_empty() {
                    ui.collapsing("Model keyvalues", |ui| ui.label(RichText::new(&m.keyvalues).monospace()));
                }
            }
        }
    }

    fn sequence_panel(&mut self, ui: &mut egui::Ui) {
        let Some(scene) = &self.scene else { return };
        ui.horizontal(|ui| {
            if ui.button(if self.playing { "Pause" } else { "Play" }).clicked() {
                self.playing = !self.playing;
            }
            if ui.button("|<").clicked() {
                self.frame = 0.0;
            }
            if ui.button("<").clicked() {
                self.frame = (self.frame.floor() - 1.0).max(0.0);
                self.playing = false;
            }
            if ui.button(">").clicked() {
                self.frame += 1.0;
                self.playing = false;
            }
            ui.checkbox(&mut self.looping, "Loop");
            ui.label("Speed");
            ui.add(egui::Slider::new(&mut self.speed, 0.0..=4.0).suffix("x"));
            if let Some(sq) = scene.seqs.get(self.seq) {
                let max = (sq.frames - 1).max(0) as f32;
                ui.label("Frame");
                ui.add(egui::Slider::new(&mut self.frame, 0.0..=max.max(0.001)).max_decimals(1));
                ui.label(format!("/ {}  @ {:.0} fps", sq.frames - 1, sq.fps));
            }
        });
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label("Search");
                    ui.add(egui::TextEdit::singleline(&mut self.seq_filter).desired_width(200.0));
                });
                let f = self.seq_filter.to_ascii_lowercase();
                let list: Vec<usize> = (0..scene.seqs.len()).filter(|&i| f.is_empty() || scene.seqs[i].name.to_ascii_lowercase().contains(&f) || scene.seqs[i].activity.to_ascii_lowercase().contains(&f)).collect();
                egui::ScrollArea::vertical().id_salt("seq_list").max_height(170.0).show_rows(ui, 18.0, list.len(), |ui, range| {
                    ui.set_min_width(330.0);
                    for &i in &list[range] {
                        let s = &scene.seqs[i];
                        let label = if s.activity.is_empty() { s.name.clone() } else { format!("{}  [{}]", s.name, s.activity) };
                        let label = if s.source > 0 { format!("{label}  *") } else { label };
                        if ui.selectable_label(self.seq == i, label).clicked() {
                            self.seq = i;
                            self.frame = 0.0;
                        }
                    }
                });
                ui.label(RichText::new("* = from an $includemodel").small().weak());
            });
            ui.vertical(|ui| {
                let m = scene.main();
                if !m.pose_params.is_empty() {
                    ui.label(RichText::new("Pose parameters").strong());
                    for (i, p) in m.pose_params.iter().enumerate() {
                        if let Some(v) = self.pose_params.get_mut(i) {
                            let (a, b) = if p.start <= p.end { (p.start, p.end) } else { (p.end, p.start) };
                            ui.add(egui::Slider::new(v, a..=b.max(a + 0.001)).text(&p.name));
                        }
                    }
                }
                if let Some(sr) = scene.seqs.get(self.seq) {
                    let src = &scene.sources[sr.source];
                    let sd = &src.mdl.seqs[sr.index];
                    ui.label(RichText::new(format!("{}  ({} frames, {}{}{})", sd.name, sr.frames, if sr.looping { "looping" } else { "not looping" }, if sr.delta { ", delta" } else { "" }, if sd.groupsize[0] * sd.groupsize[1] > 1 { format!(", {}x{} blend", sd.groupsize[0], sd.groupsize[1]) } else { String::new() })).strong());
                    if sr.source > 0 {
                        ui.label(RichText::new(format!("from {}", src.name)).weak());
                    }
                    if !sd.events.is_empty() {
                        ui.label("Events:");
                        for e in &sd.events {
                            ui.label(RichText::new(format!("  frame {:.1}: {} {}", e.cycle * (sr.frames - 1).max(1) as f32, if e.name.is_empty() { e.event.to_string() } else { e.name.clone() }, e.options)).monospace());
                        }
                    }
                }
            });
        });
    }

    fn open_dialog_ui(&mut self, ctx: &egui::Context, sh: &mut Shared) {
        if !self.open_dialog {
            return;
        }
        let Some(fs) = sh.fs.clone() else {
            self.open_dialog = false;
            sh.error("Game content is still being mounted; try again in a moment.");
            return;
        };
        if self.models.is_none() {
            self.models = Some(Arc::new(fs.list_models()));
        }
        let models = self.models.clone().unwrap_or_default();
        let mut open = true;
        let mut chosen: Option<String> = None;
        egui::Window::new(if self.open_for_merge { "Add bonemerged model" } else { "Open Model" }).open(&mut open).default_size([760.0, 520.0]).show(ctx, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.label("Name");
                ui.add(egui::TextEdit::singleline(&mut self.filter_name).hint_text("contains...").desired_width(180.0));
                ui.label("Folder");
                ui.add(egui::TextEdit::singleline(&mut self.filter_folder).hint_text("e.g. models/player").desired_width(160.0));
                ui.label("Source");
                egui::ComboBox::from_id_salt("src_filter")
                    .width(240.0)
                    .selected_text(self.filter_source.and_then(|i| fs.sources.get(i)).map(|s| s.short_name()).unwrap_or_else(|| "All search paths".into()))
                    .show_ui(ui, |ui| {
                        if ui.selectable_label(self.filter_source.is_none(), "All search paths").clicked() {
                            self.filter_source = None;
                        }
                        for (i, s) in fs.sources.iter().enumerate() {
                            if ui.selectable_label(self.filter_source == Some(i), format!("{i}: {}", s.short_name())).clicked() {
                                self.filter_source = Some(i);
                            }
                        }
                    });
                ui.checkbox(&mut self.only_overridden, "Only models that override another copy");
            });
            let name = self.filter_name.to_ascii_lowercase();
            let folder = self.filter_folder.to_ascii_lowercase().replace('\\', "/");
            let list: Vec<&ModelEntry> = models
                .iter()
                .filter(|m| {
                    let file = m.path.rsplit('/').next().unwrap_or(&m.path);
                    (name.is_empty() || file.contains(&name))
                        && (folder.is_empty() || m.path.starts_with(folder.trim_start_matches('/')))
                        && self.filter_source.map(|s| m.source == s).unwrap_or(true)
                        && (!self.only_overridden || !m.shadowed.is_empty())
                })
                .collect();
            ui.label(RichText::new(format!("{} of {} models. The copy from the first (top-most) search path is always used.", list.len(), models.len())).small().weak());
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, 18.0, list.len(), |ui, range| {
                for m in &list[range] {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(false, &m.path).double_clicked() {
                            chosen = Some(m.path.clone());
                        }
                        let src = fs.sources.get(m.source).map(|s| s.short_name()).unwrap_or_default();
                        ui.label(RichText::new(src).small().weak());
                        if !m.shadowed.is_empty() {
                            ui.label(RichText::new(format!("(overrides {} other)", m.shadowed.len())).small().color(Color32::from_rgb(230, 190, 90)));
                        }
                        if ui.small_button("Open").clicked() {
                            chosen = Some(m.path.clone());
                        }
                    });
                }
            });
        });
        if let Some(p) = chosen {
            self.open_dialog = false;
            if self.open_for_merge {
                self.merge_loading = Some(spawn_load(Some(fs), p, false));
            } else {
                self.loading_label = p.clone();
                self.loading = Some(spawn_load(Some(fs), p, false));
            }
        } else if !open {
            self.open_dialog = false;
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        self.camera.handle_input(&resp, ui);
        if resp.hovered() && ui.input(|i| i.key_pressed(egui::Key::F)) {
            if let Some(s) = &self.scene {
                let (mn, mx) = s.frame_bounds();
                self.camera.frame_bounds(mn, mx);
            }
        }
        let bg = self.custom_bg.unwrap_or(sh.theme.viewport_bg);
        let mut f = FrameData {
            clear: Some([bg.r() as f32 / 255.0, bg.g() as f32 / 255.0, bg.b() as f32 / 255.0, 1.0]),
            light_dir: Vec3::new(self.light_pitch.to_radians().cos() * self.light_yaw.to_radians().cos(), self.light_pitch.to_radians().cos() * self.light_yaw.to_radians().sin(), self.light_pitch.to_radians().sin()),
            ambient: self.ambient,
            mode: Some(self.mode),
            wire_overlay: self.wire_overlay,
            backface_cull: self.backface_cull,
            ..Default::default()
        };
        let aspect = rect.width() / rect.height().max(1.0);
        f.view_proj = self.camera.view_proj(aspect);
        if self.ground {
            let size = self.scene.as_ref().map(|s| {
                let (a, b) = s.bounds();
                ((b - a).length() * 1.5).max(64.0)
            });
            let size = size.unwrap_or(128.0);
            let step = if size > 2048.0 { 128.0 } else if size > 512.0 { 32.0 } else { 8.0 };
            render::grid(&mut f.lines, (size / step).ceil() * step, step);
        }

        // advance animation
        let now = Instant::now();
        let dt = self.last_tick.map(|t| now.duration_since(t).as_secs_f32()).unwrap_or(0.0).min(0.1);
        self.last_tick = Some(now);
        let mut labels: Vec<(egui::Pos2, String, Color32)> = Vec::new();
        if let Some(scene) = &self.scene {
            if let Some(sq) = scene.seqs.get(self.seq) {
                let max = (sq.frames - 1).max(0) as f32;
                if self.playing && max > 0.0 {
                    self.frame += dt * sq.fps * self.speed;
                    if self.frame > max {
                        self.frame = if self.looping { self.frame % max.max(1e-3) } else { max };
                    }
                    ui.ctx().request_repaint();
                }
                self.frame = self.frame.clamp(0.0, max);
            }
            let pose = if scene.seqs.is_empty() { scene.bind_pose() } else { scene.sequence_pose(self.seq, self.frame, &self.pose_params) };
            let world = scene.world(&pose);
            let body: i32 = scene.main().bodyparts.iter().enumerate().map(|(i, bp)| bp.base * self.bodygroups.get(i).copied().unwrap_or(0) as i32).sum();
            let flex_w = if scene.main().flex_descs.is_empty() { None } else { Some(scene.flex_weights(&self.flex_values)) };
            let tint = if self.use_custom_paint {
                Some([self.custom_paint[0] as f32 / 255.0, self.custom_paint[1] as f32 / 255.0, self.custom_paint[2] as f32 / 255.0])
            } else {
                self.paint.map(|i| PAINTS[i].1).map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0])
            };
            let mut normals = Vec::new();
            scene.build_geometry(&world, body, self.skin, self.lod, flex_w.as_deref(), tint, &mut f.vertices, &mut f.indices, &mut f.draws, if self.show_normals { Some(&mut normals) } else { None });
            f.lines.extend(normals);
            // bonemerged models
            for m in &self.merged {
                if !m.visible {
                    continue;
                }
                let mpose = m.scene.bind_pose();
                let mut mworld: Vec<Affine3A> = Vec::with_capacity(mpose.len());
                for (i, b) in m.scene.bones().iter().enumerate() {
                    let w = match m.to_main.get(i).copied().flatten() {
                        Some(mi) => world[mi],
                        None => {
                            let local = Affine3A::from_rotation_translation(mpose[i].1, mpose[i].0);
                            if b.parent >= 0 { mworld[b.parent as usize] * local } else { local }
                        }
                    };
                    mworld.push(w);
                }
                m.scene.build_geometry(&mworld, 0, m.skin, self.lod, None, tint, &mut f.vertices, &mut f.indices, &mut f.draws, None);
            }
            let vp = f.view_proj;
            if self.show_bones || self.sel_bone.is_some() {
                for (i, b) in scene.bones().iter().enumerate() {
                    let p = world[i].translation.into();
                    let sel = self.sel_bone == Some(i);
                    if b.parent >= 0 && (self.show_bones || sel) {
                        let pp: Vec3 = world[b.parent as usize].translation.into();
                        render::line(&mut f.overlay_lines, pp, p, if sel { [1.0, 1.0, 0.2, 1.0] } else { [0.9, 0.75, 0.2, 0.9] });
                    }
                    if sel {
                        render::axes(&mut f.overlay_lines, world[i], 3.0);
                    }
                    if self.bone_names || sel {
                        if let Some(sp) = render::project(vp, rect, p) {
                            labels.push((sp, b.name.clone(), Color32::from_rgb(240, 220, 120)));
                        }
                    }
                }
            }
            if self.show_attachments {
                for (i, a) in scene.main().attachments.iter().enumerate() {
                    let bw = world.get(a.bone.max(0) as usize).copied().unwrap_or(Affine3A::IDENTITY);
                    let local = Affine3A::from_mat3_translation(
                        glam::Mat3::from_cols(Vec3::new(a.local[0][0], a.local[1][0], a.local[2][0]), Vec3::new(a.local[0][1], a.local[1][1], a.local[2][1]), Vec3::new(a.local[0][2], a.local[1][2], a.local[2][2])),
                        Vec3::new(a.local[0][3], a.local[1][3], a.local[2][3]),
                    );
                    let xf = bw * local;
                    render::axes(&mut f.overlay_lines, xf, if self.sel_attachment == Some(i) { 6.0 } else { 3.0 });
                    if self.attachment_names || self.sel_attachment == Some(i) {
                        if let Some(sp) = render::project(vp, rect, xf.translation.into()) {
                            labels.push((sp, a.name.clone(), Color32::from_rgb(150, 220, 255)));
                        }
                    }
                }
            }
            if self.show_hitboxes {
                scene.hitbox_lines(&world, self.hitbox_set, self.sel_hitbox, &mut f.overlay_lines);
            }
            if self.show_physics {
                scene.physics_lines(&world, &mut f.lines);
            }
            if self.show_bbox {
                let m = scene.main();
                render::wire_box(&mut f.lines, Affine3A::IDENTITY, m.hull_min, m.hull_max, [0.3, 1.0, 0.3, 1.0]);
                if m.view_bbmin != m.view_bbmax {
                    render::wire_box(&mut f.lines, Affine3A::IDENTITY, m.view_bbmin, m.view_bbmax, [1.0, 0.3, 0.3, 1.0]);
                }
            }
            // click to select bone
            if resp.clicked() && self.show_bones {
                if let Some(pos) = resp.interact_pointer_pos() {
                    let mut best = (12.0f32, None);
                    for i in 0..world.len() {
                        if let Some(sp) = render::project(vp, rect, world[i].translation.into()) {
                            let d = sp.distance(pos);
                            if d < best.0 {
                                best = (d, Some(i));
                            }
                        }
                    }
                    if best.1.is_some() {
                        self.sel_bone = best.1;
                    }
                }
            }
        }
        if self.pending_screenshot {
            self.pending_screenshot = false;
            let name = format!("screenshot_{}.png", chrono::Local::now().format("%Y%m%d_%H%M%S"));
            let dir = sh.session.as_ref().map(|s| s.dir.join("screenshots")).unwrap_or_else(|| std::path::PathBuf::from("."));
            let _ = std::fs::create_dir_all(&dir);
            f.screenshot = Some(dir.join(name));
        }
        render::paint_viewport(ui, rect, sh.renderer.clone(), f);
        let painter = ui.painter_at(rect);
        for (p, t, c) in labels {
            painter.text(p + egui::vec2(4.0, -4.0), egui::Align2::LEFT_BOTTOM, t, egui::FontId::proportional(12.0), c);
        }
        if let Some(scene) = &self.scene {
            let seq = scene.seqs.get(self.seq).map(|s| format!("{}  frame {:.1}/{}", s.name, self.frame, (s.frames - 1).max(0))).unwrap_or_default();
            painter.text(rect.left_top() + egui::vec2(8.0, 8.0), egui::Align2::LEFT_TOP, format!("{}\n{seq}", scene.path), egui::FontId::proportional(13.0), Color32::from_white_alpha(200));
        } else {
            painter.text(rect.center(), egui::Align2::CENTER_CENTER, "No model loaded", egui::FontId::proportional(18.0), Color32::from_white_alpha(120));
        }
        let _ = Quat::IDENTITY;
    }
}

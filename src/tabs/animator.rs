//! Animator tab: frame-by-frame / pose-by-pose animation of a QC's skeleton.

use crate::anim::{self, AnimDoc, Ease, IkChain, Keyframe, Rig};
use crate::app::Shared;
use crate::formats::mdl::{euler_to_quat, quat_to_euler};
use crate::render::{self, Blend, DrawCall, FrameData, GpuVertex, OrbitCamera, RenderMode, TextureUpload};
use eframe::egui::{self, Color32, RichText};
use glam::{Affine3A, Quat, Vec3};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

enum Modal {
    None,
    SaveAs(String),
    OpenList(Vec<PathBuf>),
    QcMissing { doc: AnimDoc, path: PathBuf },
    RigMismatch { doc: AnimDoc, diffs: Vec<String>, path: PathBuf },
    ConfirmDiscard(Box<PendingAction>),
}

/// Rotation that makes the root -> head direction point up (+Z).
fn stand_up_rotation(rig: &Rig) -> Quat {
    let w = rig.world(&rig.bind);
    let root = rig.bones.iter().position(|b| b.parent < 0).unwrap_or(0);
    let head = rig.bones.iter().position(|b| b.name.to_ascii_lowercase().contains("head")).or_else(|| {
        let r: Vec3 = w[root].translation.into();
        (0..w.len()).max_by(|&a, &b| (Vec3::from(w[a].translation) - r).length().total_cmp(&(Vec3::from(w[b].translation) - r).length()))
    });
    let Some(h) = head else { return Quat::IDENTITY };
    let v = (Vec3::from(w[h].translation) - Vec3::from(w[root].translation)).normalize_or_zero();
    if v == Vec3::ZERO || v.dot(Vec3::Z) > 0.7 {
        return Quat::IDENTITY;
    }
    Quat::from_rotation_arc(v, Vec3::Z)
}

fn rotated_bounds(q: Quat, mn: Vec3, mx: Vec3) -> (Vec3, Vec3) {
    let mut a = Vec3::splat(f32::MAX);
    let mut b = Vec3::splat(f32::MIN);
    for i in 0..8 {
        let c = Vec3::new(if i & 1 != 0 { mx.x } else { mn.x }, if i & 2 != 0 { mx.y } else { mn.y }, if i & 4 != 0 { mx.z } else { mn.z });
        let r = q * c;
        a = a.min(r);
        b = b.max(r);
    }
    (a, b)
}

enum PendingAction {
    NewAnim,
    OpenQc(PathBuf),
    OpenAnim(PathBuf),
}

pub struct AnimatorTab {
    rig: Option<Rig>,
    doc: Option<AnimDoc>,
    doc_path: Option<PathBuf>,
    dirty: bool,
    materials: HashMap<String, (Option<String>, bool, Option<f32>)>,
    uploads: Vec<TextureUpload>,
    camera: OrbitCamera,
    frame: f32,
    playing: bool,
    last_tick: Option<Instant>,
    sel_bone: Option<usize>,
    bone_filter: String,
    chains: Vec<IkChain>,
    easy_rig: bool,
    show_mesh: bool,
    show_bones: bool,
    bone_names: bool,
    mode: RenderMode,
    clipboard: Option<Vec<(Vec3, Quat)>>,
    undo: Vec<std::collections::BTreeMap<u32, Keyframe>>,
    redo: Vec<std::collections::BTreeMap<u32, Keyframe>>,
    in_gesture: bool,
    /// a left drag that started on a joint: it moves that bone instead of orbiting
    bone_drag: bool,
    fade_from: u32,
    fade_to: u32,
    fade_ease: Ease,
    modal: Modal,
    status: String,
    materials_with_fs: bool,
    /// display-only rotation (rig data is never changed by it)
    display_rot: Quat,
    auto_stand: bool,
}

impl Default for AnimatorTab {
    fn default() -> Self {
        AnimatorTab {
            rig: None,
            doc: None,
            doc_path: None,
            dirty: false,
            materials: HashMap::new(),
            uploads: Vec::new(),
            camera: OrbitCamera { yaw: 0.0, ..OrbitCamera::default() },
            frame: 0.0,
            playing: false,
            last_tick: None,
            sel_bone: None,
            bone_filter: String::new(),
            chains: Vec::new(),
            easy_rig: false,
            show_mesh: true,
            show_bones: true,
            bone_names: false,
            mode: RenderMode::Textured,
            clipboard: None,
            undo: Vec::new(),
            redo: Vec::new(),
            in_gesture: false,
            bone_drag: false,
            fade_from: 0,
            fade_to: 10,
            fade_ease: Ease::Smooth,
            modal: Modal::None,
            status: String::new(),
            materials_with_fs: false,
            display_rot: Quat::IDENTITY,
            auto_stand: true,
        }
    }
}

impl AnimatorTab {
    pub fn load_qc(&mut self, path: &Path, sh: &mut Shared) -> bool {
        match Rig::load(path) {
            Ok(rig) => {
                for w in &rig.warnings {
                    sh.error(w.clone());
                }
                self.load_materials(&rig, sh);
                self.display_rot = if self.auto_stand { stand_up_rotation(&rig) } else { Quat::IDENTITY };
                // frame camera on the mesh
                let mut mn = Vec3::splat(f32::MAX);
                let mut mx = Vec3::splat(f32::MIN);
                for m in &rig.meshes {
                    for t in &m.tris {
                        for v in &t.verts {
                            mn = mn.min(v.pos);
                            mx = mx.max(v.pos);
                        }
                    }
                }
                if mn.x > mx.x {
                    let w = rig.world(&rig.bind);
                    for x in &w {
                        mn = mn.min(x.translation.into());
                        mx = mx.max(x.translation.into());
                    }
                }
                self.camera = OrbitCamera { yaw: 0.0, ..OrbitCamera::default() };
                let (dmn, dmx) = rotated_bounds(self.display_rot, mn, mx);
                self.camera.frame_bounds(dmn, dmx);
                self.chains.clear();
                self.easy_rig = false;
                self.sel_bone = None;
                self.doc = Some(AnimDoc::new(&rig));
                self.doc_path = None;
                self.dirty = false;
                self.undo.clear();
                self.redo.clear();
                self.frame = 0.0;
                self.status = format!("Loaded {} ({} bones, {} mesh file(s))", path.display(), rig.bones.len(), rig.meshes.len());
                self.rig = Some(rig);
                true
            }
            Err(e) => {
                sh.error(format!("Could not load QC: {e}"));
                false
            }
        }
    }

    pub fn easy_rig_now(&mut self) {
        if let Some(rig) = &self.rig {
            self.chains = anim::detect_chains(rig);
            self.easy_rig = !self.chains.is_empty();
        }
    }

    fn load_materials(&mut self, rig: &Rig, sh: &Shared) {
        self.materials.clear();
        self.uploads.clear();
        self.materials_with_fs = sh.fs.is_some();
        let Some(fs) = &sh.fs else { return };
        let mut names: Vec<String> = rig.meshes.iter().flat_map(|m| m.tris.iter().map(|t| t.material.clone())).collect();
        names.sort();
        names.dedup();
        for name in names {
            let clean = name.trim_end_matches(".bmp").trim_end_matches(".tga").trim_end_matches(".vmt").to_string();
            let mut entry = (None, false, None);
            if let Some(vmt) = fs.find_material(&rig.cdmaterials, &clean) {
                if let Some(kv) = crate::scene::load_vmt(fs, &vmt, 0) {
                    entry.1 = kv.get_str("$translucent").map(|v| v.trim() == "1").unwrap_or(false);
                    if kv.get_str("$alphatest").map(|v| v.trim() == "1").unwrap_or(false) {
                        entry.2 = Some(0.5);
                    }
                    if let Some(bt) = kv.get_str("$basetexture") {
                        let tp = format!("materials/{}.vtf", bt.replace('\\', "/").trim_end_matches(".vtf"));
                        if let Some(data) = fs.read(&tp) {
                            if let Ok(img) = crate::formats::vtf::decode(&data, 2048) {
                                self.uploads.push(TextureUpload { key: tp.clone(), width: img.width, height: img.height, rgba: Arc::new(img.pixels), clamp: false });
                                entry.0 = Some(tp);
                            }
                        }
                    }
                }
            }
            self.materials.insert(name, entry);
        }
    }

    fn push_textures(&self, sh: &Shared) {
        let Ok(mut r) = sh.renderer.lock() else { return };
        let Some(r) = r.as_mut() else { return };
        for up in &self.uploads {
            if !r.has_texture(&up.key) {
                r.queue_upload(TextureUpload { key: up.key.clone(), width: up.width, height: up.height, rgba: up.rgba.clone(), clamp: up.clamp });
            }
        }
    }

    fn current_frame(&self) -> u32 {
        self.frame.round().max(0.0) as u32
    }

    fn begin_edit(&mut self) {
        if !self.in_gesture {
            if let Some(d) = &self.doc {
                self.undo.push(d.keys.clone());
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
                self.redo.clear();
            }
            self.in_gesture = true;
        }
    }

    /// Pose at the current frame, ready to be edited (creates the key if missing).
    fn edit_pose(&mut self) -> Option<&mut Vec<(Vec3, Quat)>> {
        let f = self.current_frame();
        let rig = self.rig.as_ref()?;
        let doc = self.doc.as_mut()?;
        if !doc.keys.contains_key(&f) {
            let pose = doc.sample(f as f32, &rig.bind);
            let ease = doc.keys.range(..f).next_back().map(|(_, k)| k.ease).unwrap_or(Ease::Smooth);
            doc.keys.insert(f, Keyframe { pose, ease });
        }
        self.dirty = true;
        doc.keys.get_mut(&f).map(|k| &mut k.pose)
    }

    fn animations_dir(sh: &Shared) -> Option<PathBuf> {
        sh.session.as_ref().map(|s| s.animations_dir())
    }

    fn save(&mut self, sh: &mut Shared, name: Option<String>) {
        let (Some(rig), Some(doc)) = (&self.rig, &mut self.doc) else { return };
        let Some(dir) = Self::animations_dir(sh) else { return };
        let _ = sh.files.ensure();
        let _ = std::fs::create_dir_all(&dir);
        let name = name.or_else(|| doc.name.clone()).unwrap_or_else(|| anim::next_auto_name(&dir, &rig.qc_name));
        let clean: String = name.chars().map(|c| if matches!(c, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
        doc.name = Some(clean.clone());
        let path = dir.join(format!("{clean}.txt"));
        match std::fs::write(&path, doc.to_text()) {
            Ok(()) => {
                self.dirty = false;
                self.doc_path = Some(path.clone());
                sh.notify(format!("Saved {}", path.display()));
            }
            Err(e) => sh.error(format!("Could not save animation: {e}")),
        }
    }

    fn export(&mut self, sh: &mut Shared, dmx: bool) {
        let (Some(rig), Some(doc)) = (&self.rig, &self.doc) else { return };
        let ext = if dmx { "dmx" } else { "smd" };
        let default = format!("{}.{ext}", doc.name.clone().unwrap_or_else(|| format!("{}_animation", rig.qc_name)));
        let dlg = rfd::FileDialog::new().set_directory(rig.qc_path.parent().unwrap_or(Path::new("."))).set_file_name(&default).add_filter(if dmx { "DMX animation" } else { "SMD animation" }, &[ext]);
        if let Some(p) = dlg.save_file() {
            let text = if dmx { doc.export_dmx(rig) } else { doc.export_smd(rig) };
            match std::fs::write(&p, text) {
                Ok(()) => sh.notify(format!("Exported {}", p.display())),
                Err(e) => sh.error(format!("Export failed: {e}")),
            }
        }
    }

    fn open_anim_file(&mut self, path: &Path, sh: &mut Shared) {
        let doc = match std::fs::read_to_string(path).map_err(|e| e.to_string()).and_then(|t| AnimDoc::from_text(&t)) {
            Ok(d) => d,
            Err(e) => {
                sh.error(format!("Could not open {}: {e}", path.display()));
                return;
            }
        };
        if !doc.qc_path.is_file() {
            self.modal = Modal::QcMissing { doc, path: path.to_path_buf() };
            return;
        }
        self.finish_open(doc, path.to_path_buf(), sh);
    }

    fn finish_open(&mut self, doc: AnimDoc, path: PathBuf, sh: &mut Shared) {
        let same_qc = self.rig.as_ref().map(|r| r.qc_path == doc.qc_path).unwrap_or(false);
        if !same_qc && !self.load_qc(&doc.qc_path.clone(), sh) {
            return;
        }
        let Some(rig) = &self.rig else { return };
        let diffs = doc.rig_differences(rig);
        if !diffs.is_empty() {
            self.modal = Modal::RigMismatch { doc, diffs, path };
            return;
        }
        self.apply_doc(doc, path);
    }

    fn apply_doc(&mut self, mut doc: AnimDoc, path: PathBuf) {
        if let Some(rig) = &self.rig {
            // map bones by name in case of a (confirmed) mismatch
            if doc.rig_differences(rig).is_empty() {
                doc.bones = rig.bone_signature();
            } else {
                let names: Vec<String> = doc.bones.iter().map(|b| b.0.clone()).collect();
                for k in doc.keys.values_mut() {
                    let old = k.pose.clone();
                    k.pose = rig.bones.iter().enumerate().map(|(i, b)| names.iter().position(|n| n.eq_ignore_ascii_case(&b.name)).and_then(|j| old.get(j).copied()).unwrap_or(rig.bind[i])).collect();
                }
                doc.bones = rig.bone_signature();
            }
            doc.qc_path = rig.qc_path.clone();
        }
        self.doc = Some(doc);
        self.doc_path = Some(path);
        self.dirty = false;
        self.undo.clear();
        self.redo.clear();
        self.frame = 0.0;
    }

    fn run_or_confirm(&mut self, action: PendingAction, sh: &mut Shared) {
        if self.dirty {
            self.modal = Modal::ConfirmDiscard(Box::new(action));
        } else {
            self.run_action(action, sh);
        }
    }

    fn run_action(&mut self, action: PendingAction, sh: &mut Shared) {
        match action {
            PendingAction::NewAnim => {
                if let Some(rig) = &self.rig {
                    self.doc = Some(AnimDoc::new(rig));
                    self.doc_path = None;
                    self.dirty = false;
                    self.undo.clear();
                    self.frame = 0.0;
                }
            }
            PendingAction::OpenQc(p) => {
                self.load_qc(&p, sh);
            }
            PendingAction::OpenAnim(p) => self.open_anim_file(&p, sh),
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        if self.rig.is_some() && !self.materials_with_fs && sh.fs.is_some() {
            // game finished mounting after the QC was opened: retry materials
            if let Some(rig) = self.rig.take() {
                self.load_materials(&rig, sh);
                self.rig = Some(rig);
            }
        }
        self.push_textures(sh);
        if !ui.input(|i| i.pointer.any_down()) {
            self.in_gesture = false;
        }
        self.shortcuts(ui, sh);
        self.toolbar(ui, sh);
        ui.separator();
        if self.rig.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space(60.0);
                ui.heading("Animator");
                ui.label("Open the .QC of a decompiled (or your own) model to start. Its reference SMDs, skeleton and textures are loaded automatically.");
                ui.add_space(8.0);
                if ui.button("Open QC...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().add_filter("QC", &["qc"]).pick_file() {
                        self.load_qc(&p, sh);
                    }
                }
                if ui.button("Open saved animation...").clicked() {
                    self.open_list(sh);
                }
            });
            self.modals(ui.ctx(), sh);
            return;
        }
        egui::Panel::left("anim_left").resizable(true).default_size(290.0).frame(egui::Frame::new().inner_margin(4.0)).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("anim_left_scroll").show(ui, |ui| self.left_panel(ui));
        });
        egui::Panel::bottom("anim_timeline").resizable(true).default_size(150.0).frame(egui::Frame::new().inner_margin(4.0)).show(ui, |ui| self.timeline(ui));
        egui::CentralPanel::default().frame(egui::Frame::new()).show(ui, |ui| self.viewport(ui, sh));
        self.modals(ui.ctx(), sh);
    }

    fn open_list(&mut self, sh: &Shared) {
        let mut files: Vec<PathBuf> = Self::animations_dir(sh)
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e.eq_ignore_ascii_case("txt")).unwrap_or(false)).collect())
            .unwrap_or_default();
        files.sort();
        self.modal = Modal::OpenList(files);
    }

    fn shortcuts(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        if self.rig.is_none() || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (ctrl, s, z, y, k, space, del) = ui.input(|i| (i.modifiers.ctrl, i.key_pressed(egui::Key::S), i.key_pressed(egui::Key::Z), i.key_pressed(egui::Key::Y), i.key_pressed(egui::Key::K), i.key_pressed(egui::Key::Space), i.key_pressed(egui::Key::Delete)));
        if ctrl && s {
            self.save(sh, None);
        }
        if ctrl && z {
            self.undo_step(true);
        }
        if ctrl && y {
            self.undo_step(false);
        }
        if k && !ctrl {
            self.begin_edit();
            let _ = self.edit_pose();
        }
        if space {
            self.playing = !self.playing;
        }
        if del {
            self.delete_key();
        }
    }

    fn undo_step(&mut self, undo: bool) {
        let Some(doc) = &mut self.doc else { return };
        let (from, to) = if undo { (&mut self.undo, &mut self.redo) } else { (&mut self.redo, &mut self.undo) };
        if let Some(state) = from.pop() {
            to.push(std::mem::replace(&mut doc.keys, state));
            self.dirty = true;
        }
    }

    fn delete_key(&mut self) {
        let f = self.current_frame();
        if self.doc.as_ref().map(|d| d.keys.contains_key(&f) && d.keys.len() > 1).unwrap_or(false) {
            self.begin_edit();
            self.in_gesture = false;
            if let Some(d) = &mut self.doc {
                d.keys.remove(&f);
            }
            self.dirty = true;
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        ui.horizontal_wrapped(|ui| {
            if ui.button("Open QC...").clicked() {
                if let Some(p) = rfd::FileDialog::new().add_filter("QC", &["qc"]).pick_file() {
                    self.run_or_confirm(PendingAction::OpenQc(p), sh);
                }
            }
            let has = self.rig.is_some();
            if ui.add_enabled(has, egui::Button::new("New animation")).clicked() {
                self.run_or_confirm(PendingAction::NewAnim, sh);
            }
            if ui.button("Open animation...").clicked() {
                self.open_list(sh);
            }
            if ui.add_enabled(has, egui::Button::new("Save")).on_hover_text("Ctrl+S. Unnamed animations are saved as <qc>_animation_N in the session's animations folder").clicked() {
                self.save(sh, None);
            }
            if ui.add_enabled(has, egui::Button::new("Save as...")).clicked() {
                let current = self.doc.as_ref().and_then(|d| d.name.clone()).unwrap_or_default();
                self.modal = Modal::SaveAs(current);
            }
            ui.separator();
            if ui.add_enabled(has, egui::Button::new("Export SMD...")).clicked() {
                self.export(sh, false);
            }
            if ui.add_enabled(has, egui::Button::new("Export DMX...")).clicked() {
                self.export(sh, true);
            }
            ui.separator();
            if let (Some(rig), Some(doc)) = (&self.rig, &self.doc) {
                ui.label(RichText::new(format!("{}{}", doc.display_name(), if self.dirty { " *" } else { "" })).strong().color(sh.theme.accent));
                ui.label(RichText::new(format!("QC: {}", rig.qc_path.display())).weak());
            }
        });
        if !self.status.is_empty() {
            ui.label(RichText::new(&self.status).small().weak());
        }
    }

    fn left_panel(&mut self, ui: &mut egui::Ui) {
        let Some(rig) = &self.rig else { return };
        ui.label(RichText::new("Display").strong());
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.show_mesh, "Mesh");
            ui.checkbox(&mut self.show_bones, "Bones");
            ui.checkbox(&mut self.bone_names, "Names");
            ui.selectable_value(&mut self.mode, RenderMode::Textured, "Textured");
            ui.selectable_value(&mut self.mode, RenderMode::Smooth, "Shaded");
            ui.selectable_value(&mut self.mode, RenderMode::Wireframe, "Wire");
        });
        ui.horizontal(|ui| {
            if ui.checkbox(&mut self.auto_stand, "Stand the model up").on_hover_text("Display only: some rigs (e.g. TF2 characters) are authored lying down. Exported animations are not affected.").changed() {
                self.display_rot = if self.auto_stand { stand_up_rotation(rig) } else { Quat::IDENTITY };
            }
        });
        ui.separator();
        // ---- easy rig
        ui.label(RichText::new("Easy rig (IK)").strong());
        if self.chains.is_empty() {
            if ui.button("Create easy rig").on_hover_text("Finds arms and legs by bone name so that moving a hand or foot moves the whole limb").clicked() {
                self.chains = anim::detect_chains(rig);
                self.easy_rig = !self.chains.is_empty();
                self.status = if self.chains.is_empty() { "No hand/foot chains found from the bone names. Add one manually below.".into() } else { format!("Easy rig: {} chain(s) found", self.chains.len()) };
            }
        } else {
            ui.checkbox(&mut self.easy_rig, "Easy rig enabled");
            let mut remove = None;
            for (i, c) in self.chains.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut c.enabled, &c.name);
                    if ui.small_button("x").clicked() {
                        remove = Some(i);
                    }
                });
                ui.label(RichText::new(c.bones.iter().map(|&b| rig.bones[b].name.clone()).collect::<Vec<_>>().join(" > ")).small().weak());
            }
            if let Some(i) = remove {
                self.chains.remove(i);
            }
        }
        if let Some(b) = self.sel_bone {
            if ui.button(format!("Add IK chain ending at '{}'", rig.bones[b].name)).on_hover_text("Uses the selected bone and its two parents").clicked() {
                let mut chain = vec![b];
                let mut p = rig.bones[b].parent;
                while p >= 0 && chain.len() < 3 {
                    chain.push(p as usize);
                    p = rig.bones[p as usize].parent;
                }
                chain.reverse();
                if chain.len() >= 2 {
                    self.chains.push(IkChain { name: format!("Custom ({})", rig.bones[b].name), bones: chain, enabled: true });
                    self.easy_rig = true;
                }
            }
        }
        ui.label(RichText::new("Drag a joint to pose it: an IK hand/foot (green) moves the whole limb, any other bone rotates. Drag empty space to orbit. Ctrl+drag works on the selected bone from anywhere.").small().weak());
        ui.separator();

        // ---- selected bone transform
        if let Some(b) = self.sel_bone {
            ui.label(RichText::new(format!("Bone: {}", rig.bones[b].name)).strong());
            let f = self.current_frame();
            let sampled = self.doc.as_ref().map(|d| d.sample(f as f32, &rig.bind)).unwrap_or_default();
            if let Some(&(p, q)) = sampled.get(b) {
                let e = quat_to_euler(q);
                let mut pos = [p.x, p.y, p.z];
                let mut rot = [e.x.to_degrees(), e.y.to_degrees(), e.z.to_degrees()];
                let mut changed = false;
                egui::Grid::new("bone_xf").num_columns(4).show(ui, |ui| {
                    ui.label("Position");
                    for v in &mut pos {
                        changed |= ui.add(egui::DragValue::new(v).speed(0.05).max_decimals(3)).changed();
                    }
                    ui.end_row();
                    ui.label("Rotation °");
                    for v in &mut rot {
                        changed |= ui.add(egui::DragValue::new(v).speed(0.5).max_decimals(2)).changed();
                    }
                    ui.end_row();
                });
                if changed {
                    self.begin_edit();
                    if let Some(pose) = self.edit_pose() {
                        pose[b] = (Vec3::from_array(pos), euler_to_quat(Vec3::new(rot[0].to_radians(), rot[1].to_radians(), rot[2].to_radians())));
                    }
                }
                ui.horizontal(|ui| {
                    if ui.button("Reset bone").clicked() {
                        let bind = self.rig.as_ref().map(|r| r.bind[b]);
                        self.begin_edit();
                        self.in_gesture = false;
                        if let (Some(bind), Some(pose)) = (bind, self.edit_pose()) {
                            pose[b] = bind;
                        }
                    }
                    if ui.button("Reset whole pose").clicked() {
                        let bind = self.rig.as_ref().map(|r| r.bind.clone());
                        self.begin_edit();
                        self.in_gesture = false;
                        if let (Some(bind), Some(pose)) = (bind, self.edit_pose()) {
                            *pose = bind;
                        }
                    }
                });
            }
            ui.separator();
        }

        // ---- bone list
        let Some(rig) = &self.rig else { return };
        ui.label(RichText::new(format!("Bones ({})", rig.bones.len())).strong());
        ui.add(egui::TextEdit::singleline(&mut self.bone_filter).hint_text("filter"));
        let f = self.bone_filter.to_ascii_lowercase();
        let depth = |mut i: usize| {
            let mut d = 0;
            while rig.bones[i].parent >= 0 && d < 40 {
                i = rig.bones[i].parent as usize;
                d += 1;
            }
            d
        };
        for (i, b) in rig.bones.iter().enumerate() {
            if !f.is_empty() && !b.name.to_ascii_lowercase().contains(&f) {
                continue;
            }
            let indent = if f.is_empty() { "  ".repeat(depth(i).min(12)) } else { String::new() };
            let is_eff = self.chains.iter().any(|c| c.enabled && c.bones.last() == Some(&i));
            let label = format!("{indent}{}{}", b.name, if is_eff { "  [IK]" } else { "" });
            if ui.selectable_label(self.sel_bone == Some(i), label).clicked() {
                self.sel_bone = Some(i);
            }
        }
    }

    fn timeline(&mut self, ui: &mut egui::Ui) {
        let Some(rig) = &self.rig else { return };
        let Some(doc) = &mut self.doc else { return };
        let mut changed_doc = false;
        ui.horizontal_wrapped(|ui| {
            if ui.button(if self.playing { "Pause" } else { "Play" }).clicked() {
                self.playing = !self.playing;
            }
            if ui.button("|<").clicked() {
                self.frame = 0.0;
            }
            if ui.button("< key").clicked() {
                let f = self.frame.round() as u32;
                if let Some((k, _)) = doc.keys.range(..f).next_back() {
                    self.frame = *k as f32;
                }
            }
            if ui.button("<").clicked() {
                self.frame = (self.frame.round() - 1.0).max(0.0);
            }
            if ui.button(">").clicked() {
                self.frame = (self.frame.round() + 1.0).min((doc.frames.max(1) - 1) as f32);
            }
            if ui.button("key >").clicked() {
                let f = self.frame.round() as u32;
                if let Some((k, _)) = doc.keys.range(f + 1..).next() {
                    self.frame = *k as f32;
                }
            }
            ui.label(format!("Frame {:.0} / {}", self.frame.round(), doc.frames.max(1) - 1));
            ui.separator();
            ui.label("FPS");
            changed_doc |= ui.add(egui::DragValue::new(&mut doc.fps).range(1.0..=240.0).speed(0.2)).changed();
            ui.label("Frames");
            changed_doc |= ui.add(egui::DragValue::new(&mut doc.frames).range(1..=100_000)).changed();
            changed_doc |= ui.checkbox(&mut doc.looping, "Loop").changed();
            ui.label("Smoothing");
            changed_doc |= ui.add(egui::Slider::new(&mut doc.smoothing, 0.0..=1.0)).on_hover_text("Blends every transition toward a smooth spline through the keys").changed();
        });
        // track with key markers
        let max = (doc.frames.max(1) - 1) as f32;
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 4.0, Color32::from_black_alpha(150));
        let x_of = |f: f32| rect.left() + 8.0 + (rect.width() - 16.0) * if max > 0.0 { f / max } else { 0.0 };
        let step = if max > 300.0 { 50 } else if max > 120.0 { 10 } else { 5 };
        for f in (0..=max as u32).step_by(step) {
            let x = x_of(f as f32);
            painter.line_segment([egui::pos2(x, rect.bottom() - 8.0), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, Color32::GRAY));
            painter.text(egui::pos2(x + 2.0, rect.bottom() - 8.0), egui::Align2::LEFT_BOTTOM, f.to_string(), egui::FontId::proportional(9.0), Color32::GRAY);
        }
        for (k, key) in &doc.keys {
            let x = x_of(*k as f32);
            let c = match key.ease {
                Ease::Hold => Color32::from_rgb(200, 120, 120),
                Ease::Linear => Color32::from_rgb(200, 200, 200),
                _ => Color32::from_rgb(232, 200, 90),
            };
            painter.add(egui::Shape::convex_polygon(vec![egui::pos2(x, rect.top() + 4.0), egui::pos2(x + 5.0, rect.top() + 11.0), egui::pos2(x, rect.top() + 18.0), egui::pos2(x - 5.0, rect.top() + 11.0)], c, egui::Stroke::NONE));
        }
        let cx = x_of(self.frame);
        painter.line_segment([egui::pos2(cx, rect.top()), egui::pos2(cx, rect.bottom())], egui::Stroke::new(2.0, Color32::from_rgb(255, 90, 90)));
        if resp.dragged() || resp.clicked() {
            if let Some(p) = resp.interact_pointer_pos() {
                let t = ((p.x - rect.left() - 8.0) / (rect.width() - 16.0)).clamp(0.0, 1.0);
                self.frame = (t * max).round();
                self.playing = false;
            }
        }
        // key operations
        let f = self.frame.round() as u32;
        let mut new_key = false;
        let mut del = false;
        let mut copy = false;
        let mut paste = false;
        ui.horizontal_wrapped(|ui| {
            new_key = ui.button("Set key (K)").clicked();
            del = ui.add_enabled(doc.keys.contains_key(&f) && doc.keys.len() > 1, egui::Button::new("Delete key")).clicked();
            copy = ui.button("Copy pose").clicked();
            paste = ui.add_enabled(self.clipboard.is_some(), egui::Button::new("Paste pose")).clicked();
            ui.separator();
            // ease of the segment that starts at or before this frame
            let seg_key = doc.keys.range(..=f).next_back().map(|(k, _)| *k);
            if let Some(sk) = seg_key {
                let cur = doc.keys[&sk].ease;
                ui.label(format!("Transition from key {sk}:"));
                egui::ComboBox::from_id_salt("ease").selected_text(cur.label()).show_ui(ui, |ui| {
                    for e in Ease::ALL {
                        if ui.selectable_label(cur == e, e.label()).clicked() {
                            if let Some(k) = doc.keys.get_mut(&sk) {
                                k.ease = e;
                            }
                            changed_doc = true;
                        }
                    }
                });
            }
            ui.separator();
            ui.label("Fade frame");
            ui.add(egui::DragValue::new(&mut self.fade_from).range(0..=doc.frames.max(1) - 1));
            ui.label("into frame");
            ui.add(egui::DragValue::new(&mut self.fade_to).range(0..=doc.frames.max(1) - 1));
            egui::ComboBox::from_id_salt("fade_ease").selected_text(self.fade_ease.label()).show_ui(ui, |ui| {
                for e in [Ease::Smooth, Ease::Linear, Ease::EaseIn, Ease::EaseOut, Ease::Spline] {
                    ui.selectable_value(&mut self.fade_ease, e, e.label());
                }
            });
            if ui.button("Apply fade").on_hover_text("Replaces the keys between the two frames with a smooth blend from the first pose into the second").clicked() && self.fade_to > self.fade_from {
                self.undo.push(doc.keys.clone());
                doc.crossfade(self.fade_from, self.fade_to, self.fade_ease, &rig.bind);
                changed_doc = true;
            }
        });
        if changed_doc {
            self.dirty = true;
        }
        if copy {
            self.clipboard = Some(doc.sample(f as f32, &rig.bind));
        }
        if new_key {
            self.begin_edit();
            self.in_gesture = false;
            let _ = self.edit_pose();
        }
        if del {
            self.delete_key();
        }
        if paste {
            if let Some(c) = self.clipboard.clone() {
                self.begin_edit();
                self.in_gesture = false;
                if let Some(p) = self.edit_pose() {
                    *p = c;
                }
            }
        }
    }

    fn viewport(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        let rect = ui.available_rect_before_wrap();
        let resp = ui.allocate_rect(rect, egui::Sense::click_and_drag());
        let ctrl = ui.input(|i| i.modifiers.ctrl);
        // A left drag that starts on a joint grabs that bone; anywhere else it orbits.
        // (Ctrl + left drag manipulates the selected bone from anywhere.)
        if resp.drag_started_by(egui::PointerButton::Primary) {
            self.bone_drag = false;
            let shift = ui.input(|i| i.modifiers.shift);
            if !ctrl && !shift && self.show_bones {
                let origin = ui.input(|i| i.pointer.press_origin());
                if let (Some(pos), Some(rig), Some(doc)) = (origin, &self.rig, &self.doc) {
                    let pose = doc.sample(if self.playing { self.frame } else { self.frame.round() }, &rig.bind);
                    let world = rig.world(&pose);
                    let vp = self.camera.view_proj(rect.width() / rect.height().max(1.0)) * glam::Mat4::from_quat(self.display_rot);
                    if let Some(b) = pick_joint(&world, vp, rect, pos) {
                        self.sel_bone = Some(b);
                        self.bone_drag = true;
                        self.playing = false;
                    }
                }
            }
        }
        if !resp.dragged() {
            self.bone_drag = false;
        }
        let manip = (ctrl || self.bone_drag) && self.sel_bone.is_some() && resp.dragged_by(egui::PointerButton::Primary);
        if !manip {
            self.camera.handle_input(&resp, ui);
        }
        let (Some(rig), Some(doc)) = (&self.rig, &self.doc) else { return };
        // playback
        let now = Instant::now();
        let dt = self.last_tick.map(|t| now.duration_since(t).as_secs_f32()).unwrap_or(0.0).min(0.1);
        self.last_tick = Some(now);
        let max = (doc.frames.max(1) - 1) as f32;
        if self.playing {
            self.frame += dt * doc.fps;
            if self.frame > max {
                // the preview always loops; `Loop` controls how the last key blends back to the first
                self.frame = 0.0;
            }
            ui.ctx().request_repaint();
        }
        let pose = doc.sample(if self.playing { self.frame } else { self.frame.round() }, &rig.bind);
        let world = rig.world(&pose);
        let aspect = rect.width() / rect.height().max(1.0);
        let vp = self.camera.view_proj(aspect) * glam::Mat4::from_quat(self.display_rot);
        let bg = sh.theme.viewport_bg;
        let mut f = FrameData {
            view_proj: vp,
            clear: Some([bg.r() as f32 / 255.0, bg.g() as f32 / 255.0, bg.b() as f32 / 255.0, 1.0]),
            light_dir: Vec3::new(0.6, 0.4, 0.7),
            ambient: 0.35,
            mode: Some(self.mode),
            ..Default::default()
        };
        {
            // the ground stays level in display space
            let inv = self.display_rot.inverse();
            let mut g = Vec::new();
            render::grid(&mut g, 128.0, 8.0);
            for v in &mut g {
                v.pos = (inv * Vec3::from_array(v.pos)).to_array();
            }
            f.lines.extend(g);
        }
        if self.show_mesh {
            for m in &rig.meshes {
                let skin: Vec<Affine3A> = m.bone_map.iter().enumerate().map(|(i, rb)| rb.and_then(|rb| world.get(rb)).copied().unwrap_or(Affine3A::IDENTITY) * m.inv_bind.get(i).copied().unwrap_or(Affine3A::IDENTITY)).collect();
                let mut cur_mat: Option<&str> = None;
                let mut first = f.indices.len() as u32;
                let flush = |f: &mut FrameData, mat: Option<&str>, first: u32| {
                    let count = f.indices.len() as u32 - first;
                    if count == 0 {
                        return;
                    }
                    let info = mat.and_then(|m| self.materials.get(m));
                    let texture = info.and_then(|i| i.0.clone());
                    let blend = if info.map(|i| i.1).unwrap_or(false) { Blend::Translucent } else if info.and_then(|i| i.2).is_some() { Blend::AlphaTest } else { Blend::Opaque };
                    f.draws.push(DrawCall { first, count, texture, blend, alpha_ref: info.and_then(|i| i.2).unwrap_or(0.0), cull: false, tint: [1.0; 4], selfillum: false });
                };
                for t in &m.tris {
                    if cur_mat != Some(t.material.as_str()) {
                        flush(&mut f, cur_mat, first);
                        cur_mat = Some(t.material.as_str());
                        first = f.indices.len() as u32;
                    }
                    for v in &t.verts {
                        let mut p = Vec3::ZERO;
                        let mut n = Vec3::ZERO;
                        let mut ws = 0.0f32;
                        if v.links.is_empty() {
                            if let Some(s) = skin.get(v.bone.max(0) as usize) {
                                p = s.transform_point3(v.pos);
                                n = s.transform_vector3(v.normal);
                                ws = 1.0;
                            }
                        } else {
                            for (b, w) in &v.links {
                                if let Some(s) = skin.get((*b).max(0) as usize) {
                                    p += s.transform_point3(v.pos) * *w;
                                    n += s.transform_vector3(v.normal) * *w;
                                    ws += *w;
                                }
                            }
                        }
                        if ws > 0.0 && (ws - 1.0).abs() > 1e-3 {
                            p /= ws;
                        }
                        if ws == 0.0 {
                            p = v.pos;
                            n = v.normal;
                        }
                        f.indices.push(f.vertices.len() as u32);
                        f.vertices.push(GpuVertex { pos: p.to_array(), normal: n.normalize_or(Vec3::Z).to_array(), uv: [v.uv[0], 1.0 - v.uv[1]] });
                    }
                }
                flush(&mut f, cur_mat, first);
            }
        }
        let mut labels = Vec::new();
        let ik_effectors: Vec<usize> = if self.easy_rig { self.chains.iter().filter(|c| c.enabled).filter_map(|c| c.bones.last().copied()).collect() } else { Vec::new() };
        let sel_chain: Vec<usize> = self.sel_bone.and_then(|b| self.chains.iter().find(|c| c.enabled && self.easy_rig && c.bones.last() == Some(&b))).map(|c| c.bones.clone()).unwrap_or_default();
        if self.show_bones {
            for (i, b) in rig.bones.iter().enumerate() {
                let p: Vec3 = world[i].translation.into();
                let sel = self.sel_bone == Some(i);
                let in_chain = sel_chain.contains(&i);
                if b.parent >= 0 {
                    let pp: Vec3 = world[b.parent as usize].translation.into();
                    let c = if sel || in_chain { [1.0, 0.95, 0.2, 1.0] } else { [0.9, 0.75, 0.25, 0.85] };
                    render::line(&mut f.overlay_lines, pp, p, c);
                }
                // joint marker
                let s = (self.camera.distance * 0.006).max(0.15);
                let c = if sel { [1.0, 0.3, 0.3, 1.0] } else if ik_effectors.contains(&i) { [0.3, 1.0, 0.5, 1.0] } else { [1.0, 1.0, 1.0, 0.7] };
                for ax in [Vec3::X, Vec3::Y, Vec3::Z] {
                    render::line(&mut f.overlay_lines, p - ax * s, p + ax * s, c);
                }
                if sel {
                    render::axes(&mut f.overlay_lines, world[i], s * 5.0);
                }
                if self.bone_names || sel {
                    if let Some(sp) = render::project(vp, rect, p) {
                        labels.push((sp, b.name.clone()));
                    }
                }
            }
        }
        // selection by click
        if resp.clicked() && !ctrl {
            if let Some(b) = resp.interact_pointer_pos().and_then(|pos| pick_joint(&world, vp, rect, pos)) {
                self.sel_bone = Some(b);
            }
        }
        render::paint_viewport(ui, rect, sh.renderer.clone(), f);
        let painter = ui.painter_at(rect);
        for (p, t) in labels {
            painter.text(p + egui::vec2(5.0, -3.0), egui::Align2::LEFT_BOTTOM, t, egui::FontId::proportional(12.0), Color32::from_rgb(240, 220, 130));
        }
        painter.text(rect.left_top() + egui::vec2(8.0, 8.0), egui::Align2::LEFT_TOP, format!("{}  frame {:.0}", doc.display_name(), self.frame.round()), egui::FontId::proportional(13.0), Color32::from_white_alpha(200));

        // manipulation
        if manip {
            let d = resp.drag_delta();
            if d != egui::Vec2::ZERO {
                let b = self.sel_bone.unwrap_or(0);
                let new_pose = {
                    let (Some(rig), Some(doc)) = (&self.rig, &self.doc) else { return };
                    let mut pose = doc.sample(self.current_frame() as f32, &rig.bind);
                    let world = rig.world(&pose);
                    let eye = self.camera.eye();
                    let fwd = (self.camera.target - eye).normalize_or(Vec3::X);
                    let inv = self.display_rot.inverse();
                    let right = inv * fwd.cross(Vec3::Z).normalize_or(Vec3::Y);
                    let up = inv * fwd.cross(Vec3::Z).normalize_or(Vec3::Y).cross(fwd).normalize_or(Vec3::Z);
                    let px = 2.0 * self.camera.distance * (self.camera.fov.to_radians() * 0.5).tan() / rect.height().max(1.0);
                    let chain = self.chains.iter().find(|c| c.enabled && self.easy_rig && c.bones.last() == Some(&b)).map(|c| c.bones.clone());
                    match chain {
                        Some(chain) => {
                            let target: Vec3 = Vec3::from(world[b].translation) + right * d.x * px - up * d.y * px;
                            anim::solve_ik(rig, &mut pose, &chain, target);
                        }
                        None => {
                            let parent = rig.bones[b].parent;
                            let parent_world_rot = if parent >= 0 { world[parent as usize].to_scale_rotation_translation().1 } else { Quat::IDENTITY };
                            let world_rot = world[b].to_scale_rotation_translation().1;
                            let r = Quat::from_axis_angle(up, d.x * 0.01) * Quat::from_axis_angle(right, d.y * 0.01);
                            pose[b].1 = (parent_world_rot.inverse() * (r * world_rot).normalize()).normalize();
                        }
                    }
                    pose
                };
                self.begin_edit();
                if let Some(p) = self.edit_pose() {
                    *p = new_pose;
                }
            }
        }
    }
    fn modals(&mut self, ctx: &egui::Context, sh: &mut Shared) {
        let modal = std::mem::replace(&mut self.modal, Modal::None);
        let center = egui::Align2::CENTER_CENTER;
        self.modal = match modal {
            Modal::None => Modal::None,
            Modal::SaveAs(mut name) => {
                let mut keep = true;
                let mut save = false;
                egui::Window::new("Save animation as").collapsible(false).resizable(false).anchor(center, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                    ui.label("Name (saved into the session's animations folder):");
                    let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(280.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        save = true;
                    }
                    ui.horizontal(|ui| {
                        save |= ui.button("Save").clicked();
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                if save && !name.trim().is_empty() {
                    self.save(sh, Some(name.trim().to_string()));
                    Modal::None
                } else if keep {
                    Modal::SaveAs(name)
                } else {
                    Modal::None
                }
            }
            Modal::OpenList(files) => {
                let mut keep = true;
                let mut chosen: Option<PathBuf> = None;
                egui::Window::new("Open animation").collapsible(false).default_size([420.0, 360.0]).anchor(center, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                    if files.is_empty() {
                        ui.label("No saved animations in this session yet.");
                    }
                    egui::ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
                        for f in &files {
                            if ui.button(f.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()).clicked() {
                                chosen = Some(f.clone());
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Browse...").clicked() {
                            chosen = rfd::FileDialog::new().add_filter("Golden Stunstick animation", &["txt"]).pick_file();
                        }
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                if let Some(p) = chosen {
                    self.run_or_confirm(PendingAction::OpenAnim(p), sh);
                    std::mem::replace(&mut self.modal, Modal::None)
                } else if keep {
                    Modal::OpenList(files)
                } else {
                    Modal::None
                }
            }
            Modal::QcMissing { mut doc, path } => {
                let mut keep = true;
                let mut pick = false;
                egui::Window::new("QC not found").collapsible(false).resizable(false).anchor(center, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                    ui.label(format!("The QC this animation was made with no longer exists:\n{}", doc.qc_path.display()));
                    ui.label("Choose where the QC is now.");
                    ui.horizontal(|ui| {
                        pick = ui.button("Choose QC...").clicked();
                        if ui.button("Cancel").clicked() {
                            keep = false;
                        }
                    });
                });
                if pick {
                    match rfd::FileDialog::new().add_filter("QC", &["qc"]).pick_file() {
                        Some(q) => {
                            doc.qc_path = q;
                            self.finish_open(doc, path, sh);
                            std::mem::replace(&mut self.modal, Modal::None)
                        }
                        None => Modal::QcMissing { doc, path },
                    }
                } else if keep {
                    Modal::QcMissing { doc, path }
                } else {
                    Modal::None
                }
            }
            Modal::RigMismatch { doc, diffs, path } => {
                let mut choice = None;
                egui::Window::new("Skeleton mismatch").collapsible(false).anchor(center, egui::vec2(0.0, 0.0)).default_size([480.0, 320.0]).show(ctx, |ui| {
                    ui.colored_label(Color32::from_rgb(240, 200, 90), "The bones / rig of this model do not match the animation:");
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for d in &diffs {
                            ui.label(format!("- {d}"));
                        }
                    });
                    ui.label("Continue opens the animation anyway, matching bones by name (unmatched bones use the model's bind pose).");
                    ui.horizontal(|ui| {
                        if ui.button("Continue").clicked() {
                            choice = Some(true);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(false);
                        }
                    });
                });
                match choice {
                    Some(true) => {
                        self.apply_doc(doc, path);
                        Modal::None
                    }
                    Some(false) => Modal::None,
                    None => Modal::RigMismatch { doc, diffs, path },
                }
            }
            Modal::ConfirmDiscard(action) => {
                let mut choice = None;
                egui::Window::new("Unsaved changes").collapsible(false).resizable(false).anchor(center, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                    ui.label("The current animation has unsaved changes.");
                    ui.horizontal(|ui| {
                        if ui.button("Save first").clicked() {
                            choice = Some(0);
                        }
                        if ui.button("Discard").clicked() {
                            choice = Some(1);
                        }
                        if ui.button("Cancel").clicked() {
                            choice = Some(2);
                        }
                    });
                });
                match choice {
                    Some(0) => {
                        self.save(sh, None);
                        self.run_action(*action, sh);
                        std::mem::replace(&mut self.modal, Modal::None)
                    }
                    Some(1) => {
                        self.dirty = false;
                        self.run_action(*action, sh);
                        std::mem::replace(&mut self.modal, Modal::None)
                    }
                    Some(_) => Modal::None,
                    None => Modal::ConfirmDiscard(action),
                }
            }
        };
    }
}

/// The joint whose screen position is nearest `pos` (within 14 px), if any.
fn pick_joint(world: &[Affine3A], vp: glam::Mat4, rect: egui::Rect, pos: egui::Pos2) -> Option<usize> {
    let mut best = (14.0f32, None);
    for (i, w) in world.iter().enumerate() {
        if let Some(sp) = render::project(vp, rect, w.translation.into()) {
            let d = sp.distance(pos);
            if d < best.0 {
                best = (d, Some(i));
            }
        }
    }
    best.1
}

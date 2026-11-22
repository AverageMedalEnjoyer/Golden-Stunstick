//! Main window: Files / Settings menus, sessions and the three tabs.

pub mod files;
pub mod session;
pub mod theme;

use crate::gamefs::GameFs;
use crate::render::SharedRenderer;
use crate::tabs::{animator::AnimatorTab, compiler::CompilerTab, model_viewer::ModelViewerTab};
use eframe::egui::{self, Color32, RichText};
use files::FilesDir;
use session::Session;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use theme::{BackgroundCache, Theme};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tab {
    Animator,
    ModelViewer,
    Compiler,
}

/// State shared with every tab.
pub struct Shared {
    pub files: FilesDir,
    pub session: Option<Session>,
    pub fs: Option<Arc<GameFs>>,
    pub fs_status: String,
    pub renderer: SharedRenderer,
    pub theme: Theme,
    pub notices: Vec<(String, bool)>,
    /// `assets/circle.png`, uploaded once
    pub circle: Option<egui::TextureHandle>,
}

impl Shared {
    /// A `size`-pixel dot in `color` (from the embedded circle asset).
    pub fn dot(&self, color: Color32, size: f32) -> egui::Image<'static> {
        let id = self.circle.as_ref().map(|t| t.id()).unwrap_or_default();
        egui::Image::from_texture(egui::load::SizedTexture::new(id, egui::vec2(size, size))).tint(color)
    }
    pub fn notify(&mut self, msg: impl Into<String>) {
        self.notices.push((msg.into(), false));
    }
    pub fn error(&mut self, msg: impl Into<String>) {
        self.notices.push((msg.into(), true));
    }
}

struct NewSessionDialog {
    name: String,
    gameinfo: String,
    studiomdl: String,
    error: Option<String>,
}

pub struct GoldenStunstick {
    shared: Shared,
    tab: Tab,
    icon: Option<egui::TextureHandle>,
    new_session: Option<NewSessionDialog>,
    open_session: Option<Vec<String>>,
    error_popup: Option<String>,
    settings_open: bool,
    settings_page: u8,
    theme_draft: Option<Theme>,
    background: BackgroundCache,
    mounting: Option<Receiver<Result<GameFs, String>>>,
    viewer: ModelViewerTab,
    compiler: CompilerTab,
    animator: AnimatorTab,
    last_title: String,
    /// model requested on the command line, opened once the game is mounted
    pending_model: Option<String>,
    pending_seq: Option<String>,
}

impl GoldenStunstick {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let files = FilesDir::new();
        let theme = Theme::load(&files.settings_file());
        theme.apply(&cc.egui_ctx);
        let icon = image::load_from_memory(crate::ICON_PNG).ok().map(|img| {
            let img = img.thumbnail(64, 64).to_rgba8();
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            cc.egui_ctx.load_texture("gs_icon", ci, egui::TextureOptions::LINEAR)
        });
        let circle = image::load_from_memory(crate::CIRCLE_PNG).ok().map(|img| {
            let img = img.to_rgba8();
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            cc.egui_ctx.load_texture("gs_circle", ci, egui::TextureOptions::LINEAR)
        });
        let renderer: SharedRenderer = Arc::new(Mutex::new(None));
        GoldenStunstick {
            shared: Shared { files, session: None, fs: None, fs_status: String::new(), renderer, theme, notices: Vec::new(), circle },
            tab: Tab::ModelViewer,
            icon,
            new_session: None,
            open_session: None,
            error_popup: None,
            settings_open: false,
            settings_page: 0,
            theme_draft: None,
            background: BackgroundCache::new(),
            mounting: None,
            viewer: ModelViewerTab::default(),
            compiler: CompilerTab::default(),
            animator: AnimatorTab::default(),
            last_title: String::new(),
            pending_model: None,
            pending_seq: None,
        }
    }

    /// Command line: `--session <name>` opens a session, `--tab animator|viewer|compiler`
    /// picks the tab and `--model <models/...mdl>` opens a model in the viewer.
    pub fn apply_args(&mut self, args: &[String]) {
        let get = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
        if let Some(name) = get("--session") {
            let dir = self.shared.files.session_dir(&name);
            match Session::load(&name, &dir) {
                Ok(s) => self.start_session(s),
                Err(e) => self.error_popup = Some(e),
            }
        }
        match get("--tab").as_deref() {
            Some("animator") => self.tab = Tab::Animator,
            Some("compiler") => self.tab = Tab::Compiler,
            Some("viewer") => self.tab = Tab::ModelViewer,
            _ => {}
        }
        if let Some(qc) = get("--qc") {
            self.tab = Tab::Animator;
            if self.animator.load_qc(std::path::Path::new(&qc), &mut self.shared) && args.iter().any(|a| a == "--easy-rig") {
                self.animator.easy_rig_now();
            }
        }
        self.pending_model = get("--model");
        self.pending_seq = get("--seq");
        if let Some(p) = get("--viewer-page") {
            self.viewer.set_panel(&p);
        }
        if args.iter().any(|a| a == "--open-session-list") {
            self.open_session = Some(self.shared.files.list_sessions());
        }
    }

    fn start_session(&mut self, s: Session) {
        self.shared.session = Some(s.clone());
        self.shared.fs = None;
        self.shared.fs_status = "Mounting game content...".into();
        let (tx, rx) = std::sync::mpsc::channel();
        let gi = s.gameinfo.clone();
        std::thread::spawn(move || {
            let _ = tx.send(GameFs::mount(&gi, None));
        });
        self.mounting = Some(rx);
        self.viewer = ModelViewerTab::default();
        self.compiler = CompilerTab::default();
        self.animator = AnimatorTab::default();
        let _ = std::fs::create_dir_all(s.animations_dir());
        let _ = std::fs::create_dir_all(s.logs_dir());
    }

    fn poll_mount(&mut self) {
        let Some(rx) = &self.mounting else { return };
        if let Ok(res) = rx.try_recv() {
            self.mounting = None;
            match res {
                Ok(fs) => {
                    let n = fs.sources.len();
                    let bad = fs.report.iter().filter(|r| !r.ok).count();
                    self.shared.fs_status = format!("{} - {n} search paths mounted{}", fs.game_name, if bad > 0 { format!(", {bad} skipped") } else { String::new() });
                    self.shared.fs = Some(Arc::new(fs));
                    if let Some(m) = self.pending_model.take() {
                        self.viewer.request_model(&self.shared, &m, self.pending_seq.take());
                    }
                }
                Err(e) => {
                    self.shared.fs_status = format!("Mount failed: {e}");
                    self.error_popup = Some(format!("Could not mount gameinfo.txt:\n{e}"));
                }
            }
        }
    }

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            if let Some(icon) = &self.icon {
                ui.add(egui::Image::new(icon).fit_to_exact_size(egui::vec2(20.0, 20.0)));
            }
            ui.menu_button("Files", |ui| {
                if ui.button("New Session").clicked() {
                    self.new_session = Some(NewSessionDialog { name: String::new(), gameinfo: String::new(), studiomdl: String::new(), error: None });
                    ui.close();
                }
                if ui.button("Open Session").clicked() {
                    self.open_session = Some(self.shared.files.list_sessions());
                    ui.close();
                }
            });
            if ui.button("Settings").clicked() {
                self.settings_open = true;
                self.theme_draft = Some(self.shared.theme.clone());
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(RichText::new(format!("Golden Stunstick v{}", env!("CARGO_PKG_VERSION"))).weak());
            });
        });
    }

    fn session_header(&mut self, ui: &mut egui::Ui) {
        let Some(s) = &self.shared.session else { return };
        let accent = self.shared.theme.accent;
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&s.name).heading().color(accent));
            ui.separator();
            ui.label(RichText::new("gameinfo.txt:").weak());
            ui.label(s.gameinfo.display().to_string());
            ui.separator();
            ui.label(RichText::new("studiomdl.exe:").weak());
            match &s.studiomdl {
                Some(p) => ui.label(p.display().to_string()),
                None => ui.label(RichText::new("not set (needed for compiling only)").color(Color32::from_rgb(230, 150, 80))),
            };
        });
        ui.horizontal(|ui| {
            if self.mounting.is_some() {
                ui.spinner();
            }
            ui.label(RichText::new(&self.shared.fs_status).small().weak());
        });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            for (t, label) in [(Tab::Animator, "Animator"), (Tab::ModelViewer, "Model Viewer"), (Tab::Compiler, "Compiler & Decompiler")] {
                let sel = self.tab == t;
                let text = if sel { RichText::new(label).strong().color(accent) } else { RichText::new(label) };
                if ui.add(egui::Button::selectable(sel, text).min_size(egui::vec2(150.0, 26.0))).clicked() {
                    self.tab = t;
                }
            }
        });
    }

    fn dialogs(&mut self, ctx: &egui::Context) {
        // ---- new session
        let mut create: Option<Session> = None;
        let mut close_new = false;
        if let Some(d) = &mut self.new_session {
            egui::Window::new("New Session").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                egui::Grid::new("new_session_grid").num_columns(3).spacing([8.0, 8.0]).show(ui, |ui| {
                    ui.label("Session name");
                    ui.add(egui::TextEdit::singleline(&mut d.name).hint_text("newgssession").desired_width(320.0));
                    ui.end_row();
                    ui.label("gameinfo.txt");
                    ui.add(egui::TextEdit::singleline(&mut d.gameinfo).desired_width(320.0));
                    if ui.button("Browse...").clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title("Select gameinfo.txt").add_filter("gameinfo", &["txt"]).pick_file() {
                            d.gameinfo = p.display().to_string();
                            if d.studiomdl.is_empty() {
                                if let Some(s) = session::guess_studiomdl(&p) {
                                    d.studiomdl = s.display().to_string();
                                }
                            }
                        }
                    }
                    ui.end_row();
                    ui.label("studiomdl.exe");
                    ui.add(egui::TextEdit::singleline(&mut d.studiomdl).hint_text("optional - only needed to compile").desired_width(320.0));
                    if ui.button("Browse...").clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title("Select studiomdl.exe").add_filter("studiomdl", &["exe"]).pick_file() {
                            d.studiomdl = p.display().to_string();
                        }
                    }
                    ui.end_row();
                });
                ui.label(RichText::new("studiomdl.exe is only used by the compiler. The Animator, Model Viewer and Decompiler work without it.").small().weak());
                if let Some(e) = &d.error {
                    ui.colored_label(Color32::from_rgb(255, 110, 110), e);
                }
                ui.horizontal(|ui| {
                    if ui.button("Create").clicked() {
                        match files::valid_session_name(&d.name) {
                            Err(e) => d.error = Some(e),
                            Ok(name) => {
                                let gi = PathBuf::from(d.gameinfo.trim().trim_matches('"'));
                                let sm = d.studiomdl.trim().trim_matches('"');
                                let dir = self.shared.files.session_dir(&name);
                                if !gi.is_file() {
                                    d.error = Some("gameinfo.txt was not found at that path.".into());
                                } else if !sm.is_empty() && !PathBuf::from(sm).is_file() {
                                    d.error = Some("studiomdl.exe was not found at that path (leave it empty to skip).".into());
                                } else if dir.exists() {
                                    d.error = Some(format!("A session called '{name}' already exists."));
                                } else {
                                    let s = Session { name: name.clone(), dir, gameinfo: gi, studiomdl: (!sm.is_empty()).then(|| PathBuf::from(sm)) };
                                    let _ = self.shared.files.ensure();
                                    match s.save() {
                                        Ok(()) => create = Some(s),
                                        Err(e) => d.error = Some(format!("Could not create the session folder: {e}")),
                                    }
                                }
                            }
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        close_new = true;
                    }
                });
            });
        }
        if close_new {
            self.new_session = None;
        }
        if let Some(s) = create {
            self.new_session = None;
            self.start_session(s);
        }

        // ---- open session
        let mut chosen: Option<String> = None;
        let mut close_open = false;
        if let Some(list) = &self.open_session {
            egui::Window::new("Open Session").collapsible(false).resizable(true).default_size([380.0, 360.0]).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.label(RichText::new(self.shared.files.sessions_dir().display().to_string()).small().weak());
                ui.separator();
                if list.is_empty() {
                    ui.label("No sessions yet. Use Files > New Session.");
                }
                egui::ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                    for name in list {
                        let dot = self.shared.dot(self.shared.theme.accent, 9.0);
                        if ui.add(egui::Button::image_and_text(dot, format!(" {name}")).min_size(egui::vec2(ui.available_width(), 24.0))).clicked() {
                            chosen = Some(name.clone());
                        }
                    }
                });
                ui.separator();
                if ui.button("Cancel").clicked() {
                    close_open = true;
                }
            });
        }
        if close_open {
            self.open_session = None;
        }
        if let Some(name) = chosen {
            self.open_session = None;
            let dir = self.shared.files.session_dir(&name);
            match Session::load(&name, &dir) {
                Ok(s) => {
                    if !s.gameinfo.is_file() {
                        self.error_popup = Some(format!("The gameinfo.txt saved in this session no longer exists:\n{}", s.gameinfo.display()));
                    } else {
                        self.start_session(s);
                    }
                }
                Err(e) => self.error_popup = Some(e),
            }
        }

        // ---- error popup
        let mut close_err = false;
        if let Some(e) = &self.error_popup {
            egui::Window::new("Error").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0)).show(ctx, |ui| {
                ui.colored_label(Color32::from_rgb(255, 120, 120), e);
                if ui.button("OK").clicked() {
                    close_err = true;
                }
            });
        }
        if close_err {
            self.error_popup = None;
        }

        // ---- settings
        if self.settings_open {
            let mut open = true;
            let mut apply = false;
            let mut reset = false;
            egui::Window::new("Settings").open(&mut open).default_size([520.0, 420.0]).show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.settings_page, 0, "Theme");
                });
                ui.separator();
                let Some(t) = &mut self.theme_draft else { return };
                if self.settings_page == 0 {
                    egui::Grid::new("theme_grid").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                        let color = |ui: &mut egui::Ui, label: &str, c: &mut Color32| {
                            ui.label(label);
                            ui.color_edit_button_srgba(c);
                            ui.end_row();
                        };
                        color(ui, "Border color", &mut t.border);
                        color(ui, "Background color", &mut t.background);
                        color(ui, "Panel color", &mut t.panel);
                        color(ui, "Text color", &mut t.text);
                        color(ui, "Accent color", &mut t.accent);
                        color(ui, "3D viewport background", &mut t.viewport_bg);
                        ui.label("Panel opacity");
                        ui.add(egui::Slider::new(&mut t.panel_opacity, 0.0..=100.0).suffix("%"));
                        ui.end_row();
                        ui.label("Background image");
                        ui.horizontal(|ui| {
                            let label = t.bg_image.as_ref().map(|p| p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()).unwrap_or_else(|| "(none)".into());
                            ui.label(label);
                            if ui.button("Choose...").clicked() {
                                if let Some(p) = rfd::FileDialog::new().add_filter("images", &["png", "jpg", "jpeg", "bmp", "tga"]).pick_file() {
                                    t.bg_image = Some(p);
                                }
                            }
                            if t.bg_image.is_some() && ui.button("Clear").clicked() {
                                t.bg_image = None;
                            }
                        });
                        ui.end_row();
                        ui.label("Background blur");
                        ui.add(egui::Slider::new(&mut t.bg_blur, 0.0..=100.0).suffix("%"));
                        ui.end_row();
                        ui.label("Background image opacity");
                        ui.add(egui::Slider::new(&mut t.bg_image_opacity, 0.0..=100.0).suffix("%"));
                        ui.end_row();
                        ui.label("Border width");
                        ui.add(egui::Slider::new(&mut t.border_width, 0.0..=4.0));
                        ui.end_row();
                        ui.label("Corner rounding");
                        ui.add(egui::Slider::new(&mut t.rounding, 0.0..=12.0));
                        ui.end_row();
                        ui.label("UI scale");
                        ui.add(egui::Slider::new(&mut t.ui_scale, 0.7..=2.0));
                        ui.end_row();
                    });
                    if t.bg_image.is_some() && t.panel_opacity > 95.0 {
                        ui.label(RichText::new("Tip: lower the panel opacity to see the background image behind the panels.").small().weak());
                    }
                    if let Some(e) = &self.background.error {
                        ui.colored_label(Color32::from_rgb(255, 120, 120), e);
                    }
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Apply & Save").clicked() {
                        apply = true;
                    }
                    if ui.button("Reset to defaults").clicked() {
                        reset = true;
                    }
                });
            });
            if reset {
                self.theme_draft = Some(Theme::default());
            }
            // live preview
            if let Some(t) = &self.theme_draft {
                if *t != self.shared.theme {
                    t.apply(ctx);
                }
            }
            if apply {
                if let Some(t) = self.theme_draft.clone() {
                    self.shared.theme = t;
                    let _ = self.shared.files.ensure();
                    if let Err(e) = self.shared.theme.save(&self.shared.files.settings_file()) {
                        self.error_popup = Some(format!("Could not save settings: {e}"));
                    }
                }
            }
            if !open {
                self.settings_open = false;
                self.theme_draft = None;
                self.shared.theme.apply(ctx);
            }
        }
    }

    fn notices(&mut self, ctx: &egui::Context) {
        if self.shared.notices.is_empty() {
            return;
        }
        let mut remove = None;
        egui::Area::new(egui::Id::unique("gs_notices")).anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-12.0, -12.0)).show(ctx, |ui| {
            for (i, (msg, err)) in self.shared.notices.iter().enumerate().rev().take(4) {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.horizontal(|ui| {
                        if *err {
                            ui.colored_label(Color32::from_rgb(255, 120, 120), msg);
                        } else {
                            ui.label(msg);
                        }
                        if ui.small_button("x").clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
        });
        if let Some(i) = remove {
            self.shared.notices.remove(i);
        }
    }

    fn welcome(&mut self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            if let Some(icon) = &self.icon {
                ui.add(egui::Image::new(icon).fit_to_exact_size(egui::vec2(96.0, 96.0)));
            }
            ui.heading(RichText::new("Golden Stunstick").size(30.0).color(self.shared.theme.accent));
            ui.label("Source engine modding workbench");
            ui.add_space(16.0);
            ui.label("Start with Files > New Session (or Open Session) to unlock the Animator, Model Viewer and Compiler & Decompiler.");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.add_space((ui.available_width() - 260.0).max(0.0) / 2.0);
                if ui.button("New Session").clicked() {
                    self.new_session = Some(NewSessionDialog { name: String::new(), gameinfo: String::new(), studiomdl: String::new(), error: None });
                }
                if ui.button("Open Session").clicked() {
                    self.open_session = Some(self.shared.files.list_sessions());
                }
            });
        });
    }
}

impl eframe::App for GoldenStunstick {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.shared.files.periodic_check();
        // wake up even when idle so a deleted goldenstunstick_files folder is re-created
        ctx.request_repaint_after(std::time::Duration::from_secs(3));
        self.poll_mount();
        if self.mounting.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        let theme_for_bg = self.theme_draft.clone().unwrap_or_else(|| self.shared.theme.clone());
        self.background.update(&ctx, &theme_for_bg);
        self.background.paint(&ctx, &theme_for_bg);

        let title = match &self.shared.session {
            Some(s) => format!("Golden Stunstick - {}", s.name),
            None => "Golden Stunstick".to_string(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        let panel_frame = egui::Frame::new().fill(theme_for_bg.panel_fill()).inner_margin(egui::Margin::symmetric(8, 4)).stroke(egui::Stroke::new(theme_for_bg.border_width, theme_for_bg.border.gamma_multiply(0.6)));
        egui::Panel::top("gs_top").frame(panel_frame).show(ui, |ui| {
            self.top_bar(ui);
            if self.shared.session.is_some() {
                ui.separator();
                self.session_header(ui);
            }
        });

        egui::CentralPanel::default().frame(egui::Frame::new().inner_margin(egui::Margin::same(6))).show(ui, |ui| {
            if self.shared.session.is_none() {
                self.welcome(ui);
                return;
            }
            match self.tab {
                Tab::ModelViewer => self.viewer.ui(ui, &mut self.shared),
                Tab::Compiler => self.compiler.ui(ui, &mut self.shared),
                Tab::Animator => self.animator.ui(ui, &mut self.shared),
            }
        });

        self.dialogs(&ctx);
        self.notices(&ctx);
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let (Some(gl), Ok(mut r)) = (gl, self.shared.renderer.lock()) {
            if let Some(mut r) = r.take() {
                r.destroy(gl);
            }
        }
    }
}

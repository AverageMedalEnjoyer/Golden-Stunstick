//! Compiler & Decompiler tab (a Crowbar-style front end for studiomdl plus
//! Golden Stunstick's own verified decompiler).

use crate::app::Shared;
use crate::compile::{self, CompileJob, CompileMsg};
use crate::decompile::{self, DecompileInput, DecompileOptions};
use crate::formats::qc::{self, Issue, Severity};
use crate::verify::Check;
use eframe::egui::{self, Color32, RichText};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};

#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum Page {
    #[default]
    Compile,
    Decompile,
}

/// Where compiled models go.
#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum OutMode {
    /// `<game>/models/<$modelname>` (studiomdl's own behaviour)
    #[default]
    GameModels,
    /// `<qc folder>/<subfolder>`
    NextToQc,
    /// a folder of your choice
    Custom,
}

#[derive(PartialEq, Eq, Clone, Copy, Default)]
enum DecompSource {
    #[default]
    DiskFile,
    DiskFolder,
    Game,
}

enum DecompMsg {
    Log(String),
    Checks(String, Vec<Check>),
    Done { files: usize },
}

pub struct CompilerTab {
    page: Page,
    // compile
    qc_path: String,
    batch_folder: bool,
    recursive: bool,
    opt_verbose: bool,
    opt_nox360: bool,
    opt_fastbuild: bool,
    opt_definebones: bool,
    opt_printbones: bool,
    opt_dumpmaterials: bool,
    lint_first: bool,
    extra_args: String,
    copy_to: String,
    out_mode: OutMode,
    out_custom: String,
    out_sub: String,
    out_keep_tree: bool,
    last_out_dir: Option<PathBuf>,
    log: Vec<String>,
    issues: Vec<Issue>,
    show_fixes: bool,
    running: Option<Receiver<CompileMsg>>,
    queue: Vec<PathBuf>,
    current_qc: Option<PathBuf>,
    current_lines: Vec<String>,
    last_error_log: Option<PathBuf>,
    summary: Vec<(PathBuf, bool)>,
    // decompile
    d_source: DecompSource,
    d_input: String,
    d_game_filter: String,
    d_game_model: Option<String>,
    d_out: String,
    d_opts: DecompileOptions,
    d_verify: bool,
    d_log: Vec<String>,
    d_checks: Vec<(String, Vec<Check>)>,
    d_running: Option<Receiver<DecompMsg>>,
    d_last_out: Option<PathBuf>,
    model_list: Option<Vec<String>>,
}

impl Default for CompilerTab {
    fn default() -> Self {
        CompilerTab {
            page: Page::Compile,
            qc_path: String::new(),
            batch_folder: false,
            recursive: true,
            opt_verbose: true,
            opt_nox360: true,
            opt_fastbuild: false,
            opt_definebones: false,
            opt_printbones: false,
            opt_dumpmaterials: false,
            lint_first: true,
            extra_args: String::new(),
            copy_to: String::new(),
            out_mode: OutMode::GameModels,
            out_custom: String::new(),
            out_sub: "compiled".into(),
            out_keep_tree: false,
            last_out_dir: None,
            log: Vec::new(),
            issues: Vec::new(),
            show_fixes: false,
            running: None,
            queue: Vec::new(),
            current_qc: None,
            current_lines: Vec::new(),
            last_error_log: None,
            summary: Vec::new(),
            d_source: DecompSource::DiskFile,
            d_input: String::new(),
            d_game_filter: String::new(),
            d_game_model: None,
            d_out: String::new(),
            d_opts: DecompileOptions::default(),
            d_verify: true,
            d_log: Vec::new(),
            d_checks: Vec::new(),
            d_running: None,
            d_last_out: None,
            model_list: None,
        }
    }
}

/// Private game folder studiomdl compiles into when the output is not the game's own
/// models folder (removed when the compile run ends).
fn staging_game_dir() -> PathBuf {
    std::env::temp_dir().join(format!("golden_stunstick_compile_{}", std::process::id()))
}

pub fn open_folder(p: &Path) {
    let _ = std::process::Command::new("explorer").arg(p).spawn();
}

fn sev_color(s: Severity) -> Color32 {
    match s {
        Severity::Error => Color32::from_rgb(255, 110, 110),
        Severity::Warning => Color32::from_rgb(240, 200, 90),
        Severity::Info => Color32::from_rgb(150, 190, 255),
    }
}

fn log_line_color(l: &str) -> Option<Color32> {
    let u = l.to_ascii_uppercase();
    if u.contains("ERROR") {
        Some(Color32::from_rgb(255, 110, 110))
    } else if u.contains("WARNING") {
        Some(Color32::from_rgb(240, 200, 90))
    } else if u.starts_with("COMPLETED") || u.contains("[OK]") {
        Some(Color32::from_rgb(130, 220, 130))
    } else {
        None
    }
}

fn find_qcs(dir: &Path, recursive: bool, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    items.sort();
    for p in items {
        if p.is_dir() {
            if recursive {
                find_qcs(&p, recursive, out);
            }
        } else if p.extension().map(|e| e.eq_ignore_ascii_case("qc")).unwrap_or(false) {
            out.push(p);
        }
    }
}

impl CompilerTab {
    pub fn ui(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        self.poll(sh, ui.ctx());
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.page, Page::Compile, RichText::new("Compile").strong());
            ui.selectable_value(&mut self.page, Page::Decompile, RichText::new("Decompile").strong());
        });
        ui.separator();
        match self.page {
            Page::Compile => self.compile_ui(ui, sh),
            Page::Decompile => self.decompile_ui(ui, sh),
        }
    }

    fn poll(&mut self, sh: &mut Shared, ctx: &egui::Context) {
        // compile messages
        let mut finished: Option<bool> = None;
        if let Some(rx) = &self.running {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
            while let Ok(m) = rx.try_recv() {
                match m {
                    CompileMsg::Line(l) => {
                        self.current_lines.push(l.clone());
                        self.log.push(l);
                    }
                    CompileMsg::Finished { success, exit_code } => {
                        self.log.push(format!("--- studiomdl finished ({}) ---", if success { "success".to_string() } else { format!("FAILED, exit code {}", exit_code.map(|c| c.to_string()).unwrap_or("?".into())) }));
                        finished = Some(success);
                    }
                }
            }
        }
        if let Some(success) = finished {
            self.running = None;
            let qc_path = self.current_qc.clone().unwrap_or_default();
            let mut issues = compile::analyze_output(&self.current_lines, &qc_path);
            if !success {
                if issues.iter().all(|i| i.severity != Severity::Error) {
                    issues.insert(0, Issue { severity: Severity::Error, file: qc_path.clone(), line: 0, message: "studiomdl stopped without a recognizable error message".into(), suggestion: "Read the end of the log; enable -verbose for more detail.".into() });
                }
                if let Some(s) = &sh.session {
                    let job = self.job_for(&qc_path, sh);
                    match compile::write_error_log(&s.logs_dir(), &job, &self.current_lines, &issues) {
                        Ok(p) => {
                            self.log.push(format!("Error report written to {}", p.display()));
                            self.last_error_log = Some(p);
                        }
                        Err(e) => self.log.push(format!("Could not write the error report: {e}")),
                    }
                }
                sh.error(format!("Compile failed: {}", qc_path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
            } else {
                self.copy_outputs(&qc_path, sh);
            }
            self.issues.extend(issues);
            self.summary.push((qc_path, success));
            self.start_next(sh);
        }

        // decompile messages
        let mut d_done = false;
        if let Some(rx) = &self.d_running {
            ctx.request_repaint_after(std::time::Duration::from_millis(60));
            while let Ok(m) = rx.try_recv() {
                match m {
                    DecompMsg::Log(l) => self.d_log.push(l),
                    DecompMsg::Checks(name, c) => self.d_checks.push((name, c)),
                    DecompMsg::Done { files } => {
                        self.d_log.push(format!("=== finished: {files} model(s) processed ==="));
                        d_done = true;
                    }
                }
            }
        }
        if d_done {
            self.d_running = None;
            sh.notify("Decompile finished");
        }
    }

    fn job_for(&self, qc: &Path, sh: &Shared) -> CompileJob {
        let mut extra = Vec::new();
        if self.opt_verbose {
            extra.push("-verbose".into());
        }
        if self.opt_nox360 {
            extra.push("-nox360".into());
        }
        if self.opt_fastbuild {
            extra.push("-fastbuild".into());
        }
        if self.opt_definebones {
            extra.push("-definebones".into());
        }
        if self.opt_printbones {
            extra.push("-printbones".into());
        }
        if self.opt_dumpmaterials {
            extra.push("-dumpmaterials".into());
        }
        extra.extend(self.extra_args.split_whitespace().map(|s| s.to_string()));
        let s = sh.session.as_ref();
        CompileJob {
            studiomdl: s.and_then(|s| s.studiomdl.clone()).unwrap_or_default(),
            game_dir: if self.out_mode == OutMode::GameModels { s.map(|s| s.game_dir()).unwrap_or_default() } else { staging_game_dir() },
            qc: qc.to_path_buf(),
            extra_args: extra,
        }
    }

    /// The folder a QC's compiled files end up in (None for the game's models folder).
    fn output_root(&self, qc: &Path) -> Option<PathBuf> {
        match self.out_mode {
            OutMode::GameModels => None,
            OutMode::NextToQc => Some(qc.parent().unwrap_or(Path::new(".")).join(self.out_sub.trim())),
            OutMode::Custom => Some(PathBuf::from(self.out_custom.trim().trim_matches('"'))),
        }
    }

    fn copy_outputs(&mut self, qc_path: &Path, sh: &Shared) {
        let Ok(q) = qc::Qc::load(qc_path) else { return };
        let Some(name) = q.model_name() else { return };
        let Some(s) = &sh.session else { return };
        let rel = name.replace('\\', "/");
        let rel = rel.strip_prefix("models/").unwrap_or(&rel).to_string();
        let rel_dir = Path::new(&rel).parent().unwrap_or(Path::new("")).to_path_buf();
        const EXTS: [&str; 8] = [".mdl", ".vvd", ".dx90.vtx", ".dx80.vtx", ".sw.vtx", ".phy", ".ani", ".vtx"];
        let files_of = |stem: &Path| -> Vec<PathBuf> { EXTS.iter().map(|e| PathBuf::from(format!("{}{e}", stem.display()))).filter(|p| p.is_file()).collect() };
        let stem = match self.output_root(qc_path) {
            None => {
                let out = s.game_dir().join("models").join(&rel);
                self.log.push(format!("Output: {}", out.display()));
                self.last_out_dir = out.parent().map(|p| p.to_path_buf());
                out.with_extension("")
            }
            Some(root) => {
                // studiomdl wrote into the staging game; move the files to the chosen folder
                let staged = staging_game_dir().join("models").join(&rel).with_extension("");
                let dest = if self.out_keep_tree { root.join(&rel_dir) } else { root };
                if let Err(e) = std::fs::create_dir_all(&dest) {
                    self.log.push(format!("ERROR: could not create {}: {e}", dest.display()));
                    return;
                }
                let staged_files = files_of(&staged);
                if staged_files.is_empty() {
                    self.log.push(format!("ERROR: studiomdl reported success but no files were found in {}", staged.parent().unwrap_or(Path::new("")).display()));
                    return;
                }
                for src in staged_files {
                    let d = dest.join(src.file_name().unwrap_or_default());
                    let r = std::fs::rename(&src, &d).or_else(|_| std::fs::copy(&src, &d).and_then(|_| std::fs::remove_file(&src)));
                    match r {
                        Ok(_) => self.log.push(format!("Output: {}", d.display())),
                        Err(e) => self.log.push(format!("ERROR writing {}: {e}", d.display())),
                    }
                }
                self.last_out_dir = Some(dest.clone());
                dest.join(Path::new(&rel).file_stem().unwrap_or_default())
            }
        };
        if self.copy_to.trim().is_empty() {
            return;
        }
        let dest = PathBuf::from(self.copy_to.trim()).join(&rel_dir);
        let _ = std::fs::create_dir_all(&dest);
        for src in files_of(&stem) {
            let d = dest.join(src.file_name().unwrap_or_default());
            match std::fs::copy(&src, &d) {
                Ok(_) => self.log.push(format!("Copied {}", d.display())),
                Err(e) => self.log.push(format!("ERROR copying {}: {e}", src.display())),
            }
        }
    }

    fn start_compile(&mut self, sh: &mut Shared) {
        self.issues.clear();
        self.summary.clear();
        self.log.clear();
        self.last_error_log = None;
        let path = PathBuf::from(self.qc_path.trim().trim_matches('"'));
        let mut qcs = Vec::new();
        if self.batch_folder {
            find_qcs(&path, self.recursive, &mut qcs);
            if qcs.is_empty() {
                sh.error("No .qc files found in that folder");
                return;
            }
        } else {
            if !path.is_file() {
                sh.error("QC file not found");
                return;
            }
            qcs.push(path);
        }
        let Some(s) = &sh.session else { return };
        match &s.studiomdl {
            Some(p) if p.is_file() => {}
            _ => {
                sh.error("This session has no valid studiomdl.exe. Right-click the studiomdl.exe path at the top of the window to set one.");
                return;
            }
        }
        match self.out_mode {
            OutMode::GameModels => {}
            OutMode::Custom if self.out_custom.trim().is_empty() => {
                sh.error("Choose an output folder (or pick another output option)");
                return;
            }
            OutMode::NextToQc if self.out_sub.trim().is_empty() || self.out_sub.contains(['/', '\\', ':']) => {
                sh.error("Type a sub-folder name for 'Folder next to each QC' (a plain name, e.g. compiled)");
                return;
            }
            _ => {
                // studiomdl always writes to <game>/models, so compile into a private staging
                // game (searching the session's game content) and move the files afterwards
                let dir = staging_game_dir();
                let _ = std::fs::remove_dir_all(&dir);
                if let Err(e) = compile::make_temp_game(&dir, sh.fs.as_deref()) {
                    sh.error(format!("Could not create the staging folder {}: {e}", dir.display()));
                    return;
                }
            }
        }
        self.last_out_dir = None;
        self.queue = qcs;
        self.queue.reverse();
        self.start_next(sh);
    }

    fn start_next(&mut self, sh: &mut Shared) {
        let Some(qc) = self.queue.pop() else {
            self.current_qc = None;
            if self.out_mode != OutMode::GameModels {
                let _ = std::fs::remove_dir_all(staging_game_dir());
            }
            return;
        };
        self.current_lines.clear();
        self.log.push(format!("=== Compiling {} ===", qc.display()));
        if self.lint_first {
            match qc::Qc::load(&qc) {
                Ok(q) => {
                    let lint = qc::lint(&q);
                    let errors = lint.iter().filter(|i| i.severity == Severity::Error).count();
                    for i in &lint {
                        self.log.push(format!("[pre-check] {}: {}{}", if i.severity == Severity::Error { "ERROR" } else { "WARNING" }, i.message, if i.line > 0 { format!(" (line {})", i.line) } else { String::new() }));
                    }
                    self.issues.extend(lint.iter().cloned());
                    if errors > 0 {
                        self.log.push(format!("Pre-check found {errors} error(s); studiomdl was not started. Fix them or untick 'Check QC before compiling'."));
                        let job = self.job_for(&qc, sh);
                        if let Some(s) = &sh.session {
                            if let Ok(p) = compile::write_error_log(&s.logs_dir(), &job, &self.log, &lint) {
                                self.log.push(format!("Error report written to {}", p.display()));
                                self.last_error_log = Some(p);
                            }
                        }
                        self.summary.push((qc, false));
                        return self.start_next(sh);
                    }
                }
                Err(e) => self.log.push(format!("[pre-check] ERROR: {e}")),
            }
        }
        let job = self.job_for(&qc, sh);
        self.current_qc = Some(qc);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || compile::run(&job, &tx));
        self.running = Some(rx);
    }

    fn compile_ui(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        let busy = self.running.is_some();
        egui::Grid::new("compile_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            ui.label(if self.batch_folder { "QC folder" } else { "QC file" });
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.qc_path).desired_width(520.0));
                if ui.button("Browse...").clicked() {
                    let pick = if self.batch_folder { rfd::FileDialog::new().pick_folder() } else { rfd::FileDialog::new().add_filter("QC", &["qc"]).pick_file() };
                    if let Some(p) = pick {
                        self.qc_path = p.display().to_string();
                    }
                }
                ui.checkbox(&mut self.batch_folder, "Compile every QC in a folder");
                if self.batch_folder {
                    ui.checkbox(&mut self.recursive, "include sub-folders");
                }
            });
            ui.end_row();
            ui.label("Game folder");
            ui.label(sh.session.as_ref().map(|s| s.game_dir().display().to_string()).unwrap_or_default());
            ui.end_row();
            ui.label("Output");
            ui.add_enabled_ui(!busy, |ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.radio_value(&mut self.out_mode, OutMode::GameModels, "Game's models folder");
                        ui.radio_value(&mut self.out_mode, OutMode::NextToQc, "Folder next to each QC");
                        ui.radio_value(&mut self.out_mode, OutMode::Custom, "Custom folder");
                    });
                    match self.out_mode {
                        OutMode::GameModels => {
                            let g = sh.session.as_ref().map(|s| s.game_dir().join("models").display().to_string()).unwrap_or_default();
                            ui.label(RichText::new(format!("{g}\\<$modelname>")).small().weak());
                        }
                        OutMode::NextToQc => {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("<QC folder>\\").weak());
                                ui.add(egui::TextEdit::singleline(&mut self.out_sub).desired_width(160.0));
                            });
                        }
                        OutMode::Custom => {
                            ui.horizontal(|ui| {
                                ui.add(egui::TextEdit::singleline(&mut self.out_custom).hint_text("output folder").desired_width(520.0));
                                if ui.button("Browse...").clicked() {
                                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                                        self.out_custom = p.display().to_string();
                                    }
                                }
                            });
                        }
                    }
                    if self.out_mode != OutMode::GameModels {
                        ui.checkbox(&mut self.out_keep_tree, "Keep the $modelname sub-folders (e.g. player\\john\\)")
                            .on_hover_text("studiomdl compiles into a private staging folder; the finished files are moved here, so the game's models folder is not touched");
                    }
                });
            });
            ui.end_row();
            ui.label("studiomdl.exe");
            ui.label(sh.session.as_ref().and_then(|s| s.studiomdl.as_ref()).map(|p| p.display().to_string()).unwrap_or_else(|| "(not set in this session)".into()));
            ui.end_row();
            ui.label("Options");
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut self.lint_first, "Check QC before compiling");
                ui.checkbox(&mut self.opt_verbose, "-verbose");
                ui.checkbox(&mut self.opt_nox360, "-nox360");
                ui.checkbox(&mut self.opt_fastbuild, "-fastbuild");
                ui.checkbox(&mut self.opt_definebones, "-definebones");
                ui.checkbox(&mut self.opt_printbones, "-printbones");
                ui.checkbox(&mut self.opt_dumpmaterials, "-dumpmaterials");
            });
            ui.end_row();
            ui.label("Extra arguments");
            ui.add(egui::TextEdit::singleline(&mut self.extra_args).desired_width(520.0));
            ui.end_row();
            ui.label("Also copy output to");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.copy_to).hint_text("optional folder").desired_width(520.0));
                if ui.button("Browse...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.copy_to = p.display().to_string();
                    }
                }
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new(RichText::new("Compile").strong()).min_size(egui::vec2(110.0, 28.0))).clicked() {
                self.start_compile(sh);
            }
            if ui.add_enabled(!busy, egui::Button::new("Check QC only")).clicked() {
                self.issues.clear();
                self.log.clear();
                match qc::Qc::load(Path::new(self.qc_path.trim().trim_matches('"'))) {
                    Ok(q) => {
                        self.issues = qc::lint(&q);
                        self.log.push(format!("Checked {} file(s): {} issue(s)", q.files.len(), self.issues.len()));
                    }
                    Err(e) => self.log.push(format!("ERROR: {e}")),
                }
            }
            if busy {
                ui.spinner();
                ui.label(format!("Compiling {}...", self.current_qc.as_ref().and_then(|p| p.file_name()).map(|s| s.to_string_lossy().to_string()).unwrap_or_default()));
            }
            if let Some(p) = &self.last_error_log {
                if ui.link(format!("Open {}", p.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default())).clicked() {
                    let _ = std::process::Command::new("notepad").arg(p).spawn();
                }
            }
            if let Some(d) = self.last_out_dir.clone() {
                if ui.button("Open output folder").on_hover_text(d.display().to_string()).clicked() {
                    open_folder(&d);
                }
            }
            if let Some(s) = &sh.session {
                if ui.button("Logs folder").clicked() {
                    let _ = std::fs::create_dir_all(s.logs_dir());
                    open_folder(&s.logs_dir());
                }
            }
        });
        if !self.summary.is_empty() && self.summary.len() > 1 {
            let ok = self.summary.iter().filter(|s| s.1).count();
            ui.label(format!("Batch: {ok} succeeded, {} failed", self.summary.len() - ok));
        }
        ui.separator();

        // issues + log side by side
        let avail = ui.available_size();
        ui.horizontal(|ui| {
            let log_w = (avail.x * 0.58).max(300.0);
            ui.vertical(|ui| {
                ui.set_width(log_w);
                ui.set_min_height(avail.y - 8.0);
                ui.label(RichText::new("studiomdl output").strong());
                egui::Frame::new().fill(Color32::from_black_alpha(140)).inner_margin(6.0).show(ui, |ui| {
                    egui::ScrollArea::both().id_salt("compile_log").stick_to_bottom(true).auto_shrink([false, false]).min_scrolled_height(avail.y - 50.0).max_height(avail.y - 40.0).show(ui, |ui| {
                        for l in &self.log {
                            let t = RichText::new(l).monospace().size(12.0);
                            ui.label(match log_line_color(l) {
                                Some(c) => t.color(c),
                                None => t,
                            });
                        }
                    });
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    let errs = self.issues.iter().filter(|i| i.severity == Severity::Error).count();
                    let warns = self.issues.iter().filter(|i| i.severity == Severity::Warning).count();
                    ui.label(RichText::new(format!("Issues: {errs} error(s), {warns} warning(s)")).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add_enabled(!self.issues.is_empty(), egui::Button::new("Suggested Fixes")).clicked() {
                            self.show_fixes = true;
                        }
                    });
                });
                egui::ScrollArea::vertical().id_salt("issues").auto_shrink([false, false]).max_height(avail.y - 40.0).show(ui, |ui| {
                    for i in &self.issues {
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(sev_color(i.severity), match i.severity {
                                Severity::Error => "ERROR",
                                Severity::Warning => "WARN",
                                Severity::Info => "INFO",
                            });
                            if i.line > 0 {
                                let f = i.file.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                                ui.label(RichText::new(format!("{f}:{}", i.line)).monospace());
                            }
                            ui.label(&i.message);
                        });
                    }
                    if self.issues.is_empty() {
                        ui.label(RichText::new("No issues.").weak());
                    }
                });
            });
        });

        if self.show_fixes {
            let mut open = true;
            egui::Window::new("Suggested Fixes").open(&mut open).default_size([560.0, 420.0]).show(ui.ctx(), |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (n, i) in self.issues.iter().enumerate() {
                        ui.group(|ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.colored_label(sev_color(i.severity), format!("{}.", n + 1));
                                ui.label(RichText::new(&i.message).strong());
                            });
                            if i.line > 0 {
                                ui.label(RichText::new(format!("{} line {}", i.file.display(), i.line)).small().weak());
                                if let Some(src) = std::fs::read_to_string(&i.file).ok().and_then(|t| t.lines().nth(i.line - 1).map(|s| s.to_string())) {
                                    ui.label(RichText::new(src.trim()).monospace().color(Color32::from_rgb(200, 200, 160)));
                                }
                            }
                            ui.label(format!("Fix: {}", i.suggestion));
                        });
                    }
                });
            });
            if !open {
                self.show_fixes = false;
            }
        }
    }

    fn decompile_ui(&mut self, ui: &mut egui::Ui, sh: &mut Shared) {
        let busy = self.d_running.is_some();
        ui.horizontal(|ui| {
            ui.label("Source:");
            ui.selectable_value(&mut self.d_source, DecompSource::DiskFile, "MDL file");
            ui.selectable_value(&mut self.d_source, DecompSource::DiskFolder, "Folder of MDLs");
            ui.selectable_value(&mut self.d_source, DecompSource::Game, "From mounted game");
        });
        egui::Grid::new("decomp_grid").num_columns(2).spacing([10.0, 6.0]).show(ui, |ui| {
            match self.d_source {
                DecompSource::DiskFile | DecompSource::DiskFolder => {
                    ui.label(if self.d_source == DecompSource::DiskFile { "MDL file" } else { "Folder" });
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.d_input).desired_width(520.0));
                        if ui.button("Browse...").clicked() {
                            let pick = if self.d_source == DecompSource::DiskFile { rfd::FileDialog::new().add_filter("MDL", &["mdl"]).pick_file() } else { rfd::FileDialog::new().pick_folder() };
                            if let Some(p) = pick {
                                self.d_input = p.display().to_string();
                                if self.d_out.is_empty() {
                                    let base = if p.is_dir() { p.clone() } else { p.parent().map(|x| x.to_path_buf()).unwrap_or_default() };
                                    self.d_out = base.join("decompiled").display().to_string();
                                }
                            }
                        }
                    });
                    ui.end_row();
                }
                DecompSource::Game => {
                    ui.label("Model");
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label("Filter");
                            ui.add(egui::TextEdit::singleline(&mut self.d_game_filter).hint_text("e.g. player/heavy").desired_width(300.0));
                            ui.label(self.d_game_model.clone().unwrap_or_else(|| "(none selected)".into()));
                        });
                        if let Some(fs) = &sh.fs {
                            if self.model_list.is_none() {
                                self.model_list = Some(fs.list_models().into_iter().map(|m| m.path).collect());
                            }
                            let filt = self.d_game_filter.to_ascii_lowercase();
                            let list: Vec<&String> = self.model_list.as_ref().map(|l| l.iter().filter(|p| filt.is_empty() || p.contains(&filt)).collect()).unwrap_or_default();
                            egui::ScrollArea::vertical().id_salt("decomp_models").max_height(140.0).show_rows(ui, 18.0, list.len(), |ui, range| {
                                for p in &list[range] {
                                    if ui.selectable_label(self.d_game_model.as_deref() == Some(p.as_str()), p.as_str()).clicked() {
                                        self.d_game_model = Some((*p).clone());
                                        if let Some(s) = &sh.session {
                                            self.d_out = s.dir.join("decompiled").join(p.trim_end_matches(".mdl")).display().to_string();
                                        }
                                    }
                                }
                            });
                        } else {
                            ui.label("Game content is still mounting...");
                        }
                    });
                    ui.end_row();
                }
            }
            ui.label("Output folder");
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut self.d_out).desired_width(520.0));
                if ui.button("Browse...").clicked() {
                    if let Some(p) = rfd::FileDialog::new().pick_folder() {
                        self.d_out = p.display().to_string();
                    }
                }
            });
            ui.end_row();
            ui.label("Write");
            ui.horizontal_wrapped(|ui| {
                let o = &mut self.d_opts;
                ui.checkbox(&mut o.write_qc, "QC");
                ui.checkbox(&mut o.reference_smd, "Reference SMDs");
                ui.checkbox(&mut o.lod_smd, "LOD SMDs");
                ui.checkbox(&mut o.physics_smd, "Physics SMD");
                ui.checkbox(&mut o.anim_smd, "Animation SMDs");
                ui.checkbox(&mut o.flex_vta, "Flex VTA");
                ui.checkbox(&mut o.procedural_vrd, "Procedural bones VRD");
            });
            ui.end_row();
            ui.label("QC options");
            ui.horizontal_wrapped(|ui| {
                let o = &mut self.d_opts;
                ui.checkbox(&mut o.definebones, "Always write $definebone");
                ui.checkbox(&mut o.group_into_qci, "Group into QCI files");
                ui.checkbox(&mut o.prefix_with_model_name, "Prefix file names with model name");
                ui.checkbox(&mut o.skip_hitboxes_if_auto, "Skip auto-generated hitboxes");
                ui.label("Animations sub-folder");
                ui.add(egui::TextEdit::singleline(&mut o.anims_subfolder).desired_width(80.0));
            });
            ui.end_row();
            ui.label("Accuracy");
            ui.vertical(|ui| {
                ui.checkbox(&mut self.d_verify, "Verify by recompiling with studiomdl and comparing with the original (recommended)");
                ui.label(RichText::new("Every decompile also re-reads its own SMDs and compares them with the MDL frame by frame.").small().weak());
            });
            ui.end_row();
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(!busy, egui::Button::new(RichText::new("Decompile").strong()).min_size(egui::vec2(110.0, 28.0))).clicked() {
                self.start_decompile(sh);
            }
            if busy {
                ui.spinner();
                ui.label("Working...");
            }
            if let Some(p) = &self.d_last_out {
                if ui.button("Open output folder").clicked() {
                    open_folder(p);
                }
            }
        });
        ui.separator();
        let avail = ui.available_size();
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width((avail.x * 0.5).max(300.0));
                ui.set_min_height(avail.y - 8.0);
                ui.label(RichText::new("Log").strong());
                egui::Frame::new().fill(Color32::from_black_alpha(140)).inner_margin(6.0).show(ui, |ui| {
                    egui::ScrollArea::both().id_salt("decomp_log").stick_to_bottom(true).auto_shrink([false, false]).min_scrolled_height(avail.y - 50.0).max_height(avail.y - 40.0).show(ui, |ui| {
                        for l in &self.d_log {
                            let t = RichText::new(l).monospace().size(12.0);
                            ui.label(match log_line_color(l) {
                                Some(c) => t.color(c),
                                None => t,
                            });
                        }
                    });
                });
            });
            ui.separator();
            ui.vertical(|ui| {
                ui.label(RichText::new("Accuracy report").strong());
                egui::ScrollArea::vertical().id_salt("decomp_checks").auto_shrink([false, false]).max_height(avail.y - 40.0).show(ui, |ui| {
                    if self.d_checks.is_empty() {
                        ui.label(RichText::new("Run a decompile to see the verification results.").weak());
                    }
                    for (name, checks) in &self.d_checks {
                        let all_ok = checks.iter().all(|c| c.ok);
                        let (good, bad) = (Color32::from_rgb(130, 220, 130), Color32::from_rgb(255, 120, 120));
                        egui::CollapsingHeader::new(RichText::new(format!("{name} \u{2013} {}", if all_ok { "all checks pass" } else { "differences found" })).color(if all_ok { good } else { bad }))
                            .default_open(true)
                            .id_salt(name)
                            .show(ui, |ui| {
                                for c in checks {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.add(sh.dot(if c.ok { good } else { bad }, 10.0)).on_hover_text(if c.ok { "matches" } else { "differs" });
                                        ui.label(RichText::new(&c.category).strong());
                                        ui.label(&c.detail);
                                    });
                                }
                            });
                    }
                });
            });
        });
    }

    fn start_decompile(&mut self, sh: &mut Shared) {
        self.d_log.clear();
        self.d_checks.clear();
        let out_root = PathBuf::from(self.d_out.trim().trim_matches('"'));
        if self.d_out.trim().is_empty() {
            sh.error("Choose an output folder");
            return;
        }
        enum Item {
            Disk(PathBuf),
            Game(String),
        }
        let mut items: Vec<Item> = Vec::new();
        match self.d_source {
            DecompSource::DiskFile => {
                let p = PathBuf::from(self.d_input.trim().trim_matches('"'));
                if !p.is_file() {
                    sh.error("MDL file not found");
                    return;
                }
                items.push(Item::Disk(p));
            }
            DecompSource::DiskFolder => {
                let p = PathBuf::from(self.d_input.trim().trim_matches('"'));
                let mut v = Vec::new();
                fn walk(d: &Path, out: &mut Vec<PathBuf>) {
                    if let Ok(rd) = std::fs::read_dir(d) {
                        for e in rd.flatten() {
                            let p = e.path();
                            if p.is_dir() {
                                walk(&p, out);
                            } else if p.extension().map(|x| x.eq_ignore_ascii_case("mdl")).unwrap_or(false) {
                                out.push(p);
                            }
                        }
                    }
                }
                walk(&p, &mut v);
                v.sort();
                if v.is_empty() {
                    sh.error("No .mdl files in that folder");
                    return;
                }
                items.extend(v.into_iter().map(Item::Disk));
            }
            DecompSource::Game => match &self.d_game_model {
                Some(m) => items.push(Item::Game(m.clone())),
                None => {
                    sh.error("Pick a model from the list");
                    return;
                }
            },
        }
        let multi = items.len() > 1;
        let opts = self.d_opts.clone();
        let verify = self.d_verify;
        let studiomdl = sh.session.as_ref().and_then(|s| s.studiomdl.clone()).filter(|p| p.is_file());
        if verify && studiomdl.is_none() {
            self.d_log.push("NOTE: no studiomdl.exe in this session, so the recompile comparison is skipped (the SMD self-check still runs).".into());
        }
        let fs = sh.fs.clone();
        let (tx, rx): (Sender<DecompMsg>, Receiver<DecompMsg>) = std::sync::mpsc::channel();
        self.d_running = Some(rx);
        self.d_last_out = Some(out_root.clone());
        std::thread::spawn(move || {
            let mut n = 0;
            for it in items {
                let (input, label) = match it {
                    Item::Disk(p) => (DecompileInput::from_disk(&p), p.display().to_string()),
                    Item::Game(g) => (fs.as_deref().ok_or_else(|| "game not mounted".to_string()).and_then(|f| DecompileInput::from_gamefs(f, &g)), g),
                };
                let input = match input {
                    Ok(i) => i,
                    Err(e) => {
                        let _ = tx.send(DecompMsg::Log(format!("ERROR: {label}: {e}")));
                        continue;
                    }
                };
                let out = if multi { out_root.join(&input.stem) } else { out_root.clone() };
                let _ = tx.send(DecompMsg::Log(format!("=== {label} -> {}", out.display())));
                let tx2 = tx.clone();
                let mut log = move |s: String| {
                    let _ = tx2.send(DecompMsg::Log(s));
                };
                match decompile::decompile(&input, &out, &opts, &mut log) {
                    Ok(report) => {
                        let mdl = crate::formats::mdl::Mdl::parse(input.mdl.clone());
                        let mut checks = Vec::new();
                        if let Ok(mdl) = &mdl {
                            checks.extend(crate::verify::self_check(mdl, input.ani.as_deref(), input.vvd.as_deref(), input.vtx.as_deref(), &report, &out));
                        }
                        if verify {
                            if let (Some(sm), Some(qc)) = (&studiomdl, &report.qc_path) {
                                log("Verifying: recompiling the decompiled QC with studiomdl...".into());
                                match crate::verify::roundtrip(&input, qc, sm, fs.as_deref(), &out.join("_gs_verify"), &mut log) {
                                    Ok(c) => checks.extend(c),
                                    Err(e) => checks.push(Check { category: "Recompile".into(), ok: false, detail: e }),
                                }
                                let _ = std::fs::remove_dir_all(out.join("_gs_verify"));
                            }
                        }
                        let _ = tx.send(DecompMsg::Checks(input.stem.clone(), checks));
                    }
                    Err(e) => {
                        let _ = tx.send(DecompMsg::Log(format!("ERROR: {e}")));
                    }
                }
                n += 1;
            }
            let _ = tx.send(DecompMsg::Done { files: n });
        });
    }
}

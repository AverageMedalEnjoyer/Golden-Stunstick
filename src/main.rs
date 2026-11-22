// Hide the console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod tabs;

pub use golden_stunstick::{anim, compile, decompile, formats, gamefs, render, scene, verify};

/// The window / taskbar icon, embedded in the EXE.
pub const ICON_PNG: &[u8] = include_bytes!("../assets/Stunstick_Icon.png");
/// A white anti-aliased dot, tinted at draw time (list markers, pass/fail lights, swatches).
pub const CIRCLE_PNG: &[u8] = include_bytes!("../assets/circle.png");

fn window_icon() -> Option<eframe::egui::IconData> {
    let img = image::load_from_memory(ICON_PNG).ok()?.thumbnail(256, 256).to_rgba8();
    Some(eframe::egui::IconData { width: img.width(), height: img.height(), rgba: img.into_raw() })
}

fn main() -> eframe::Result<()> {
    let mut viewport = eframe::egui::ViewportBuilder::default().with_title("Golden Stunstick").with_inner_size([1280.0, 760.0]).with_min_inner_size([900.0, 600.0]).with_maximized(true);
    if let Some(icon) = window_icon() {
        viewport = viewport.with_icon(std::sync::Arc::new(icon));
    }
    let options = eframe::NativeOptions {
        viewport,
        depth_buffer: 24,
        multisampling: 4,
        renderer: eframe::Renderer::Glow,
        persist_window: true,
        ..Default::default()
    };
    eframe::run_native("Golden Stunstick", options, Box::new(|cc| {
        let mut app = app::GoldenStunstick::new(cc);
        app.apply_args(&std::env::args().collect::<Vec<_>>());
        Ok(Box::new(app))
    }))
}

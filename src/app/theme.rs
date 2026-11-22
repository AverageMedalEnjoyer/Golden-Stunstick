//! Customizable theme (Settings -> Theme), persisted to goldenstunstick_files/settings.txt.

use eframe::egui::{self, Color32, Stroke};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub struct Theme {
    pub border: Color32,
    pub background: Color32,
    pub panel: Color32,
    pub panel_opacity: f32,
    pub text: Color32,
    pub accent: Color32,
    pub viewport_bg: Color32,
    pub bg_image: Option<PathBuf>,
    /// 0..=100 percent
    pub bg_blur: f32,
    /// 0..=100 percent
    pub bg_image_opacity: f32,
    pub border_width: f32,
    pub rounding: f32,
    pub ui_scale: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            border: Color32::from_rgb(232, 210, 122), // soft yellow
            background: Color32::BLACK,
            panel: Color32::from_rgb(16, 15, 12),
            panel_opacity: 92.0,
            text: Color32::from_rgb(236, 230, 210),
            accent: Color32::from_rgb(232, 200, 90),
            viewport_bg: Color32::from_rgb(40, 40, 46),
            bg_image: None,
            bg_blur: 0.0,
            bg_image_opacity: 100.0,
            border_width: 1.0,
            rounding: 4.0,
            ui_scale: 1.0,
        }
    }
}

fn hex(c: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}{:02x}", c.r(), c.g(), c.b(), c.a())
}

fn parse_hex(s: &str) -> Option<Color32> {
    let s = s.trim().trim_start_matches('#');
    let v = |i: usize| u8::from_str_radix(s.get(i..i + 2)?, 16).ok();
    match s.len() {
        6 => Some(Color32::from_rgb(v(0)?, v(2)?, v(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(v(0)?, v(2)?, v(4)?, v(6)?)),
        _ => None,
    }
}

impl Theme {
    pub fn load(path: &Path) -> Theme {
        let mut t = Theme::default();
        let Ok(text) = std::fs::read_to_string(path) else { return t };
        let text = text.trim_start_matches('\u{feff}');
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let f = || v.parse::<f32>().ok();
            match k {
                "border_color" => t.border = parse_hex(v).unwrap_or(t.border),
                "background_color" => t.background = parse_hex(v).unwrap_or(t.background),
                "panel_color" => t.panel = parse_hex(v).unwrap_or(t.panel),
                "panel_opacity" => t.panel_opacity = f().unwrap_or(t.panel_opacity).clamp(0.0, 100.0),
                "text_color" => t.text = parse_hex(v).unwrap_or(t.text),
                "accent_color" => t.accent = parse_hex(v).unwrap_or(t.accent),
                "viewport_color" => t.viewport_bg = parse_hex(v).unwrap_or(t.viewport_bg),
                "background_image" => t.bg_image = if v.is_empty() { None } else { Some(PathBuf::from(v)) },
                "background_blur" => t.bg_blur = f().unwrap_or(0.0).clamp(0.0, 100.0),
                "background_image_opacity" => t.bg_image_opacity = f().unwrap_or(100.0).clamp(0.0, 100.0),
                "border_width" => t.border_width = f().unwrap_or(1.0).clamp(0.0, 6.0),
                "rounding" => t.rounding = f().unwrap_or(4.0).clamp(0.0, 16.0),
                "ui_scale" => t.ui_scale = f().unwrap_or(1.0).clamp(0.6, 2.5),
                _ => {}
            }
        }
        t
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let s = format!(
            "// Golden Stunstick settings\nborder_color = {}\nbackground_color = {}\npanel_color = {}\npanel_opacity = {}\ntext_color = {}\naccent_color = {}\nviewport_color = {}\nbackground_image = {}\nbackground_blur = {}\nbackground_image_opacity = {}\nborder_width = {}\nrounding = {}\nui_scale = {}\n",
            hex(self.border),
            hex(self.background),
            hex(self.panel),
            self.panel_opacity,
            hex(self.text),
            hex(self.accent),
            hex(self.viewport_bg),
            self.bg_image.as_ref().map(|p| p.display().to_string()).unwrap_or_default(),
            self.bg_blur,
            self.bg_image_opacity,
            self.border_width,
            self.rounding,
            self.ui_scale
        );
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::write(path, s)
    }

    pub fn panel_fill(&self) -> Color32 {
        let a = (self.panel_opacity / 100.0 * 255.0) as u8;
        Color32::from_rgba_unmultiplied(self.panel.r(), self.panel.g(), self.panel.b(), a)
    }

    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        let border = Stroke::new(self.border_width, self.border);
        let soft = Stroke::new(self.border_width, self.border.gamma_multiply(0.55));
        let r = egui::CornerRadius::same(self.rounding.round() as u8);
        v.override_text_color = Some(self.text);
        v.panel_fill = self.panel_fill();
        v.window_fill = Color32::from_rgba_unmultiplied(self.panel.r(), self.panel.g(), self.panel.b(), 250);
        v.window_stroke = border;
        v.window_corner_radius = r;
        v.menu_corner_radius = r;
        v.extreme_bg_color = Color32::from_rgba_unmultiplied(0, 0, 0, 200);
        v.faint_bg_color = Color32::from_rgba_unmultiplied(255, 255, 255, 8);
        v.selection.bg_fill = self.accent.gamma_multiply(0.45);
        v.selection.stroke = Stroke::new(1.0, self.accent);
        v.hyperlink_color = self.accent;
        let w = &mut v.widgets;
        for wv in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            wv.corner_radius = r;
        }
        w.noninteractive.bg_stroke = soft;
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        w.inactive.bg_stroke = soft;
        w.inactive.weak_bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 14);
        w.inactive.bg_fill = Color32::from_rgba_unmultiplied(255, 255, 255, 20);
        w.hovered.bg_stroke = border;
        w.hovered.weak_bg_fill = self.accent.gamma_multiply(0.18);
        w.hovered.bg_fill = self.accent.gamma_multiply(0.25);
        w.active.bg_stroke = Stroke::new(self.border_width.max(1.0), self.accent);
        w.active.weak_bg_fill = self.accent.gamma_multiply(0.35);
        w.active.bg_fill = self.accent.gamma_multiply(0.4);
        w.open.bg_stroke = border;
        ctx.set_visuals(v);
        ctx.set_zoom_factor(self.ui_scale);
    }
}

/// A decoded + blurred background image, cached by (path, blur).
pub struct BackgroundCache {
    key: Option<(PathBuf, u32)>,
    pub texture: Option<egui::TextureHandle>,
    pub error: Option<String>,
}

impl BackgroundCache {
    pub fn new() -> Self {
        Self { key: None, texture: None, error: None }
    }

    pub fn update(&mut self, ctx: &egui::Context, theme: &Theme) {
        let Some(path) = &theme.bg_image else {
            self.key = None;
            self.texture = None;
            self.error = None;
            return;
        };
        let key = (path.clone(), theme.bg_blur.round() as u32);
        if self.key.as_ref() == Some(&key) {
            return;
        }
        self.key = Some(key);
        self.texture = None;
        self.error = None;
        match image::open(path) {
            Ok(img) => {
                let img = img.thumbnail(1600, 1600).to_rgba8();
                let sigma = theme.bg_blur / 100.0 * 40.0;
                let img = if sigma > 0.3 { image::imageops::fast_blur(&img, sigma) } else { img };
                let size = [img.width() as usize, img.height() as usize];
                let ci = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
                self.texture = Some(ctx.load_texture("gs_background", ci, egui::TextureOptions::LINEAR));
            }
            Err(e) => self.error = Some(format!("Could not load background image: {e}")),
        }
    }

    /// Paint background colour + image (cover fit) on the background layer.
    pub fn paint(&self, ctx: &egui::Context, theme: &Theme) {
        let rect = ctx.viewport_rect();
        let painter = ctx.layer_painter(egui::LayerId::background());
        painter.rect_filled(rect, 0.0, theme.background);
        if let Some(tex) = &self.texture {
            let [w, h] = tex.size();
            let (iw, ih) = (w as f32, h as f32);
            let scale = (rect.width() / iw).max(rect.height() / ih);
            let size = egui::vec2(iw * scale, ih * scale);
            let r = egui::Rect::from_center_size(rect.center(), size);
            let a = (theme.bg_image_opacity / 100.0 * 255.0) as u8;
            painter.image(tex.id(), r, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::from_white_alpha(a));
        }
    }
}

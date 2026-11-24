//! OpenGL model renderer drawn inside egui through a glow paint callback.

use eframe::egui;
use eframe::egui_glow;
use eframe::glow::{self, HasContext};
use glam::{Mat4, Vec3};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct GpuVertex {
    pub pos: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderMode {
    Wireframe,
    Flat,
    Smooth,
    Textured,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Blend {
    Opaque,
    AlphaTest,
    Translucent,
    Additive,
}

#[derive(Clone, Debug)]
pub struct DrawCall {
    pub first: u32,
    pub count: u32,
    pub texture: Option<String>,
    pub blend: Blend,
    pub alpha_ref: f32,
    pub cull: bool,
    pub tint: [f32; 4],
    pub selfillum: bool,
    /// eyeball iris projected from world position (Eyes / EyeRefract shaders)
    pub iris: Option<IrisDraw>,
}

#[derive(Clone, Debug)]
pub struct IrisDraw {
    pub texture: String,
    /// iris texture u = dot(u.xyz, world_pos) + u.w (same for v)
    pub u: [f32; 4],
    pub v: [f32; 4],
    /// true: the iris replaces the base (EyeRefract); false: blended over it by iris alpha (Eyes)
    pub replace: bool,
    /// EyeRefract `$dilation` (0.5 leaves the pupil as painted)
    pub dilation: f32,
}

/// Everything needed to draw one viewport for one frame.
#[derive(Default)]
pub struct FrameData {
    pub view_proj: Mat4,
    pub clear: Option<[f32; 4]>,
    pub vertices: Vec<GpuVertex>,
    pub indices: Vec<u32>,
    pub draws: Vec<DrawCall>,
    pub lines: Vec<LineVertex>,
    /// drawn without depth test (bones, selection)
    pub overlay_lines: Vec<LineVertex>,
    pub light_dir: Vec3,
    pub ambient: f32,
    pub mode: Option<RenderMode>,
    pub wire_overlay: bool,
    pub backface_cull: bool,
    pub screenshot: Option<PathBuf>,
}

pub struct TextureUpload {
    pub key: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Arc<Vec<u8>>,
    pub clamp: bool,
}

pub struct Renderer {
    mesh_prog: glow::Program,
    line_prog: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    line_vao: glow::VertexArray,
    line_vbo: glow::Buffer,
    textures: HashMap<String, glow::Texture>,
    white: glow::Texture,
    pending: Vec<TextureUpload>,
    pub last_message: Option<String>,
}

pub type SharedRenderer = Arc<Mutex<Option<Renderer>>>;

unsafe fn compile(gl: &glow::Context, header: &str, vs: &str, fs: &str) -> Result<glow::Program, String> {
    unsafe {
        let prog = gl.create_program()?;
        let mut shaders = Vec::new();
        for (kind, src) in [(glow::VERTEX_SHADER, vs), (glow::FRAGMENT_SHADER, fs)] {
            let sh = gl.create_shader(kind)?;
            gl.shader_source(sh, &format!("{header}\n{src}"));
            gl.compile_shader(sh);
            if !gl.get_shader_compile_status(sh) {
                return Err(format!("shader compile error: {}", gl.get_shader_info_log(sh)));
            }
            gl.attach_shader(prog, sh);
            shaders.push(sh);
        }
        gl.link_program(prog);
        if !gl.get_program_link_status(prog) {
            return Err(format!("shader link error: {}", gl.get_program_info_log(prog)));
        }
        for sh in shaders {
            gl.detach_shader(prog, sh);
            gl.delete_shader(sh);
        }
        Ok(prog)
    }
}

const MESH_VS: &str = r#"
in vec3 a_pos; in vec3 a_normal; in vec2 a_uv;
uniform mat4 u_mvp;
out vec3 v_normal; out vec2 v_uv; out vec3 v_pos;
void main() { v_normal = a_normal; v_uv = a_uv; v_pos = a_pos; gl_Position = u_mvp * vec4(a_pos, 1.0); }
"#;

const MESH_FS: &str = r#"
in vec3 v_normal; in vec2 v_uv; in vec3 v_pos;
uniform sampler2D u_tex; uniform int u_mode; uniform vec3 u_light; uniform vec4 u_tint;
uniform float u_alpha_ref; uniform float u_ambient; uniform int u_selfillum; uniform int u_wire;
uniform int u_iris; uniform sampler2D u_iris_tex; uniform vec4 u_iris_u; uniform vec4 u_iris_v; uniform float u_iris_dil;
out vec4 frag;
void main() {
    if (u_wire == 1) { frag = u_tint; return; }
    vec3 n = normalize(v_normal);
    if (u_mode == 1) { n = normalize(cross(dFdx(v_pos), dFdy(v_pos))); }
    float d = dot(n, normalize(u_light));
    float hl = d * 0.5 + 0.5;
    float light = u_ambient + (1.0 - u_ambient) * hl * hl;
    vec4 base = vec4(0.82, 0.82, 0.82, 1.0);
    if (u_mode == 3) { base = texture(u_tex, v_uv); }
    if (u_mode == 3 && u_iris != 0) {
        vec2 iuv = vec2(dot(u_iris_u.xyz, v_pos) + u_iris_u.w, dot(u_iris_v.xyz, v_pos) + u_iris_v.w);
        if (u_iris == 2) {
            // EyeRefract (eye_refract_ps2x.fxc): the iris texture spans twice the projection
            // ("sphere uv"), then the pupil is dilated about the centre (iris radius 0.2)
            vec2 c = iuv * 0.5 - 0.25;
            float f = clamp(length(c) / 0.2, 0.0, 1.0);
            c *= mix(1.0, f, clamp(u_iris_dil, 0.0, 1.0) * 2.5 - 1.25);
            iuv = c + 0.5;
        }
        vec4 iris = texture(u_iris_tex, iuv);
        if (u_iris == 2) { base = vec4(iris.rgb, 1.0); } else { base.rgb = mix(base.rgb, iris.rgb, iris.a); }
    }
    if (u_alpha_ref > 0.0 && base.a < u_alpha_ref) discard;
    vec3 col = base.rgb * u_tint.rgb;
    if (u_selfillum == 0) { col *= light; }
    frag = vec4(col, base.a * u_tint.a);
}
"#;

const LINE_VS: &str = r#"
in vec3 a_pos; in vec4 a_color; uniform mat4 u_mvp; out vec4 v_color;
void main() { v_color = a_color; gl_Position = u_mvp * vec4(a_pos, 1.0); }
"#;

const LINE_FS: &str = r#"
in vec4 v_color; out vec4 frag; void main() { frag = v_color; }
"#;

impl Renderer {
    pub fn new(gl: &glow::Context) -> Result<Renderer, String> {
        unsafe {
            let sv = egui_glow::ShaderVersion::get(gl);
            let mut header = sv.version_declaration().to_string();
            if header.contains(" es") {
                header.push_str("\nprecision highp float;\nprecision highp int;");
            }
            let mesh_prog = compile(gl, &header, MESH_VS, MESH_FS)?;
            let line_prog = compile(gl, &header, LINE_VS, LINE_FS)?;
            let vao = gl.create_vertex_array()?;
            let vbo = gl.create_buffer()?;
            let ebo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
            let stride = std::mem::size_of::<GpuVertex>() as i32;
            for (name, size, off) in [("a_pos", 3, 0), ("a_normal", 3, 12), ("a_uv", 2, 24)] {
                if let Some(loc) = gl.get_attrib_location(mesh_prog, name) {
                    gl.enable_vertex_attrib_array(loc);
                    gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, stride, off);
                }
            }
            let line_vao = gl.create_vertex_array()?;
            let line_vbo = gl.create_buffer()?;
            gl.bind_vertex_array(Some(line_vao));
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(line_vbo));
            let lstride = std::mem::size_of::<LineVertex>() as i32;
            for (name, size, off) in [("a_pos", 3, 0), ("a_color", 4, 12)] {
                if let Some(loc) = gl.get_attrib_location(line_prog, name) {
                    gl.enable_vertex_attrib_array(loc);
                    gl.vertex_attrib_pointer_f32(loc, size, glow::FLOAT, false, lstride, off);
                }
            }
            gl.bind_vertex_array(None);
            let white = Self::make_texture(gl, 1, 1, &[255, 255, 255, 255], false)?;
            Ok(Renderer { mesh_prog, line_prog, vao, vbo, ebo, line_vao, line_vbo, textures: HashMap::new(), white, pending: Vec::new(), last_message: None })
        }
    }

    unsafe fn make_texture(gl: &glow::Context, w: u32, h: u32, rgba: &[u8], clamp: bool) -> Result<glow::Texture, String> {
        unsafe {
            let t = gl.create_texture()?;
            gl.bind_texture(glow::TEXTURE_2D, Some(t));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(rgba)));
            gl.generate_mipmap(glow::TEXTURE_2D);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            let wrap = if clamp { glow::CLAMP_TO_EDGE } else { glow::REPEAT } as i32;
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, wrap);
            gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, wrap);
            Ok(t)
        }
    }

    pub fn has_texture(&self, key: &str) -> bool {
        self.textures.contains_key(key) || self.pending.iter().any(|p| p.key == key)
    }

    pub fn queue_upload(&mut self, up: TextureUpload) {
        if !self.has_texture(&up.key) {
            self.pending.push(up);
        }
    }

    pub fn destroy(&mut self, gl: &glow::Context) {
        unsafe {
            gl.delete_program(self.mesh_prog);
            gl.delete_program(self.line_prog);
            gl.delete_vertex_array(self.vao);
            gl.delete_vertex_array(self.line_vao);
            gl.delete_buffer(self.vbo);
            gl.delete_buffer(self.ebo);
            gl.delete_buffer(self.line_vbo);
            for (_, t) in self.textures.drain() {
                gl.delete_texture(t);
            }
            gl.delete_texture(self.white);
        }
    }

    fn as_bytes<T>(v: &[T]) -> &[u8] {
        unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, std::mem::size_of_val(v)) }
    }

    pub fn paint(&mut self, gl: &glow::Context, info: &egui::PaintCallbackInfo, f: &FrameData) {
        unsafe {
            for up in std::mem::take(&mut self.pending) {
                if let Ok(t) = Self::make_texture(gl, up.width, up.height, &up.rgba, up.clamp) {
                    self.textures.insert(up.key, t);
                }
            }
            let vp = info.viewport_in_pixels();
            let clip = info.clip_rect_in_pixels();
            gl.viewport(vp.left_px, vp.from_bottom_px, vp.width_px, vp.height_px);
            gl.enable(glow::SCISSOR_TEST);
            gl.scissor(clip.left_px, clip.from_bottom_px, clip.width_px, clip.height_px);
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.depth_mask(true);
            gl.clear_depth_f32(1.0);
            if let Some(c) = f.clear {
                gl.clear_color(c[0], c[1], c[2], c[3]);
                gl.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            } else {
                gl.clear(glow::DEPTH_BUFFER_BIT);
            }
            let mvp = f.view_proj.to_cols_array();

            // ---- meshes
            if !f.vertices.is_empty() && !f.indices.is_empty() {
                gl.use_program(Some(self.mesh_prog));
                gl.bind_vertex_array(Some(self.vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, Self::as_bytes(&f.vertices), glow::STREAM_DRAW);
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(self.ebo));
                gl.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, Self::as_bytes(&f.indices), glow::STREAM_DRAW);
                let u = |n: &str| gl.get_uniform_location(self.mesh_prog, n);
                gl.uniform_matrix_4_f32_slice(u("u_mvp").as_ref(), false, &mvp);
                let l = f.light_dir.normalize_or(Vec3::Z);
                gl.uniform_3_f32(u("u_light").as_ref(), l.x, l.y, l.z);
                gl.uniform_1_f32(u("u_ambient").as_ref(), f.ambient);
                gl.uniform_1_i32(u("u_tex").as_ref(), 0);
                gl.uniform_1_i32(u("u_iris_tex").as_ref(), 1);
                gl.active_texture(glow::TEXTURE1);
                gl.bind_texture(glow::TEXTURE_2D, Some(self.white));
                gl.active_texture(glow::TEXTURE0);
                let mode = f.mode.unwrap_or(RenderMode::Textured);
                let mode_i = match mode {
                    RenderMode::Wireframe => 0,
                    RenderMode::Flat => 1,
                    RenderMode::Smooth => 2,
                    RenderMode::Textured => 3,
                };
                let wire_only = mode == RenderMode::Wireframe;
                // opaque first, then translucent
                for pass in 0..2 {
                    for d in &f.draws {
                        let translucent = matches!(d.blend, Blend::Translucent | Blend::Additive);
                        if (pass == 0) == translucent {
                            continue;
                        }
                        if d.cull && f.backface_cull {
                            gl.enable(glow::CULL_FACE);
                            gl.cull_face(glow::BACK);
                            gl.front_face(glow::CW);
                        } else {
                            gl.disable(glow::CULL_FACE);
                        }
                        match d.blend {
                            Blend::Translucent if !wire_only => {
                                gl.enable(glow::BLEND);
                                gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                                gl.depth_mask(false);
                            }
                            Blend::Additive if !wire_only => {
                                gl.enable(glow::BLEND);
                                gl.blend_func(glow::SRC_ALPHA, glow::ONE);
                                gl.depth_mask(false);
                            }
                            _ => {
                                gl.disable(glow::BLEND);
                                gl.depth_mask(true);
                            }
                        }
                        let tex = d.texture.as_ref().and_then(|k| self.textures.get(k)).copied().unwrap_or(self.white);
                        gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                        match d.iris.as_ref().and_then(|i| self.textures.get(&i.texture).map(|t| (i, *t))) {
                            Some((i, t)) => {
                                gl.active_texture(glow::TEXTURE1);
                                gl.bind_texture(glow::TEXTURE_2D, Some(t));
                                gl.active_texture(glow::TEXTURE0);
                                gl.uniform_1_i32(u("u_iris").as_ref(), if i.replace { 2 } else { 1 });
                                gl.uniform_4_f32(u("u_iris_u").as_ref(), i.u[0], i.u[1], i.u[2], i.u[3]);
                                gl.uniform_4_f32(u("u_iris_v").as_ref(), i.v[0], i.v[1], i.v[2], i.v[3]);
                                gl.uniform_1_f32(u("u_iris_dil").as_ref(), i.dilation);
                            }
                            None => gl.uniform_1_i32(u("u_iris").as_ref(), 0),
                        }
                        gl.uniform_1_i32(u("u_mode").as_ref(), mode_i);
                        gl.uniform_1_f32(u("u_alpha_ref").as_ref(), if mode == RenderMode::Textured { d.alpha_ref } else { 0.0 });
                        gl.uniform_1_i32(u("u_selfillum").as_ref(), d.selfillum as i32);
                        if wire_only {
                            gl.uniform_1_i32(u("u_wire").as_ref(), 1);
                            gl.uniform_4_f32(u("u_tint").as_ref(), 0.85, 0.85, 0.85, 1.0);
                            gl.polygon_mode(glow::FRONT_AND_BACK, glow::LINE);
                        } else {
                            gl.uniform_1_i32(u("u_wire").as_ref(), 0);
                            gl.uniform_4_f32(u("u_tint").as_ref(), d.tint[0], d.tint[1], d.tint[2], d.tint[3]);
                        }
                        gl.draw_elements(glow::TRIANGLES, d.count as i32, glow::UNSIGNED_INT, (d.first * 4) as i32);
                        if wire_only {
                            gl.polygon_mode(glow::FRONT_AND_BACK, glow::FILL);
                        }
                    }
                }
                gl.depth_mask(true);
                gl.disable(glow::BLEND);
                if f.wire_overlay && !wire_only {
                    gl.enable(glow::POLYGON_OFFSET_LINE);
                    gl.polygon_offset(-1.0, -1.0);
                    gl.polygon_mode(glow::FRONT_AND_BACK, glow::LINE);
                    gl.uniform_1_i32(u("u_wire").as_ref(), 1);
                    gl.uniform_4_f32(u("u_tint").as_ref(), 0.1, 0.1, 0.1, 1.0);
                    for d in &f.draws {
                        gl.draw_elements(glow::TRIANGLES, d.count as i32, glow::UNSIGNED_INT, (d.first * 4) as i32);
                    }
                    gl.polygon_mode(glow::FRONT_AND_BACK, glow::FILL);
                    gl.disable(glow::POLYGON_OFFSET_LINE);
                }
                gl.disable(glow::CULL_FACE);
            }

            // ---- lines
            for (lines, depth) in [(&f.lines, true), (&f.overlay_lines, false)] {
                if lines.is_empty() {
                    continue;
                }
                if depth {
                    gl.enable(glow::DEPTH_TEST);
                } else {
                    gl.disable(glow::DEPTH_TEST);
                }
                gl.enable(glow::BLEND);
                gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
                gl.use_program(Some(self.line_prog));
                gl.bind_vertex_array(Some(self.line_vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.line_vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, Self::as_bytes(lines), glow::STREAM_DRAW);
                gl.uniform_matrix_4_f32_slice(gl.get_uniform_location(self.line_prog, "u_mvp").as_ref(), false, &mvp);
                gl.draw_arrays(glow::LINES, 0, lines.len() as i32);
            }

            // ---- screenshot
            if let Some(path) = &f.screenshot {
                let (w, h) = (vp.width_px.max(1), vp.height_px.max(1));
                let mut buf = vec![0u8; (w * h * 4) as usize];
                gl.read_pixels(vp.left_px, vp.from_bottom_px, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut buf)));
                let row = (w * 4) as usize;
                let mut flipped = vec![0u8; buf.len()];
                for y in 0..h as usize {
                    flipped[y * row..(y + 1) * row].copy_from_slice(&buf[(h as usize - 1 - y) * row..(h as usize - y) * row]);
                }
                for px in flipped.chunks_mut(4) {
                    px[3] = 255;
                }
                self.last_message = Some(match image::save_buffer(path, &flipped, w as u32, h as u32, image::ColorType::Rgba8) {
                    Ok(()) => format!("Screenshot saved to {}", path.display()),
                    Err(e) => format!("Screenshot failed: {e}"),
                });
            }

            // restore state egui expects
            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.disable(glow::DEPTH_TEST);
            gl.disable(glow::CULL_FACE);
            gl.depth_mask(true);
            gl.enable(glow::BLEND);
            gl.blend_func_separate(glow::ONE, glow::ONE_MINUS_SRC_ALPHA, glow::ONE_MINUS_DST_ALPHA, glow::ONE);
        }
    }
}

/// Add a paint callback that draws `frame` into `rect`.
pub fn paint_viewport(ui: &egui::Ui, rect: egui::Rect, renderer: SharedRenderer, frame: FrameData) {
    let frame = Arc::new(frame);
    let cb = egui_glow::CallbackFn::new(move |info, painter| {
        let gl = painter.gl();
        let mut guard = renderer.lock().unwrap_or_else(|p| p.into_inner());
        if guard.is_none() {
            match Renderer::new(gl) {
                Ok(r) => *guard = Some(r),
                Err(e) => {
                    eprintln!("renderer init failed: {e}");
                    return;
                }
            }
        }
        if let Some(r) = guard.as_mut() {
            r.paint(gl, &info, &frame);
        }
    });
    ui.painter().add(egui::PaintCallback { rect, callback: Arc::new(cb) });
}

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct OrbitCamera {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub fov: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        OrbitCamera { target: Vec3::new(0.0, 0.0, 36.0), yaw: 180f32.to_radians(), pitch: 10f32.to_radians(), distance: 120.0, fov: 65.0 }
    }
}

impl OrbitCamera {
    pub fn eye(&self) -> Vec3 {
        let dir = Vec3::new(self.pitch.cos() * self.yaw.cos(), self.pitch.cos() * self.yaw.sin(), self.pitch.sin());
        self.target + dir * self.distance
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = glam::camera::rh::view::look_at_mat4(self.eye(), self.target, Vec3::Z);
        let near = (self.distance * 0.01).clamp(0.05, 10.0);
        let proj = glam::camera::rh::proj::opengl::perspective(self.fov.to_radians(), aspect.max(0.01), near, near * 100_000.0);
        proj * view
    }

    /// Frame a bounding box.
    pub fn frame_bounds(&mut self, min: Vec3, max: Vec3) {
        self.target = (min + max) * 0.5;
        let radius = ((max - min).length() * 0.5).max(1.0);
        self.distance = radius / (self.fov.to_radians() * 0.5).tan() * 1.1;
    }

    /// Standard model-viewer mouse controls. Returns true if the camera changed.
    pub fn handle_input(&mut self, resp: &egui::Response, ui: &egui::Ui) -> bool {
        let mut changed = false;
        let (shift, ctrl) = ui.input(|i| (i.modifiers.shift, i.modifiers.ctrl));
        if resp.dragged_by(egui::PointerButton::Primary) && !shift && !ctrl {
            let d = resp.drag_delta();
            self.yaw -= d.x * 0.01;
            self.pitch = (self.pitch + d.y * 0.01).clamp(-1.55, 1.55);
            changed = true;
        }
        if resp.dragged_by(egui::PointerButton::Middle) || (resp.dragged_by(egui::PointerButton::Primary) && shift) {
            let d = resp.drag_delta();
            let eye = self.eye();
            let fwd = (self.target - eye).normalize_or(Vec3::X);
            let right = fwd.cross(Vec3::Z).normalize_or(Vec3::Y);
            let up = right.cross(fwd);
            let s = self.distance * 0.0015;
            self.target += -right * d.x * s + up * d.y * s;
            changed = true;
        }
        if resp.dragged_by(egui::PointerButton::Secondary) {
            let d = resp.drag_delta();
            self.distance = (self.distance * (1.0 + d.y * 0.01)).clamp(0.5, 100_000.0);
            changed = true;
        }
        if resp.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.distance = (self.distance * (1.0 - scroll * 0.0015)).clamp(0.5, 100_000.0);
                changed = true;
            }
        }
        changed
    }
}

/// Free-fly camera (Animator): WASD / QE flying with smooth acceleration, right-drag to look,
/// left-drag to orbit the focus point, middle-drag to pan, smooth scroll dolly and smooth
/// "fly to" framing.
#[derive(Clone, Debug)]
pub struct FlyCamera {
    pub eye: Vec3,
    /// heading of the view direction (radians, 0 = +X)
    pub yaw: f32,
    /// elevation of the view direction (radians, positive = looking up)
    pub pitch: f32,
    pub fov: f32,
    /// distance from the eye to the orbit / focus point in front of it
    pub focus: f32,
    /// flying speed in units per second (before Shift / scroll multipliers)
    pub speed: f32,
    /// scroll-adjusted speed multiplier while flying
    pub speed_mul: f32,
    pub near: f32,
    vel: Vec3,
    dolly: f32,
    fly_to: Option<(Vec3, f32)>,
}

impl Default for FlyCamera {
    fn default() -> Self {
        let mut c = FlyCamera { eye: Vec3::ZERO, yaw: 180f32.to_radians(), pitch: -10f32.to_radians(), fov: 65.0, focus: 120.0, speed: 80.0, speed_mul: 1.0, near: 0.2, vel: Vec3::ZERO, dolly: 0.0, fly_to: None };
        c.eye = Vec3::new(0.0, 0.0, 36.0) - c.forward() * c.focus;
        c
    }
}

impl FlyCamera {
    pub fn forward(&self) -> Vec3 {
        Vec3::new(self.pitch.cos() * self.yaw.cos(), self.pitch.cos() * self.yaw.sin(), self.pitch.sin())
    }
    pub fn right(&self) -> Vec3 {
        self.forward().cross(Vec3::Z).normalize_or(Vec3::Y)
    }
    pub fn up(&self) -> Vec3 {
        self.right().cross(self.forward()).normalize_or(Vec3::Z)
    }
    /// The orbit / focus point.
    pub fn target(&self) -> Vec3 {
        self.eye + self.forward() * self.focus
    }

    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = glam::camera::rh::view::look_at_mat4(self.eye, self.eye + self.forward(), Vec3::Z);
        let proj = glam::camera::rh::proj::opengl::perspective(self.fov.to_radians(), aspect.max(0.01), self.near, self.near * 200_000.0);
        proj * view
    }

    /// World units per screen pixel at `depth` in front of the camera.
    pub fn units_per_pixel(&self, depth: f32, viewport_height: f32) -> f32 {
        2.0 * depth.max(self.near) * (self.fov.to_radians() * 0.5).tan() / viewport_height.max(1.0)
    }

    /// Frame a bounding box (instantly); also sets the flying speed and near plane from its size.
    pub fn frame_bounds(&mut self, min: Vec3, max: Vec3) {
        let radius = ((max - min).length() * 0.5).max(1.0);
        self.focus = radius / (self.fov.to_radians() * 0.5).tan() * 1.1;
        self.eye = (min + max) * 0.5 - self.forward() * self.focus;
        self.speed = radius * 1.2;
        self.near = (radius * 0.002).clamp(0.01, 4.0);
        self.vel = Vec3::ZERO;
        self.fly_to = None;
        self.dolly = 0.0;
    }

    /// Smoothly fly so that `point` is centred `distance` in front of the camera.
    pub fn fly_to(&mut self, point: Vec3, distance: f32) {
        self.fly_to = Some((point, distance.max(self.near * 4.0)));
    }

    fn orbit(&mut self, dx: f32, dy: f32) {
        let t = self.target();
        self.yaw -= dx * 0.01;
        self.pitch = (self.pitch - dy * 0.01).clamp(-1.55, 1.55);
        self.eye = t - self.forward() * self.focus;
    }

    /// Mouse + keyboard input. `orbit_ok` is false while the left button is used for something
    /// else (posing a bone). Returns true while the camera is still moving (repaint needed).
    pub fn handle_input(&mut self, resp: &egui::Response, ui: &egui::Ui, orbit_ok: bool) -> bool {
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
        let (shift, ctrl, alt) = ui.input(|i| (i.modifiers.shift, i.modifiers.ctrl, i.modifiers.alt));
        let mut user = false;
        // right-drag: look around
        let looking = resp.dragged_by(egui::PointerButton::Secondary);
        if looking {
            let d = resp.drag_delta();
            self.yaw -= d.x * 0.005;
            self.pitch = (self.pitch - d.y * 0.005).clamp(-1.55, 1.55);
            user = true;
        }
        if orbit_ok && resp.dragged_by(egui::PointerButton::Primary) && !shift && !ctrl {
            let d = resp.drag_delta();
            self.orbit(d.x, d.y);
            user = true;
        }
        if resp.dragged_by(egui::PointerButton::Middle) || (orbit_ok && resp.dragged_by(egui::PointerButton::Primary) && shift) {
            let d = resp.drag_delta();
            let s = self.units_per_pixel(self.focus, resp.rect.height());
            self.eye += (-self.right() * d.x + self.up() * d.y) * s;
            user = true;
        }
        // keyboard flying while the pointer is over the viewport (or while looking)
        let keys_ok = (resp.hovered() || looking) && !ui.ctx().egui_wants_keyboard_input() && !ctrl;
        let mut dir = Vec3::ZERO;
        if keys_ok {
            ui.input(|i| {
                let k = |key| i.key_down(key);
                if k(egui::Key::W) || k(egui::Key::ArrowUp) {
                    dir += self.forward();
                }
                if k(egui::Key::S) || k(egui::Key::ArrowDown) {
                    dir -= self.forward();
                }
                if k(egui::Key::D) || k(egui::Key::ArrowRight) {
                    dir += self.right();
                }
                if k(egui::Key::A) || k(egui::Key::ArrowLeft) {
                    dir -= self.right();
                }
                if k(egui::Key::E) {
                    dir += Vec3::Z;
                }
                if k(egui::Key::Q) {
                    dir -= Vec3::Z;
                }
            });
        }
        let mul = self.speed_mul * if shift { 3.0 } else if alt { 0.25 } else { 1.0 };
        let want = dir.normalize_or_zero() * self.speed * mul;
        let k = 1.0 - (-dt * 10.0).exp();
        self.vel = self.vel.lerp(want, k);
        if self.vel.length() < self.speed * 0.002 && want == Vec3::ZERO {
            self.vel = Vec3::ZERO;
        }
        if self.vel != Vec3::ZERO {
            self.eye += self.vel * dt;
            user = true;
        }
        // scroll: change the flying speed while looking, otherwise a smooth dolly toward the focus
        if resp.hovered() || looking {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                if looking || dir != Vec3::ZERO {
                    self.speed_mul = (self.speed_mul * (1.0 + scroll * 0.002)).clamp(0.05, 20.0);
                } else {
                    self.dolly += scroll * 0.0015 * self.focus.max(self.near * 10.0);
                }
            }
        }
        if self.dolly.abs() > self.near * 0.01 {
            let step = self.dolly * (1.0 - (-dt * 12.0).exp());
            self.dolly -= step;
            self.eye += self.forward() * step;
            // keep the focus point ahead; pushing through it carries it along
            self.focus = (self.focus - step).max(self.near * 8.0);
            user = true;
        } else {
            self.dolly = 0.0;
        }
        if user {
            self.fly_to = None;
        }
        if let Some((p, dist)) = self.fly_to {
            let goal = p - self.forward() * dist;
            let k = 1.0 - (-dt * 8.0).exp();
            self.eye = self.eye.lerp(goal, k);
            self.focus += (dist - self.focus) * k;
            if (self.eye - goal).length() < self.near * 0.5 {
                self.eye = goal;
                self.focus = dist;
                self.fly_to = None;
            }
            return true;
        }
        user || self.vel != Vec3::ZERO || self.dolly != 0.0 || dir != Vec3::ZERO
    }
}

/// Project a world point into screen space within `rect`.
pub fn project(view_proj: Mat4, rect: egui::Rect, p: Vec3) -> Option<egui::Pos2> {
    let c = view_proj * p.extend(1.0);
    if c.w <= 1e-5 {
        return None;
    }
    let n = c / c.w;
    Some(egui::pos2(rect.left() + (n.x * 0.5 + 0.5) * rect.width(), rect.top() + (1.0 - (n.y * 0.5 + 0.5)) * rect.height()))
}

pub fn line(out: &mut Vec<LineVertex>, a: Vec3, b: Vec3, color: [f32; 4]) {
    out.push(LineVertex { pos: a.to_array(), color });
    out.push(LineVertex { pos: b.to_array(), color });
}

pub fn grid(out: &mut Vec<LineVertex>, size: f32, step: f32) {
    let n = (size / step) as i32;
    for i in -n..=n {
        let t = i as f32 * step;
        let c = if i == 0 { [0.55, 0.55, 0.6, 0.9] } else { [0.35, 0.35, 0.38, 0.55] };
        line(out, Vec3::new(t, -size, 0.0), Vec3::new(t, size, 0.0), c);
        line(out, Vec3::new(-size, t, 0.0), Vec3::new(size, t, 0.0), c);
    }
}

pub fn wire_box(out: &mut Vec<LineVertex>, xf: glam::Affine3A, min: Vec3, max: Vec3, color: [f32; 4]) {
    let c = |i: usize| xf.transform_point3(Vec3::new(if i & 1 != 0 { max.x } else { min.x }, if i & 2 != 0 { max.y } else { min.y }, if i & 4 != 0 { max.z } else { min.z }));
    for (a, b) in [(0, 1), (2, 3), (4, 5), (6, 7), (0, 2), (1, 3), (4, 6), (5, 7), (0, 4), (1, 5), (2, 6), (3, 7)] {
        line(out, c(a), c(b), color);
    }
}

pub fn axes(out: &mut Vec<LineVertex>, xf: glam::Affine3A, size: f32) {
    let o = xf.transform_point3(Vec3::ZERO);
    line(out, o, xf.transform_point3(Vec3::X * size), [1.0, 0.2, 0.2, 1.0]);
    line(out, o, xf.transform_point3(Vec3::Y * size), [0.2, 1.0, 0.2, 1.0]);
    line(out, o, xf.transform_point3(Vec3::Z * size), [0.3, 0.5, 1.0, 1.0]);
}

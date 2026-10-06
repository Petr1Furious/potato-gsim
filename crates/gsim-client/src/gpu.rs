//! The density picture drawn by the graphics card instead of the rasteriser threads: bodies
//! go up as one buffer of points and are added into a floating-point image; a quarter-size
//! copy is blurred into the glow; a last shader tone-maps both onto the screen.
//!
//! Everything here is optional. [`Gpu::new`] fails cleanly on anything it cannot work with
//! (no float render targets, a shader the driver rejects, a backend other than OpenGL), and
//! the caller then keeps using the CPU rasteriser.

use macroquad::miniquad::{
    self, Backend, BlendFactor, BlendState, BufferId, BufferLayout, BufferSource, BufferType, BufferUsage, Equation, FilterMode, PassAction, Pipeline,
    PipelineParams, PrimitiveType, RenderPass, RenderingBackend, ShaderMeta, ShaderSource, TextureFormat, TextureId, TextureParams, TextureWrap,
    UniformBlockLayout, UniformDesc, UniformType, UniformsSource, VertexAttribute, VertexFormat,
};
use macroquad::prelude::*;

/// One body: position in buffer pixels, then its light (colour times brightness).
pub type Vertex = [f32; 5];

const POINT_VERTEX: &str = r#"#version 100
attribute vec2 pos;
attribute vec3 light;
uniform vec4 view;
varying highp vec3 c;
void main() {
    gl_Position = vec4(pos.x * view.x - 1.0, pos.y * view.y - 1.0, 0.0, 1.0);
    gl_PointSize = view.z;
    c = light;
}"#;

const POINT_FRAGMENT: &str = r#"#version 100
precision highp float;
varying highp vec3 c;
void main() {
    gl_FragColor = vec4(c, 1.0);
}"#;

const BLUR_VERTEX: &str = r#"#version 100
attribute vec2 pos;
varying highp vec2 uv;
void main() {
    gl_Position = vec4(pos, 0.0, 1.0);
    uv = pos * 0.5 + 0.5;
}"#;

/// One direction of a 1-2-1 blur.
const BLUR_FRAGMENT: &str = r#"#version 100
precision highp float;
varying highp vec2 uv;
uniform sampler2D tex;
uniform vec4 step;
void main() {
    gl_FragColor = 0.25 * texture2D(tex, uv - step.xy) + 0.5 * texture2D(tex, uv) + 0.25 * texture2D(tex, uv + step.xy);
}"#;

const TONE_VERTEX: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
varying highp vec2 uv;
uniform mat4 Model;
uniform mat4 Projection;
void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    uv = texcoord;
}"#;

/// Compress highlights, lift faint light, lay the result over the background colour.
const TONE_FRAGMENT: &str = r#"#version 100
precision highp float;
varying highp vec2 uv;
uniform sampler2D Texture;
uniform sampler2D Glow;
uniform vec4 Tone;
uniform vec4 Background;
void main() {
    vec3 v = texture2D(Texture, uv).rgb * Tone.x + texture2D(Glow, uv).rgb * Tone.y;
    v = sqrt(v / (1.0 + v));
    gl_FragColor = vec4(Background.rgb + (1.0 - Background.rgb) * v, 1.0);
}"#;

#[repr(C)]
struct Vec4Uniform([f32; 4]);

struct Target {
    texture: TextureId,
    pass: RenderPass,
    w: u32,
    h: u32,
}

impl Target {
    fn new(ctx: &mut dyn RenderingBackend, w: u32, h: u32, filter: FilterMode) -> Self {
        let texture = ctx.new_render_texture(TextureParams {
            width: w,
            height: h,
            format: TextureFormat::RGBA16F,
            min_filter: filter,
            mag_filter: filter,
            wrap: TextureWrap::Clamp,
            ..Default::default()
        });
        Self { texture, pass: ctx.new_render_pass(texture, None), w, h }
    }

    fn delete(&self, ctx: &mut dyn RenderingBackend) {
        ctx.delete_render_pass(self.pass);
        ctx.delete_texture(self.texture);
    }
}

pub struct Gpu {
    points: Pipeline,
    blur: Pipeline,
    tone: Material,
    vertices: BufferId,
    indices: BufferId,
    /// Bodies the two buffers have room for.
    capacity: usize,
    quad: (BufferId, BufferId),
    /// Full-size image, quarter-size glow, and scratch for blurring it.
    targets: Option<[Target; 3]>,
}

fn context() -> &'static mut dyn RenderingBackend {
    // SAFETY: macroquad runs on one thread and hands out its context for exactly this.
    unsafe { get_internal_gl().quad_context }
}

fn uniforms(names: &[&str]) -> UniformBlockLayout {
    UniformBlockLayout { uniforms: names.iter().map(|n| UniformDesc::new(n, UniformType::Float4)).collect() }
}

impl Gpu {
    pub fn new() -> Result<Self, String> {
        let ctx = context();
        let info = ctx.info();
        if info.backend != Backend::OpenGl {
            return Err("it needs the OpenGL backend".into());
        }
        if info.gl_version_string.starts_with('2') || info.gl_version_string.contains("OpenGL ES 2") {
            return Err(format!("OpenGL {} is too old for floating-point images", info.gl_version_string));
        }
        let shader = |ctx: &mut dyn RenderingBackend, vertex, fragment, images: &[&str], names: &[&str]| {
            let meta = ShaderMeta { images: images.iter().map(|s| s.to_string()).collect(), uniforms: uniforms(names) };
            ctx.new_shader(ShaderSource::Glsl { vertex, fragment }, meta).map_err(|e| format!("a shader did not compile: {e}"))
        };
        let additive = BlendState::new(Equation::Add, BlendFactor::One, BlendFactor::One);
        let point_shader = shader(ctx, POINT_VERTEX, POINT_FRAGMENT, &[], &["view"])?;
        let points = ctx.new_pipeline(
            &[BufferLayout { stride: std::mem::size_of::<Vertex>() as i32, ..Default::default() }],
            &[VertexAttribute::new("pos", VertexFormat::Float2), VertexAttribute::new("light", VertexFormat::Float3)],
            point_shader,
            PipelineParams { primitive_type: PrimitiveType::Points, color_blend: Some(additive), alpha_blend: Some(additive), ..Default::default() },
        );
        let blur_shader = shader(ctx, BLUR_VERTEX, BLUR_FRAGMENT, &["tex"], &["step"])?;
        let blur = ctx.new_pipeline(&[BufferLayout::default()], &[VertexAttribute::new("pos", VertexFormat::Float2)], blur_shader, PipelineParams::default());
        let corners: [f32; 8] = [-1.0, -1.0, 1.0, -1.0, 1.0, 1.0, -1.0, 1.0];
        let order: [u16; 6] = [0, 1, 2, 0, 2, 3];
        let quad = (
            ctx.new_buffer(BufferType::VertexBuffer, BufferUsage::Immutable, BufferSource::slice(&corners)),
            ctx.new_buffer(BufferType::IndexBuffer, BufferUsage::Immutable, BufferSource::slice(&order)),
        );
        let tone = load_material(
            macroquad::prelude::ShaderSource::Glsl { vertex: TONE_VERTEX, fragment: TONE_FRAGMENT },
            MaterialParams {
                uniforms: vec![UniformDesc::new("Tone", UniformType::Float4), UniformDesc::new("Background", UniformType::Float4)],
                textures: vec!["Glow".to_string()],
                ..Default::default()
            },
        )
        .map_err(|e| format!("the tone-mapping shader did not compile: {e}"))?;
        // Without this a vertex shader's point size is ignored on desktop OpenGL.
        // SAFETY: plain state change on the thread that owns the GL context.
        unsafe { miniquad::gl::glEnable(miniquad::gl::GL_PROGRAM_POINT_SIZE) };
        let (vertices, indices) = Self::buffers(ctx, 1 << 16);
        let mut gpu = Self { points, blur, tone, vertices, indices, capacity: 1 << 16, quad, targets: None };
        // Try the images now: a driver without float render targets shows here, not as a
        // black screen later.
        gpu.resize(ctx, 64, 64, 16, 16);
        let complete = gpu.targets.as_ref().unwrap().iter().all(|t| {
            ctx.begin_pass(Some(t.pass), PassAction::Nothing);
            // SAFETY: reads the status of the framebuffer just bound.
            let status = unsafe { miniquad::gl::glCheckFramebufferStatus(miniquad::gl::GL_FRAMEBUFFER) };
            ctx.end_render_pass();
            status == miniquad::gl::GL_FRAMEBUFFER_COMPLETE
        });
        if !complete {
            gpu.delete();
            return Err("this graphics driver cannot draw into floating-point images".into());
        }
        Ok(gpu)
    }

    fn buffers(ctx: &mut dyn RenderingBackend, capacity: usize) -> (BufferId, BufferId) {
        // Points are drawn through an index buffer like everything else: 0, 1, 2, ...
        let order: Vec<u32> = (0..capacity as u32).collect();
        (
            ctx.new_buffer(BufferType::VertexBuffer, BufferUsage::Stream, BufferSource::empty::<Vertex>(capacity)),
            ctx.new_buffer(BufferType::IndexBuffer, BufferUsage::Immutable, BufferSource::slice(&order)),
        )
    }

    fn resize(&mut self, ctx: &mut dyn RenderingBackend, w: u32, h: u32, gw: u32, gh: u32) {
        if self.targets.as_ref().is_some_and(|t| (t[0].w, t[0].h, t[1].w, t[1].h) == (w, h, gw, gh)) {
            return;
        }
        for t in self.targets.iter().flatten() {
            t.delete(ctx);
        }
        self.targets = Some([Target::new(ctx, w, h, FilterMode::Nearest), Target::new(ctx, gw, gh, FilterMode::Linear), Target::new(ctx, gw, gh, FilterMode::Linear)]);
    }

    /// Free everything this owns on the graphics card.
    pub fn delete(&mut self) {
        let ctx = context();
        for t in self.targets.take().iter().flatten() {
            t.delete(ctx);
        }
        for b in [self.vertices, self.indices, self.quad.0, self.quad.1] {
            ctx.delete_buffer(b);
        }
        ctx.delete_pipeline(self.points);
        ctx.delete_pipeline(self.blur);
    }

    /// Draw `vertices` (positions in a `buffer` sized grid of pixels) over the whole screen.
    /// `exposure` and `glow` scale the image and its halo before tone mapping.
    pub fn draw(&mut self, vertices: &[Vertex], buffer: (usize, usize), glow_size: (usize, usize), exposure: f32, glow: f32, background: Color) {
        let ctx = context();
        // The image has one pixel per physical pixel; each body covers one buffer pixel of it.
        let dpi = screen_dpi_scale();
        let (w, h) = ((screen_width() * dpi).round().clamp(16.0, 8192.0) as u32, (screen_height() * dpi).round().clamp(16.0, 8192.0) as u32);
        self.resize(ctx, w, h, glow_size.0 as u32, glow_size.1 as u32);
        if vertices.len() > self.capacity {
            ctx.delete_buffer(self.vertices);
            ctx.delete_buffer(self.indices);
            self.capacity = vertices.len().next_power_of_two();
            (self.vertices, self.indices) = Self::buffers(ctx, self.capacity);
        }
        ctx.buffer_update(self.vertices, BufferSource::slice(vertices));
        let [image, halo, scratch] = self.targets.as_ref().unwrap();
        let points = miniquad::Bindings { vertex_buffers: vec![self.vertices], index_buffer: self.indices, images: vec![] };
        let to_clip = (2.0 / buffer.0 as f32, 2.0 / buffer.1 as f32);
        // The same points twice: full size, and as single pixels of the small image, where
        // each pixel then holds the light of a whole block of the large one.
        for (target, size) in [(image, w as f32 / buffer.0 as f32), (halo, 1.0)] {
            ctx.begin_pass(Some(target.pass), PassAction::clear_color(0.0, 0.0, 0.0, 0.0));
            ctx.apply_pipeline(&self.points);
            ctx.apply_bindings(&points);
            ctx.apply_uniforms(UniformsSource::table(&Vec4Uniform([to_clip.0, to_clip.1, size.max(1.0), 0.0])));
            ctx.draw(0, vertices.len() as i32, 1);
            ctx.end_render_pass();
        }
        // Two passes of the blur, across then down, back and forth between the small images.
        for _ in 0..2 {
            for (from, to, step) in [(halo, scratch, [1.0 / halo.w as f32, 0.0, 0.0, 0.0]), (scratch, halo, [0.0, 1.0 / halo.h as f32, 0.0, 0.0])] {
                ctx.begin_pass(Some(to.pass), PassAction::Nothing);
                ctx.apply_pipeline(&self.blur);
                ctx.apply_bindings(&miniquad::Bindings { vertex_buffers: vec![self.quad.0], index_buffer: self.quad.1, images: vec![from.texture] });
                ctx.apply_uniforms(UniformsSource::table(&Vec4Uniform(step)));
                ctx.draw(0, 6, 1);
                ctx.end_render_pass();
            }
        }
        self.tone.set_uniform("Tone", [exposure, glow, 0.0, 0.0]);
        self.tone.set_uniform("Background", [background.r, background.g, background.b, 1.0]);
        self.tone.set_texture("Glow", Texture2D::from_miniquad_texture(halo.texture));
        gl_use_material(&self.tone);
        let params = DrawTextureParams { dest_size: Some(vec2(screen_width(), screen_height())), ..Default::default() };
        draw_texture_ex(&Texture2D::from_miniquad_texture(image.texture), 0.0, 0.0, WHITE, params);
        gl_use_default_material();
    }
}


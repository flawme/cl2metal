//! GPU terminal renderer: instanced quads + glyph atlas, painted as an egui
//! custom callback inside the main wgpu frame.

pub mod glyphs;

use crate::config::BackgroundFit;
use crate::term::TermCore;
use crate::theme::Theme;
use alacritty_terminal::{
    index::Point,
    term::cell::{Cell, Flags},
    vte::ansi::CursorShape,
};
use glyphs::GlyphAtlas;
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// Instance data
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct BgInst {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub color: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlyphInst {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    pub color: [f32; 4],
}

/// Placement of the background image: a quad in physical pixels plus the
/// sub-rectangle of the texture to sample.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ImageQuad {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
}

#[derive(Default)]
pub struct TermFrame {
    pub bg: Vec<BgInst>,
    pub glyphs: Vec<GlyphInst>,
}

/// How to draw the cursor this frame.
#[derive(Clone, Copy)]
pub struct CursorOpts {
    /// False during the off half of the blink cycle.
    pub visible: bool,
    /// Window unfocused: outline the block instead of filling it.
    pub hollow: bool,
}

impl Default for CursorOpts {
    fn default() -> Self {
        Self {
            visible: true,
            hollow: false,
        }
    }
}

/// Search-highlight cells: `(grid line, column) -> is the current match`.
pub type SearchCells = std::collections::HashMap<(i32, usize), bool>;

fn rgba(c: [u8; 3], a: f32) -> [f32; 4] {
    [
        c[0] as f32 / 255.0,
        c[1] as f32 / 255.0,
        c[2] as f32 / 255.0,
        a,
    ]
}

fn scale_rgb(c: [u8; 3], f: f32) -> [u8; 3] {
    [
        (c[0] as f32 * f) as u8,
        (c[1] as f32 * f) as u8,
        (c[2] as f32 * f) as u8,
    ]
}

// ---------------------------------------------------------------------------
// Frame building
// ---------------------------------------------------------------------------

/// Build GPU instance data for the current terminal viewport.
///
/// `origin_px` is the physical-pixel position of the grid's top-left cell,
/// `cell` the physical-pixel cell size. When `transparent_default_bg` is set,
/// cells using the theme's ordinary terminal background are left unpainted so
/// a shared application backdrop remains visually continuous behind the grid.
#[allow(clippy::too_many_arguments)]
pub fn build_frame(
    term: &TermCore,
    atlas: &mut GlyphAtlas,
    theme: &Theme,
    origin_px: [f32; 2],
    cell: [f32; 2],
    cursor_opts: CursorOpts,
    search: Option<&SearchCells>,
    transparent_default_bg: bool,
    // Sub-line scroll shift in pixels: the whole frame moves up by this,
    // and one row past the viewport is kept so the bottom edge stays
    // filled while the scroll glides.
    scroll_px: f32,
) -> TermFrame {
    let mut frame = TermFrame::default();
    let content = term.term.renderable_content();
    let (cw, ch) = (cell[0], cell[1]);
    let display_offset = content.display_offset;
    let selection = content.selection;
    let cursor = &content.cursor;
    let cursor_visible = cursor.shape != CursorShape::Hidden && cursor_opts.visible;

    let underline_h = (ch * 0.055).round().max(1.0);
    let rows = term.rows();

    let cell_pos = |point: Point| -> Option<[f32; 2]> {
        let row = point.line.0 + display_offset as i32;
        // One row past the viewport is kept: the sub-line shift pulls it
        // into view at the bottom edge.
        if row < 0 || row > rows as i32 {
            return None;
        }
        Some([
            origin_px[0] + point.column.0 as f32 * cw,
            origin_px[1] + row as f32 * ch - scroll_px,
        ])
    };

    for indexed in content.display_iter {
        let point = indexed.point;
        let cell_ref: &Cell = indexed.cell;
        let Some([x, y]) = cell_pos(point) else {
            continue;
        };

        let flags = cell_ref.flags;
        let bold = flags.contains(Flags::BOLD) && !flags.contains(Flags::DIM);
        let italic = flags.contains(Flags::ITALIC);
        let dim = flags.contains(Flags::DIM);

        // Resolve colors.
        let mut fg = theme.resolve(cell_ref.fg);
        let mut bg = theme.resolve(cell_ref.bg);

        // Bold + named color → bright variant (classic terminal behavior).
        if flags.contains(Flags::BOLD)
            && let alacritty_terminal::vte::ansi::Color::Named(n) = cell_ref.fg
        {
            let idx = n as usize;
            if (0..8).contains(&idx) {
                fg = theme.ansi[idx + 8];
            }
        }
        if dim {
            fg = scale_rgb(fg, theme.dim_factor);
        }

        let in_selection = selection.is_some_and(|s| s.contains(point));
        if flags.contains(Flags::INVERSE) {
            std::mem::swap(&mut fg, &mut bg);
        }
        if flags.contains(Flags::HIDDEN) {
            fg = bg;
        }

        let is_cursor = cursor_visible
            && cursor.point.line == point.line
            && cursor.point.column == point.column;

        // Background.
        let mut bg_color = rgba(bg, 1.0);
        let mut has_distinct_background = bg != theme.bg;
        if in_selection && !is_cursor {
            bg_color = rgba(theme.selection, theme.selection_alpha);
            has_distinct_background = true;
        }
        // Search highlights (the current match is stronger).
        if let Some(cells) = search
            && let Some(&is_current) = cells.get(&(point.line.0, point.column.0))
        {
            bg_color = rgba(theme.ui.accent, if is_current { 0.45 } else { 0.20 });
            has_distinct_background = true;
        }
        let mut bg_w = cw;
        if flags.contains(Flags::WIDE_CHAR) {
            bg_w = cw * 2.0;
        }
        let cursor_color = rgba(theme.cursor, 1.0);
        // Hollow: the window is unfocused, or the app asked for a hollow block.
        let hollow_cursor =
            is_cursor && (cursor_opts.hollow || matches!(cursor.shape, CursorShape::HollowBlock));
        if is_cursor && matches!(cursor.shape, CursorShape::Block | CursorShape::HollowBlock) {
            if hollow_cursor {
                let t = (ch * 0.09).round().max(1.0);
                frame.bg.push(BgInst {
                    pos: [x, y],
                    size: [bg_w, t],
                    color: cursor_color,
                });
                frame.bg.push(BgInst {
                    pos: [x, y + ch - t],
                    size: [bg_w, t],
                    color: cursor_color,
                });
                frame.bg.push(BgInst {
                    pos: [x, y],
                    size: [t, ch],
                    color: cursor_color,
                });
                frame.bg.push(BgInst {
                    pos: [x + bg_w - t, y],
                    size: [t, ch],
                    color: cursor_color,
                });
            } else {
                bg_color = cursor_color;
                has_distinct_background = true;
            }
        }
        if !transparent_default_bg || has_distinct_background {
            frame.bg.push(BgInst {
                pos: [x, y],
                size: [bg_w, ch],
                color: bg_color,
            });
        }

        // Underline / strikethrough decorations.
        let decor_color = if in_selection && !is_cursor {
            rgba(theme.fg, 0.9)
        } else {
            rgba(fg, 1.0)
        };
        if flags.intersects(Flags::ALL_UNDERLINES) {
            let uy = y + ch - underline_h - 1.0;
            let mut push = |px: f32, py: f32, pw: f32, ph: f32| {
                frame.bg.push(BgInst {
                    pos: [px, py],
                    size: [pw, ph],
                    color: decor_color,
                });
            };
            if flags.contains(Flags::UNDERCURL) {
                // Zigzag: alternating half-steps along the cell.
                let step = (cw / 4.0).max(2.0);
                let mut px = x;
                let mut up = true;
                while px < x + bg_w {
                    let seg = step.min(x + bg_w - px);
                    push(px, if up { uy } else { uy + underline_h }, seg, underline_h);
                    px += seg;
                    up = !up;
                }
            } else if flags.contains(Flags::DOTTED_UNDERLINE) {
                let step = (cw / 3.0).max(3.0);
                let mut px = x;
                while px < x + bg_w {
                    let seg = (step * 0.5).min(x + bg_w - px);
                    push(px, uy, seg, underline_h);
                    px += step;
                }
            } else if flags.contains(Flags::DASHED_UNDERLINE) {
                let step = (cw / 2.0).max(4.0);
                let mut px = x;
                while px < x + bg_w {
                    let seg = (step * 0.6).min(x + bg_w - px);
                    push(px, uy, seg, underline_h);
                    px += step;
                }
            } else if flags.contains(Flags::DOUBLE_UNDERLINE) {
                push(x, uy, bg_w, underline_h);
                push(
                    x,
                    (uy + underline_h + 1.0).min(y + ch - underline_h),
                    bg_w,
                    underline_h,
                );
            } else {
                push(x, uy, bg_w, underline_h);
            }
        }
        if flags.contains(Flags::STRIKEOUT) {
            frame.bg.push(BgInst {
                pos: [x, y + (ch * 0.5 - underline_h * 0.5)],
                size: [bg_w, underline_h],
                color: decor_color,
            });
        }

        // Cursor bar / underline shapes.
        if is_cursor {
            match cursor.shape {
                CursorShape::Beam => {
                    frame.bg.push(BgInst {
                        pos: [x, y],
                        size: [underline_h.max(1.0), ch],
                        color: rgba(theme.cursor, 1.0),
                    });
                }
                CursorShape::Underline => {
                    frame.bg.push(BgInst {
                        pos: [x, y + ch - underline_h - 1.0],
                        size: [cw, underline_h],
                        color: rgba(theme.cursor, 1.0),
                    });
                }
                _ => {}
            }
        }

        // Glyph.
        let ch_char = cell_ref.c;
        if ch_char != ' '
            && !flags.contains(Flags::WIDE_CHAR_SPACER)
            && let Some(entry) = atlas.glyph(ch_char, bold, italic)
            && entry.w > 0.0
        {
            let wide = flags.contains(Flags::WIDE_CHAR);
            let area_w = if wide { cw * 2.0 } else { cw };
            let gx = x + ((area_w - entry.w) * 0.5).round();
            let gy = y + entry.top.round();

            let mut glyph_color = rgba(fg, 1.0);
            if is_cursor && matches!(cursor.shape, CursorShape::Block) && !hollow_cursor {
                // Invert text under a filled block cursor.
                glyph_color = rgba(bg, 1.0);
            }
            frame.glyphs.push(GlyphInst {
                pos: [gx, gy],
                size: [entry.w, entry.h],
                uv0: [entry.uv[0], entry.uv[1]],
                uv1: [entry.uv[2], entry.uv[3]],
                color: glyph_color,
            });
        }
    }

    frame
}

/// Fit the background image into `rect` (`[x, y, w, h]`, physical pixels).
///
/// `cover` crops the overflow, `contain` letterboxes the whole image inside
/// the area, `stretch` distorts it to fill.
pub fn image_quad(rect: [f32; 4], image: [u32; 2], fit: BackgroundFit) -> ImageQuad {
    let (rw, rh) = (rect[2].max(1.0), rect[3].max(1.0));
    let (iw, ih) = (image[0].max(1) as f32, image[1].max(1) as f32);
    let mut pos = [rect[0], rect[1]];
    let mut size = [rw, rh];
    let mut uv0 = [0.0, 0.0];
    let mut uv1 = [1.0, 1.0];
    let (rect_aspect, image_aspect) = (rw / rh, iw / ih);
    match fit {
        BackgroundFit::Stretch => {}
        BackgroundFit::Cover => {
            // Crop the axis that overflows.
            if image_aspect > rect_aspect {
                let keep = rect_aspect / image_aspect;
                uv0[0] = (1.0 - keep) * 0.5;
                uv1[0] = uv0[0] + keep;
            } else {
                let keep = image_aspect / rect_aspect;
                uv0[1] = (1.0 - keep) * 0.5;
                uv1[1] = uv0[1] + keep;
            }
        }
        BackgroundFit::Contain => {
            // Shrink the quad so the whole image fits, centred.
            if image_aspect > rect_aspect {
                size[1] = rw / image_aspect;
            } else {
                size[0] = rh * image_aspect;
            }
            pos = [
                rect[0] + (rw - size[0]) * 0.5,
                rect[1] + (rh - size[1]) * 0.5,
            ];
        }
    }
    ImageQuad {
        pos,
        size,
        uv0,
        uv1,
    }
}

// ---------------------------------------------------------------------------
// wgpu resources
// ---------------------------------------------------------------------------

const SHADER_V2: &str = r#"
struct Uni {
    // xy = screen size in pixels, zw = the callback's viewport origin.
    screen: vec4f,
    // x = background alpha (a background image shows through the cells),
    // yz = the callback's viewport size.
    params: vec4f,
};

@group(0) @binding(0) var<uniform> u: Uni;
@group(0) @binding(1) var t_atlas: texture_2d<f32>;
@group(0) @binding(2) var s_atlas: sampler;

const CORNERS = array<vec2f, 6>(
    vec2f(0.0, 0.0), vec2f(1.0, 0.0), vec2f(0.0, 1.0),
    vec2f(0.0, 1.0), vec2f(1.0, 0.0), vec2f(1.0, 1.0),
);

struct VsOut {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec4f,
};

fn to_ndc(p: vec2f) -> vec4f {
    // egui sets the render pass viewport to the callback's rect, so positions
    // are mapped relative to that viewport, not the whole screen.
    let rel = (p - u.screen.zw) / u.params.yz;
    return vec4f(
        rel.x * 2.0 - 1.0,
        1.0 - rel.y * 2.0,
        0.0,
        1.0,
    );
}

@vertex
fn vs_bg(
    @location(0) i_pos: vec2f,
    @location(1) i_size: vec2f,
    @location(2) i_color: vec4f,
    @builtin(vertex_index) vi: u32,
) -> VsOut {
    let c = CORNERS[vi];
    var out: VsOut;
    out.pos = to_ndc(i_pos + c * i_size);
    out.uv = vec2f(0.0, 0.0);
    out.color = i_color;
    return out;
}

@fragment
fn fs_bg(in: VsOut) -> @location(0) vec4f {
    return vec4f(in.color.rgb, in.color.a * u.params.x);
}

@vertex
fn vs_glyph(
    @location(0) i_pos: vec2f,
    @location(1) i_size: vec2f,
    @location(2) i_uv0: vec2f,
    @location(3) i_uv1: vec2f,
    @location(4) i_color: vec4f,
    @builtin(vertex_index) vi: u32,
) -> VsOut {
    let c = CORNERS[vi];
    var out: VsOut;
    out.pos = to_ndc(i_pos + c * i_size);
    out.uv = mix(i_uv0, i_uv1, c);
    out.color = i_color;
    return out;
}

@fragment
fn fs_glyph(in: VsOut) -> @location(0) vec4f {
    let cov = textureSample(t_atlas, s_atlas, in.uv).r;
    return vec4f(in.color.rgb, in.color.a * cov);
}
"#;

pub struct RendererCore {
    bg_pipeline: wgpu::RenderPipeline,
    glyph_pipeline: wgpu::RenderPipeline,
    sampler: wgpu::Sampler,
    bind_group_layout: wgpu::BindGroupLayout,
}

impl RendererCore {
    pub fn new(state: &egui_wgpu::RenderState) -> Arc<Self> {
        let device = &state.device;
        let format = state.target_format;

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cl2metal terminal"),
            source: wgpu::ShaderSource::Wgsl(SHADER_V2.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("terminal bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("terminal layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        fn vertex_layout(
            stride: u64,
            attrs: &[wgpu::VertexAttribute],
        ) -> wgpu::VertexBufferLayout<'_> {
            wgpu::VertexBufferLayout {
                array_stride: stride,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: attrs,
            }
        }

        let bg_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bg pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_bg"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout(
                    std::mem::size_of::<BgInst>() as u64,
                    &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Float32x4],
                ))],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_bg"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let glyph_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("glyph pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_glyph"),
                compilation_options: Default::default(),
                buffers: &[Some(vertex_layout(
                    std::mem::size_of::<GlyphInst>() as u64,
                    &wgpu::vertex_attr_array![
                        0 => Float32x2,
                        1 => Float32x2,
                        2 => Float32x2,
                        3 => Float32x2,
                        4 => Float32x4
                    ],
                ))],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_glyph"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atlas sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });

        Arc::new(Self {
            bg_pipeline,
            glyph_pipeline,
            sampler,
            bind_group_layout,
        })
    }
}

// ---------------------------------------------------------------------------
// GPU atlas (texture + view shared between the app and the paint callback)
// ---------------------------------------------------------------------------

pub struct AtlasTex {
    pub texture: wgpu::Texture,
    pub view: Arc<wgpu::TextureView>,
    pub width: u32,
    pub height: u32,
    pub version: u64,
}

pub struct AtlasGpu {
    pub inner: Mutex<AtlasTex>,
}

impl AtlasGpu {
    pub fn new(device: &wgpu::Device, width: u32, height: u32, version: u64) -> Self {
        let (texture, view) = create_texture(device, width, height);
        Self {
            inner: Mutex::new(AtlasTex {
                texture,
                view,
                width,
                height,
                version,
            }),
        }
    }

    /// Sync the CPU atlas state (size changes + dirty rects) to the GPU.
    #[allow(clippy::too_many_arguments)]
    pub fn sync(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pixels: &[u8],
        width: u32,
        height: u32,
        version: u64,
        dirty: Vec<[u16; 4]>,
    ) -> Arc<wgpu::TextureView> {
        let mut inner = self.inner.lock().unwrap();
        if inner.version != version || inner.width != width || inner.height != height {
            let (texture, view) = create_texture(device, width, height);
            inner.texture = texture;
            inner.view = view;
            inner.width = width;
            inner.height = height;
            inner.version = version;
            // Full upload.
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &inner.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                pixels,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width),
                    rows_per_image: None,
                },
                wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
            );
        } else {
            for [x, y, w, h] in dirty {
                if w == 0 || h == 0 {
                    continue;
                }
                let (x, y, w, h) = (x as u32, y as u32, w as u32, h as u32);
                let mut rect = Vec::with_capacity((w * h) as usize);
                for row in y..(y + h) {
                    let start = (row * width + x) as usize;
                    rect.extend_from_slice(&pixels[start..start + w as usize]);
                }
                queue.write_texture(
                    wgpu::TexelCopyTextureInfo {
                        texture: &inner.texture,
                        mip_level: 0,
                        origin: wgpu::Origin3d { x, y, z: 0 },
                        aspect: wgpu::TextureAspect::All,
                    },
                    &rect,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(w),
                        rows_per_image: None,
                    },
                    wgpu::Extent3d {
                        width: w,
                        height: h,
                        depth_or_array_layers: 1,
                    },
                );
            }
        }
        inner.view.clone()
    }
}

fn create_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
) -> (wgpu::Texture, Arc<wgpu::TextureView>) {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glyph atlas"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let view = Arc::new(texture.create_view(&wgpu::TextureViewDescriptor::default()));
    (texture, view)
}

// ---------------------------------------------------------------------------
// Paint callback
// ---------------------------------------------------------------------------

/// Persistent GPU state for the terminal callbacks.
///
/// Lives in `egui_wgpu::CallbackResources` so buffers and bind groups survive
/// between frames instead of being reallocated every paint. There is one entry
/// per pane: egui runs a separate callback for each, and they must not share
/// instance buffers or the last one prepared would be drawn everywhere.
struct CallbackRes {
    /// Atlas view the per-pane bind groups were built for.
    atlas_view: Arc<wgpu::TextureView>,
    /// Per-callback state, keyed by the callback's id.
    panes: std::collections::HashMap<u64, PaneRes>,
}

/// One pane's GPU state.
struct PaneRes {
    bg_buffer: wgpu::Buffer,
    /// Capacity of `bg_buffer`, in bytes.
    bg_bytes: usize,
    glyph_buffer: wgpu::Buffer,
    /// Capacity of `glyph_buffer`, in bytes.
    glyph_bytes: usize,
    uniform: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    bg_count: u32,
    glyph_count: u32,
}

impl PaneRes {
    fn new(
        device: &wgpu::Device,
        core: &RendererCore,
        atlas_view: &Arc<wgpu::TextureView>,
        bg_bytes: usize,
        glyph_bytes: usize,
    ) -> Self {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("terminal uniform"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg_buffer = instance_buffer(device, "bg instances", bg_bytes);
        let glyph_buffer = instance_buffer(device, "glyph instances", glyph_bytes);
        let bind_group = bind_group(device, core, &uniform, atlas_view);
        Self {
            bg_buffer,
            bg_bytes: instance_bytes(bg_bytes),
            glyph_buffer,
            glyph_bytes: instance_bytes(glyph_bytes),
            uniform,
            bind_group,
            bg_count: 0,
            glyph_count: 0,
        }
    }
}

/// Physical-pixel viewport of a callback rect, rounded the way egui does when
/// it sets the render pass viewport.
pub fn viewport_px(rect: egui::Rect, ppp: f32) -> [f32; 4] {
    let left = (rect.min.x * ppp).round();
    let top = (rect.min.y * ppp).round();
    let right = (rect.max.x * ppp).round();
    let bottom = (rect.max.y * ppp).round();
    [left, top, (right - left).max(0.0), (bottom - top).max(0.0)]
}

/// Vertex buffer sized for `bytes` of instance data (at least one instance).
fn instance_buffer(device: &wgpu::Device, label: &str, bytes: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: instance_bytes(bytes) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Rounded-up buffer size for `bytes` of instance data.
///
/// Sizing is done in bytes, never in "instances of some type": the bg and glyph
/// instances have different sizes, and mixing them up overruns the buffer.
fn instance_bytes(bytes: usize) -> usize {
    bytes.max(std::mem::size_of::<BgInst>()).next_power_of_two()
}

fn bind_group(
    device: &wgpu::Device,
    core: &RendererCore,
    uniform: &wgpu::Buffer,
    texture_view: &wgpu::TextureView,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("terminal bind group"),
        layout: &core.bind_group_layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(texture_view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&core.sampler),
            },
        ],
    })
}

pub struct TerminalCallback {
    core: Arc<RendererCore>,
    atlas_view: Arc<wgpu::TextureView>,
    /// Which pane this callback draws: its own buffers and uniform.
    id: u64,
    /// The callback's rect in physical pixels: egui sets the render pass
    /// viewport to it, so the shader has to work relative to it.
    viewport: [f32; 4],
    /// Opacity of the cell backgrounds: a background image shows through.
    bg_alpha: f32,
    bg: Vec<BgInst>,
    glyphs: Vec<GlyphInst>,
}

impl TerminalCallback {
    pub fn new(
        core: Arc<RendererCore>,
        atlas_view: Arc<wgpu::TextureView>,
        id: u64,
        viewport: [f32; 4],
        frame: TermFrame,
        bg_alpha: f32,
    ) -> Self {
        Self {
            core,
            atlas_view,
            id,
            viewport,
            bg_alpha,
            bg: frame.bg,
            glyphs: frame.glyphs,
        }
    }
}

impl egui_wgpu::CallbackTrait for TerminalCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        screen: &egui_wgpu::ScreenDescriptor,
        _encoder: &mut wgpu::CommandEncoder,
        resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let size = screen.size_in_pixels;
        let viewport = self.viewport;
        let screen_px = [
            size[0] as f32,
            size[1] as f32,
            viewport[0],
            viewport[1],
            self.bg_alpha.clamp(0.0, 1.0),
            viewport[2],
            viewport[3],
            0.0,
        ];

        if resources.get::<CallbackRes>().is_none() {
            resources.insert(CallbackRes {
                atlas_view: self.atlas_view.clone(),
                panes: std::collections::HashMap::new(),
            });
        }
        let res = resources
            .get_mut::<CallbackRes>()
            .expect("callback resources just inserted");

        // The atlas texture view changes when the atlas is re-uploaded: every
        // pane's bind group has to be rebuilt against the new view.
        if !Arc::ptr_eq(&res.atlas_view, &self.atlas_view) {
            res.atlas_view = self.atlas_view.clone();
            res.panes.clear();
        }

        let bg_bytes = std::mem::size_of_val(self.bg.as_slice());
        let glyph_bytes = std::mem::size_of_val(self.glyphs.as_slice());
        let pane = res.panes.entry(self.id).or_insert_with(|| {
            PaneRes::new(device, &self.core, &self.atlas_view, bg_bytes, glyph_bytes)
        });

        queue.write_buffer(&pane.uniform, 0, bytemuck::cast_slice(&screen_px));

        // Instance buffers only grow; steady-state frames just rewrite them.
        // Sizes are tracked in bytes so the two instance types cannot be mixed up.
        if pane.bg_bytes < bg_bytes {
            pane.bg_bytes = instance_bytes(bg_bytes);
            pane.bg_buffer = instance_buffer(device, "bg instances", bg_bytes);
        }
        if pane.glyph_bytes < glyph_bytes {
            pane.glyph_bytes = instance_bytes(glyph_bytes);
            pane.glyph_buffer = instance_buffer(device, "glyph instances", glyph_bytes);
        }
        if !self.bg.is_empty() {
            queue.write_buffer(&pane.bg_buffer, 0, bytemuck::cast_slice(&self.bg));
        }
        if !self.glyphs.is_empty() {
            queue.write_buffer(&pane.glyph_buffer, 0, bytemuck::cast_slice(&self.glyphs));
        }

        pane.bg_count = self.bg.len() as u32;
        pane.glyph_count = self.glyphs.len() as u32;
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        pass: &mut wgpu::RenderPass<'static>,
        resources: &egui_wgpu::CallbackResources,
    ) {
        let Some(res) = resources.get::<CallbackRes>() else {
            return;
        };
        let Some(pane) = res.panes.get(&self.id) else {
            return;
        };
        if pane.bg_count > 0 {
            pass.set_pipeline(&self.core.bg_pipeline);
            pass.set_bind_group(0, &pane.bind_group, &[]);
            pass.set_vertex_buffer(0, pane.bg_buffer.slice(..));
            pass.draw(0..6, 0..pane.bg_count);
        }
        if pane.glyph_count > 0 {
            pass.set_pipeline(&self.core.glyph_pipeline);
            pass.set_bind_group(0, &pane.bind_group, &[]);
            pass.set_vertex_buffer(0, pane.glyph_buffer.slice(..));
            pass.draw(0..6, 0..pane.glyph_count);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: the glyph buffer used to be sized with the *bg* instance
    /// size, so a frame with enough glyphs overran it and wgpu aborted the
    /// process with a `Queue::write_buffer` validation error.
    #[test]
    fn instance_buffers_always_fit_their_data() {
        for count in 1..600 {
            let bg = count * std::mem::size_of::<BgInst>();
            assert!(instance_bytes(bg) >= bg, "bg buffer too small for {count}");
            let glyphs = count * std::mem::size_of::<GlyphInst>();
            assert!(
                instance_bytes(glyphs) >= glyphs,
                "glyph buffer too small for {count}"
            );
        }
    }

    #[test]
    fn instance_sizes_differ_so_they_must_not_be_shared() {
        assert!(std::mem::size_of::<GlyphInst>() > std::mem::size_of::<BgInst>());
    }

    #[test]
    fn empty_frames_still_get_a_usable_buffer() {
        assert!(instance_bytes(0) >= std::mem::size_of::<BgInst>());
    }

    #[test]
    fn image_quad_covers_crops_and_contain_letterboxes() {
        // A 200x100 image in a 100x100 area.
        let rect = [0.0, 0.0, 100.0, 100.0];
        let image = [200, 100];

        // Cover: fills the area, cropping the sides (half the width shows).
        let q = image_quad(rect, image, BackgroundFit::Cover);
        assert_eq!(q.pos, [0.0, 0.0]);
        assert_eq!(q.size, [100.0, 100.0]);
        assert_eq!(q.uv0, [0.25, 0.0]);
        assert_eq!(q.uv1, [0.75, 1.0]);

        // Contain: the whole image fits, letterboxed vertically.
        let q = image_quad(rect, image, BackgroundFit::Contain);
        assert_eq!(q.pos, [0.0, 25.0]);
        assert_eq!(q.size, [100.0, 50.0]);
        assert_eq!(q.uv0, [0.0, 0.0]);
        assert_eq!(q.uv1, [1.0, 1.0]);

        // Stretch: fills the area with the whole image, distorted.
        let q = image_quad(rect, image, BackgroundFit::Stretch);
        assert_eq!(q.pos, [0.0, 0.0]);
        assert_eq!(q.size, [100.0, 100.0]);
        assert_eq!(q.uv0, [0.0, 0.0]);
        assert_eq!(q.uv1, [1.0, 1.0]);
    }

    #[test]
    fn image_quad_covers_tall_images_by_cropping_the_height() {
        let q = image_quad([0.0, 0.0, 100.0, 100.0], [100, 200], BackgroundFit::Cover);
        assert_eq!(q.size, [100.0, 100.0]);
        assert_eq!(q.uv0, [0.0, 0.25]);
        assert_eq!(q.uv1, [1.0, 0.75]);
    }
}

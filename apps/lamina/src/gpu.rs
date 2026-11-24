//! The 3D view drawn on the graphics card, through the wgpu device eframe draws the window with. The scene's triangles go up once for
//! each scene (each object's world mesh, three vertices a triangle); each picture is then two passes of `gpu.wgsl` into a texture egui
//! shows as it shows the processor's. Between pictures little changes: the camera is a uniform, and the selection and a drag's preview
//! are a colour and a transform for each object. The ids and the depth are read back a pixel at a time, only when a click asks. The
//! processor's picture (`lamina_core::render::draw`) stays the fallback: a window with no wgpu device (every test), the user's choice
//! (View, kept in the settings file), or a card that fails, whose error is caught here, not left to the device's handler, and said on
//! the status bar.
//!
//! Ported from BIMCraft's `crates/ui-egui/src/gpu.rs` and `gpu.wgsl` (MIT OR Apache-2.0, the same author): the picture rendered into a
//! texture registered with egui, the targets a full-screen pass reads to draw edges where what a pixel shows changes from its
//! neighbour's, the depth compared as "greater is nearer", and errors caught in error scopes so the window carries on without the
//! card. Its BIM parts (sections, see-through, textures, levels, instancing, memory budgets) are left out.

use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Weak};

use eframe::egui_wgpu::{RenderState, wgpu};
use egui::TextureId;
use lamina_core::render::{self, Camera, Rgba, cross, dot, scale, sub};
use lamina_core::{Scene, Transform};
use wgpu::util::DeviceExt;

const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The object and its triangle at a pixel.
const IDS: wgpu::TextureFormat = wgpu::TextureFormat::Rg32Uint;
/// The plane at a pixel: its unit normal turned to the eye, and its distance from the origin.
const PLANES: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// The depth again, as the second pass copies it: a depth texture is copied whole or not at all, and a click wants one pixel.
const DEPTHS: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;

/// A vertex as the card takes it: where it is (three floats), its triangle's unit normal (three), and the triangle's place in its
/// object's mesh.
pub const STRIDE: usize = 28;
/// An object as the card takes it: its id, its colour, and the transform a drag shows it moved by (three rows of four floats).
pub const INSTANCE: usize = 56;
/// The instances before the objects': the plate's, then the grid's.
const GROUND: u32 = 2;
/// The most bytes of vertices sent, whatever the card allows: about 12 million triangles.
const MAX_BYTES: u64 = 1 << 30;
/// Where the depth lands in the buffer a pixel is read back into, after the ids (a copy starts on 256 bytes).
const DEPTH_AT: u64 = 256;

/// The scene's triangles as the card takes them: each object's world mesh, three vertices a triangle (none for a triangle with no
/// area, which the processor leaves out too), and each object's run of vertices, in the scene's order.
pub fn pack(scene: &Scene) -> (Vec<u8>, Vec<Range<u32>>) {
    let mut out = Vec::new();
    let mut runs = Vec::with_capacity(scene.objects.len());
    for o in &scene.objects {
        let start = (out.len() / STRIDE) as u32;
        let world = o.world();
        out.reserve(world.triangles.len() * 3 * STRIDE);
        for i in 0..world.triangles.len() {
            let Some(t) = world.tri(i) else { continue };
            let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
            let len = dot(n, n).sqrt();
            if len < 1e-12 {
                continue;
            }
            let n = scale(n, 1.0 / len);
            for p in t {
                for c in p.into_iter().chain(n) {
                    out.extend_from_slice(&(c as f32).to_le_bytes());
                }
                out.extend_from_slice(&(i as u32).to_le_bytes());
            }
        }
        runs.push(start..(out.len() / STRIDE) as u32);
    }
    (out, runs)
}

/// The camera, the light and the edge pass's settings as `Globals` in `gpu.wgsl` holds them, for a picture `size` pixels: the eye;
/// the picture's right and up, scaled so `dot(d, right) / ahead` is where a point `d` from the eye lands across it, -1 to 1, as
/// [`Camera::project`] puts it; the forward axis; the light; and the crease's cosine, the step, mm, and how much an edge darkens.
pub fn globals(camera: &Camera, size: [usize; 2]) -> [[f32; 4]; 6] {
    let (eye, right, up, forward) = camera.frame();
    let (w, h) = (size[0].max(1) as f64, size[1].max(1) as f64);
    let f = (h / 2.0) / (render::FOV.to_radians() / 2.0).tan();
    let v = |p: [f64; 3], k: f64| [(p[0] * k) as f32, (p[1] * k) as f32, (p[2] * k) as f32, 0.0];
    let edges = [render::CREASE.to_radians().cos() as f32, render::STEP, render::EDGE_SHADE as f32, 0.0];
    [v(eye, 1.0), v(right, f / (w / 2.0)), v(up, f / (h / 2.0)), v(forward, 1.0), v(camera.light(), 1.0), edges]
}

/// The plate's and the grid's instances, then each object's: its id, its colour (the highlight's when it is in `selected`), and the
/// transform a drag shows it moved by (from `moved`; none for the rest).
pub fn instances(scene: &Scene, selected: &[u32], moved: &[(u32, Transform)]) -> Vec<u8> {
    let one = |out: &mut Vec<u8>, id: u32, color: Rgba, t: &Transform| {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&color); // red in the low byte, as `unpack4x8unorm` reads it
        for r in 0..3 {
            for v in [t.m[r][0], t.m[r][1], t.m[r][2], t.t[r]] {
                out.extend_from_slice(&(v as f32).to_le_bytes());
            }
        }
    };
    let mut out = Vec::with_capacity((scene.objects.len() + 2) * INSTANCE);
    one(&mut out, 0, render::PLATE, &Transform::IDENTITY);
    one(&mut out, 0, render::GRID, &Transform::IDENTITY);
    for o in &scene.objects {
        let color = if selected.contains(&o.id) { render::SELECTED } else { render::MODEL };
        let t = moved.iter().find(|(id, _)| *id == o.id).map_or(&Transform::IDENTITY, |(_, t)| t);
        one(&mut out, o.id, color, t);
    }
    out
}

/// The plate, `bed` mm and centred on the origin, as two triangles, then a line every 10 mm a hair above it, as `Raster::plate`
/// draws them. Neither has a normal: they are not lit.
pub fn ground(bed: [f64; 2]) -> Vec<u8> {
    let (hx, hy) = (bed[0] / 2.0, bed[1] / 2.0);
    let mut points = vec![[-hx, -hy, 0.0], [hx, -hy, 0.0], [hx, hy, 0.0], [-hx, -hy, 0.0], [hx, hy, 0.0], [-hx, hy, 0.0]];
    let mut x = -hx;
    while x <= hx + 1e-9 {
        points.extend([[x, -hy, 0.01], [x, hy, 0.01]]);
        x += 10.0;
    }
    let mut y = -hy;
    while y <= hy + 1e-9 {
        points.extend([[-hx, y, 0.01], [hx, y, 0.01]]);
        y += 10.0;
    }
    let mut out = Vec::with_capacity(points.len() * STRIDE);
    for p in points {
        for c in p.into_iter().chain([0.0; 3]) {
            out.extend_from_slice(&(c as f32).to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes());
    }
    out
}

/// What a pixel of the card's last picture holds: the object (0 for none), its triangle, one over its depth (0 where nothing is),
/// and the picture's size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub id: u32,
    pub tri: u32,
    pub inv: f32,
    pub size: [usize; 2],
}

/// The window's use of the card: the wgpu state eframe made (none in a test, or where the window has no wgpu), whether the user
/// asked for the processor, the card once it is made, and why it stopped drawing, for the status bar.
#[derive(Default)]
pub struct Card {
    pub state: Option<RenderState>,
    pub cpu: bool,
    gpu: Option<Gpu>,
    failed: bool,
    pub note: Option<String>,
}

impl Card {
    /// Draws the picture on the card, if it may: made the first time. One that fails is let go and says why in `note`, and is not
    /// tried again until the user asks for it ([`retry`](Self::retry)); none then, and the processor draws.
    pub fn draw(
        &mut self,
        scene: &Arc<Scene>,
        selected: &[u32],
        moved: &[(u32, Transform)],
        camera: &Camera,
        bed: [f64; 2],
        size: [usize; 2],
    ) -> Option<TextureId> {
        if self.cpu || self.failed {
            return None;
        }
        let Some(state) = &self.state else { return None };
        if self.gpu.is_none() {
            match Gpu::new(state) {
                Ok(gpu) => self.gpu = Some(gpu),
                Err(e) => return self.fail(e),
            }
        }
        let gpu = self.gpu.as_mut()?;
        let drawn = catch_unwind(AssertUnwindSafe(|| gpu.draw(scene, selected, moved, camera, bed, size)))
            .unwrap_or_else(|_| Err("it stopped while drawing".into()))
            .and_then(|()| gpu.texture.ok_or_else(|| "egui has no texture of it".to_string()));
        drawn.map_err(|e| self.fail(e)).ok()
    }

    /// What the card's last picture shows at pixel (`x`, `y`), when the card drew it.
    pub fn at(&self, x: usize, y: usize) -> Option<Hit> {
        let gpu = self.gpu.as_ref().filter(|_| !self.cpu)?;
        catch_unwind(AssertUnwindSafe(|| gpu.at(x, y))).ok().flatten()
    }

    /// Tries the card again after it failed: the user asked for it.
    pub fn retry(&mut self) {
        self.failed = false;
    }

    fn fail(&mut self, e: String) -> Option<TextureId> {
        (self.gpu, self.failed) = (None, true);
        self.note = Some(crate::lang::tf("The 3D view is drawn on the processor: the graphics card failed ({}).", &[&e]));
        None
    }
}

/// The scene on the card, and which scene it is: held weakly, so its address is not reused by another while it is compared.
struct Uploaded {
    scene: Weak<Scene>,
    vertices: Option<wgpu::Buffer>,
    runs: Vec<Range<u32>>,
}

/// The first pass's targets, the picture the second draws from them and its copy of the depth, and the second's bind group.
struct Targets {
    size: [u32; 2],
    color: wgpu::TextureView,
    ids: wgpu::Texture,
    ids_view: wgpu::TextureView,
    planes: wgpu::TextureView,
    depth_view: wgpu::TextureView,
    depths: wgpu::Texture,
    depths_view: wgpu::TextureView,
    // Kept for a test to read back.
    #[cfg_attr(not(test), allow(dead_code))]
    picture: wgpu::Texture,
    picture_view: wgpu::TextureView,
    bind: wgpu::BindGroup,
}

/// The card, and what it keeps between pictures.
pub struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    state: Option<RenderState>,
    scene: wgpu::RenderPipeline,
    grid: wgpu::RenderPipeline,
    edges: wgpu::RenderPipeline,
    edges_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    globals_bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    readback: wgpu::Buffer,
    mesh: Option<Uploaded>,
    ground: Option<([f64; 2], wgpu::Buffer, u32)>,
    targets: Option<Targets>,
    texture: Option<TextureId>,
}

/// Validation and out-of-memory errors from here on come back to [`caught`] rather than going to the device's handler, which would
/// end the program.
fn scopes(device: &wgpu::Device) -> [wgpu::ErrorScopeGuard; 2] {
    [device.push_error_scope(wgpu::ErrorFilter::Validation), device.push_error_scope(wgpu::ErrorFilter::OutOfMemory)]
}

fn caught(scopes: [wgpu::ErrorScopeGuard; 2]) -> Result<(), String> {
    let [first, second] = scopes;
    // Popped in the reverse of the order they were pushed in.
    let (inner, outer) = (pollster::block_on(second.pop()), pollster::block_on(first.pop()));
    // On one line, for the status bar.
    inner.or(outer).map_or(Ok(()), |e| Err(e.to_string().split_whitespace().collect::<Vec<_>>().join(" ")))
}

impl Gpu {
    /// The card egui draws on, if it can draw this view.
    pub fn new(state: &RenderState) -> Result<Gpu, String> {
        // OpenGL lacks what the shaders read on some drivers, and where it does wgpu may panic in the shader compiler rather than
        // say so: the card is for the backends that can.
        if state.adapter.get_info().backend == wgpu::Backend::Gl {
            return Err("it needs Vulkan, Metal or DirectX 12, and the window is on OpenGL".into());
        }
        let made = catch_unwind(AssertUnwindSafe(|| Gpu::from_device(state.device.clone(), state.queue.clone())));
        let mut gpu = made.unwrap_or_else(|_| Err("its shader compiler stopped".into()))?;
        gpu.state = Some(state.clone());
        Ok(gpu)
    }

    /// A card with no window to show on: a picture is read back, not shown.
    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue) -> Result<Gpu, String> {
        let guard = scopes(&device);
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lamina-3d"),
            source: wgpu::ShaderSource::Wgsl(include_str!("gpu.wgsl").into()),
        });
        let (vertex, fragment) = (wgpu::ShaderStages::VERTEX, wgpu::ShaderStages::FRAGMENT);
        let uniform = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: vertex | fragment,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let texture = |binding: u32, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: fragment,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let float = wgpu::TextureSampleType::Float { filterable: false };
        let globals_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("lamina-3d-globals"), entries: &[uniform] });
        let edges_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lamina-3d-targets"),
            entries: &[
                uniform,
                texture(1, float),
                texture(2, wgpu::TextureSampleType::Uint),
                texture(3, float),
                texture(4, wgpu::TextureSampleType::Depth),
            ],
        });
        let layout = |label, group: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(group)],
                immediate_size: 0,
            })
        };
        let (scene_layout, edges_pipeline_layout) = (layout("lamina-3d-scene", &globals_layout), layout("lamina-3d-edges", &edges_layout));
        let vertex_attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Uint32];
        let instance_attributes = wgpu::vertex_attr_array![3 => Uint32x2, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
        let buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: STRIDE as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &vertex_attributes,
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: INSTANCE as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &instance_attributes,
            }),
        ];
        let target = |format, write_mask| Some(wgpu::ColorTargetState { format, blend: None, write_mask });
        let all = wgpu::ColorWrites::ALL;
        // The surfaces: larger depth is nearer, and of two as near the first drawn stays, as in `Raster::fill_seen`. The grid's
        // lines draw over what is as near, as `Raster::line` does, and leave the ids and the planes alone.
        let surfaces = |label, topology, compare, writes: wgpu::ColorWrites| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&scene_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_scene"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState { topology, cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(true),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_scene"),
                    compilation_options: Default::default(),
                    targets: &[target(COLOR, all), target(IDS, writes), target(PLANES, writes)],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let scene = surfaces("lamina-3d-scene", wgpu::PrimitiveTopology::TriangleList, wgpu::CompareFunction::Greater, all);
        let grid =
            surfaces("lamina-3d-grid", wgpu::PrimitiveTopology::LineList, wgpu::CompareFunction::GreaterEqual, wgpu::ColorWrites::empty());
        let edges = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("lamina-3d-edges"),
            layout: Some(&edges_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_full"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_edges"),
                compilation_options: Default::default(),
                targets: &[target(COLOR, all), target(DEPTHS, all)],
            }),
            multiview_mask: None,
            cache: None,
        });
        let buffer = |label, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size, usage, mapped_at_creation: false })
        };
        let globals = buffer("lamina-3d-globals", 6 * 16, wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST);
        let instances = buffer(
            "lamina-3d-objects",
            ((lamina_core::scene::MAX_OBJECTS + 2) * INSTANCE) as u64,
            wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        );
        let readback = buffer("lamina-3d-readback", DEPTH_AT + 4, wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST);
        let globals_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lamina-3d-globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() }],
        });
        caught(guard)?;
        Ok(Gpu {
            device,
            queue,
            state: None,
            scene,
            grid,
            edges,
            edges_layout,
            globals,
            globals_bind,
            instances,
            readback,
            mesh: None,
            ground: None,
            targets: None,
            texture: None,
        })
    }

    /// Draws `scene` as `camera` sees it in a picture `size` pixels, the objects in `selected` in the highlight colour and those in
    /// `moved` where a drag shows them: the same picture as `render::draw`. The scene goes up only when it is another `Arc` than the
    /// last; the plate only when `bed` changes. An error leaves the card to be let go: the caller draws on the processor.
    pub fn draw(
        &mut self,
        scene: &Arc<Scene>,
        selected: &[u32],
        moved: &[(u32, Transform)],
        camera: &Camera,
        bed: [f64; 2],
        size: [usize; 2],
    ) -> Result<(), String> {
        let guard = scopes(&self.device);
        if self.mesh.as_ref().is_none_or(|m| m.scene.as_ptr() != Arc::as_ptr(scene)) {
            self.upload(scene)?;
        }
        if self.ground.as_ref().is_none_or(|g| g.0 != bed) {
            let bytes = ground(bed);
            let buffer = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("lamina-3d-plate"),
                contents: &bytes,
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.ground = Some((bed, buffer, (bytes.len() / STRIDE) as u32));
        }
        // As the processor's picture, no wider or higher than 8192 pixels.
        let most = self.device.limits().max_texture_dimension_2d.min(8192);
        let [w, h] = size.map(|v| (v.min(8192) as u32).clamp(1, most));
        self.resize(w, h);
        let words: Vec<u8> = globals(camera, [w as usize, h as usize]).iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
        self.queue.write_buffer(&self.globals, 0, &words);
        let objects = instances(scene, selected, moved);
        if self.instances.size() < objects.len() as u64 {
            return Err("more objects than the card was made for".into());
        }
        self.queue.write_buffer(&self.instances, 0, &objects);
        let t = self.targets.as_ref().ok_or("it has no picture to draw in")?;
        let [r, g, b, _] = render::BACKGROUND.map(|c| f64::from(c) / 255.0);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("lamina-3d") });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lamina-3d-scene"),
                color_attachments: &[
                    attach(&t.color, wgpu::Color { r, g, b, a: 1.0 }),
                    attach(&t.ids_view, wgpu::Color::TRANSPARENT),
                    attach(&t.planes, wgpu::Color::TRANSPARENT),
                ],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &t.depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &self.globals_bind, &[]);
            pass.set_vertex_buffer(1, self.instances.slice(..));
            // The plate and its grid first, then the objects in the scene's order, as `render::draw` draws them.
            if let Some((_, plate, n)) = &self.ground {
                pass.set_pipeline(&self.scene);
                pass.set_vertex_buffer(0, plate.slice(..));
                pass.draw(0..6, 0..1);
                pass.set_pipeline(&self.grid);
                pass.draw(6..*n, 1..2);
            }
            if let Some(m) = &self.mesh
                && let Some(vertices) = &m.vertices
            {
                pass.set_pipeline(&self.scene);
                pass.set_vertex_buffer(0, vertices.slice(..));
                for (k, run) in m.runs.iter().enumerate().filter(|(_, r)| !r.is_empty()) {
                    let k = GROUND + k as u32;
                    pass.draw(run.clone(), k..k + 1);
                }
            }
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("lamina-3d-edges"),
                color_attachments: &[attach(&t.picture_view, wgpu::Color::BLACK), attach(&t.depths_view, wgpu::Color::TRANSPARENT)],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.edges);
            pass.set_bind_group(0, &t.bind, &[]);
            pass.draw(0..3, 0..1);
        }
        self.queue.submit([encoder.finish()]);
        caught(guard)
    }

    /// Sends `scene`'s triangles up, in place of the last scene's.
    fn upload(&mut self, scene: &Arc<Scene>) -> Result<(), String> {
        self.mesh = None;
        let triangles: usize = scene.objects.iter().map(|o| o.mesh.triangles.len()).sum();
        let most = self.device.limits().max_buffer_size.min(MAX_BYTES);
        if triangles as u64 * 3 * STRIDE as u64 > most {
            return Err(format!("{triangles} triangles are more than it takes in one buffer ({} MB)", most >> 20));
        }
        let (bytes, runs) = pack(scene);
        let vertices = (!bytes.is_empty()).then(|| {
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("lamina-3d-vertices"),
                contents: &bytes,
                usage: wgpu::BufferUsages::VERTEX,
            })
        });
        self.mesh = Some(Uploaded { scene: Arc::downgrade(scene), vertices, runs });
        Ok(())
    }

    /// Makes the targets `w` by `h` pixels, when they are not, and shows the new picture where egui showed the old one.
    fn resize(&mut self, w: u32, h: u32) {
        if self.targets.as_ref().is_some_and(|t| t.size == [w, h]) {
            return;
        }
        let texture = |label: &str, format, usage| {
            self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let (render, sampled, copy) =
            (wgpu::TextureUsages::RENDER_ATTACHMENT, wgpu::TextureUsages::TEXTURE_BINDING, wgpu::TextureUsages::COPY_SRC);
        let view = |t: &wgpu::Texture| t.create_view(&wgpu::TextureViewDescriptor::default());
        let color = view(&texture("lamina-3d-color", COLOR, render | sampled));
        let ids = texture("lamina-3d-ids", IDS, render | sampled | copy);
        let planes = view(&texture("lamina-3d-planes", PLANES, render | sampled));
        let depth_view = view(&texture("lamina-3d-depth", DEPTH, render | sampled));
        let depths = texture("lamina-3d-depths", DEPTHS, render | copy);
        let picture = texture("lamina-3d-picture", COLOR, render | sampled | copy);
        let (ids_view, depths_view, picture_view) = (view(&ids), view(&depths), view(&picture));
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lamina-3d-targets"),
            layout: &self.edges_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&color) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&ids_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&planes) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&depth_view) },
            ],
        });
        if let Some(state) = &self.state {
            // egui shows the picture as it shows the processor's texture, filtered alike.
            let mut renderer = state.renderer.write();
            let filter = wgpu::FilterMode::Linear;
            match self.texture {
                Some(id) => renderer.update_egui_texture_from_wgpu_texture(&self.device, &picture_view, filter, id),
                None => self.texture = Some(renderer.register_native_texture(&self.device, &picture_view, filter)),
            }
        }
        self.targets =
            Some(Targets { size: [w, h], color, ids, ids_view, planes, depth_view, depths, depths_view, picture, picture_view, bind });
    }

    /// What the last picture shows at pixel (`x`, `y`), read back from the card: only a click asks, not every frame.
    pub fn at(&self, x: usize, y: usize) -> Option<Hit> {
        let t = self.targets.as_ref()?;
        let [w, h] = t.size;
        if x >= w as usize || y >= h as usize {
            return None;
        }
        let guard = scopes(&self.device);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("lamina-3d-pixel") });
        let origin = wgpu::Origin3d { x: x as u32, y: y as u32, z: 0 };
        let one = wgpu::Extent3d { width: 1, height: 1, depth_or_array_layers: 1 };
        let into = |offset| wgpu::TexelCopyBufferInfo {
            buffer: &self.readback,
            layout: wgpu::TexelCopyBufferLayout { offset, bytes_per_row: None, rows_per_image: None },
        };
        encoder.copy_texture_to_buffer(wgpu::TexelCopyTextureInfo { origin, ..t.ids.as_image_copy() }, into(0), one);
        encoder.copy_texture_to_buffer(wgpu::TexelCopyTextureInfo { origin, ..t.depths.as_image_copy() }, into(DEPTH_AT), one);
        self.queue.submit([encoder.finish()]);
        let (sent, mapped) = std::sync::mpsc::channel();
        self.readback.map_async(wgpu::MapMode::Read, .., move |r| {
            let _ = sent.send(r.is_ok());
        });
        let waited = self.device.poll(wgpu::PollType::wait_indefinitely()).is_ok();
        let hit = (waited && mapped.try_recv() == Ok(true)).then(|| self.readback.get_mapped_range(..).ok()).flatten().and_then(|bytes| {
            let word = |at: usize| bytes.get(at..at + 4).and_then(|b| b.try_into().ok()).map(u32::from_le_bytes);
            let (id, tri, depth) = (word(0)?, word(4)?, word(DEPTH_AT as usize)?);
            Some(Hit { id, tri, inv: f32::from_bits(depth), size: [w as usize, h as usize] })
        });
        self.readback.unmap();
        caught(guard).ok()?;
        hit
    }
}

fn attach(view: &wgpu::TextureView, clear: wgpu::Color) -> Option<wgpu::RenderPassColorAttachment<'_>> {
    Some(wgpu::RenderPassColorAttachment {
        view,
        depth_slice: None,
        resolve_target: None,
        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store },
    })
}

/// The picture egui showed goes with the card.
impl Drop for Gpu {
    fn drop(&mut self) {
        if let (Some(state), Some(id)) = (&self.state, self.texture) {
            state.renderer.write().free_texture(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lamina_core::Mesh;

    fn floats(bytes: &[u8]) -> Vec<f32> {
        bytes.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
    }

    #[test]
    fn a_scene_packs_three_vertices_a_triangle_each_with_its_face_s_normal_and_place() {
        let mut s = Scene::default();
        s.add("box", Mesh::cuboid([20.0, 10.0, 5.0])).unwrap();
        // A triangle with no area is left out, as the processor leaves it out; the run still counts from where the object starts.
        let flat = Mesh::new(vec![[0.0; 3], [1.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 1.0, 0.0]], vec![[0, 1, 2], [0, 1, 3]]).unwrap();
        s.add("sliver", flat).unwrap();
        let (bytes, runs) = pack(&s);
        assert_eq!(runs, [0..36, 36..39]);
        assert_eq!(bytes.len(), 39 * STRIDE);
        let world = s.objects[0].world();
        for (v, vertex) in bytes.as_chunks::<STRIDE>().0.iter().take(36).enumerate() {
            let f = floats(&vertex[..24]);
            let tri = u32::from_le_bytes([vertex[24], vertex[25], vertex[26], vertex[27]]) as usize;
            assert_eq!(tri, v / 3, "each triangle's place in its mesh");
            let corner = world.tri(tri).unwrap()[v % 3];
            assert!((0..3).all(|k| f[k] == corner[k] as f32), "where the world mesh has it");
            let n = [f[3], f[4], f[5]];
            assert!(((n[0] * n[0] + n[1] * n[1] + n[2] * n[2]) - 1.0).abs() < 1e-6, "a unit normal: {n:?}");
            assert_eq!(n.iter().filter(|c| c.abs() > 0.999).count(), 1, "a box's faces each face an axis: {n:?}");
        }
        // The sliver's one triangle with area is its second.
        assert_eq!(u32::from_le_bytes(bytes[36 * STRIDE + 24..36 * STRIDE + 28].try_into().unwrap()), 1);
    }

    #[test]
    fn the_globals_put_a_point_where_the_camera_projects_it_and_carry_the_edge_settings() {
        let camera = Camera { target: [5.0, -3.0, 12.0], azimuth: -50.0, elevation: 25.0, distance: 240.0 };
        let (w, h) = (640.0, 360.0);
        let g = globals(&camera, [640, 360]);
        for p in [[0.0, 0.0, 0.0], [40.0, -30.0, 20.0], [-100.0, 100.0, 0.01]] {
            let d = [p[0] as f32 - g[0][0], p[1] as f32 - g[0][1], p[2] as f32 - g[0][2]];
            let along = |v: [f32; 4]| d[0] * v[0] + d[1] * v[1] + d[2] * v[2];
            let ahead = along(g[3]);
            // From clip space to pixels as the card maps it: x right from the left edge, y down from the top.
            let (x, y) = ((along(g[1]) / ahead + 1.0) * w / 2.0, (1.0 - along(g[2]) / ahead) * h / 2.0);
            let q = camera.project(p, f64::from(w), f64::from(h)).unwrap();
            assert!((f64::from(x) - q[0]).abs() < 1e-3 && (f64::from(y) - q[1]).abs() < 1e-3, "{p:?}: {x} {y} against {q:?}");
            assert!((f64::from(render::NEAR as f32 / ahead) - q[2]).abs() < 1e-7, "the depth is one over the distance ahead");
        }
        let light = camera.light();
        assert!((0..3).all(|k| (f64::from(g[4][k]) - light[k]).abs() < 1e-6));
        assert_eq!(g[5], [35f64.to_radians().cos() as f32, 0.05, 0.42, 0.0]);
    }

    #[test]
    fn each_object_carries_its_id_its_colour_and_a_drag_s_transform() {
        let mut s = Scene::default();
        let a = s.add("a", Mesh::cuboid([10.0; 3])).unwrap();
        let b = s.add("b", Mesh::cuboid([10.0; 3])).unwrap();
        let moved = [(b, Transform::translate([3.0, 0.0, 0.0]))];
        let bytes = instances(&s, &[b], &moved);
        assert_eq!(bytes.len(), 4 * INSTANCE);
        let object = |k: usize| &bytes[k * INSTANCE..(k + 1) * INSTANCE];
        let id = |k: usize| u32::from_le_bytes(object(k)[..4].try_into().unwrap());
        assert_eq!([id(0), id(1), id(2), id(3)], [0, 0, a, b], "the plate's and the grid's first, then the objects'");
        assert_eq!([&object(0)[4..8], &object(1)[4..8]], [&render::PLATE[..], &render::GRID[..]]);
        assert_eq!((&object(2)[4..8], &object(3)[4..8]), (&render::MODEL[..], &render::SELECTED[..]));
        assert_eq!(floats(&object(2)[8..]), [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], "not moved");
        assert_eq!(floats(&object(3)[8..]), [1.0, 0.0, 0.0, 3.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0], "moved by the drag");
    }

    #[test]
    fn the_ground_is_the_plate_and_a_line_every_ten_millimetres() {
        let bytes = ground([220.0, 100.0]);
        // Two triangles, then 23 lines across x and 11 across y, two ends each.
        assert_eq!(bytes.len(), (6 + 2 * (23 + 11)) * STRIDE);
        let v: Vec<Vec<f32>> = bytes.as_chunks::<STRIDE>().0.iter().map(|c| floats(&c[..24])).collect();
        assert_eq!(&v[0][..3], [-110.0, -50.0, 0.0]);
        assert_eq!(&v[6][..3], [-110.0, -50.0, 0.01]);
        assert!(v.iter().all(|f| f[3..] == [0.0; 3]), "nothing on the ground is lit");
    }

    /// The shader must compile wherever the window runs; a machine with no card still checks it here.
    #[test]
    fn the_shader_is_valid_wgsl() {
        use wgpu::naga;
        let module = naga::front::wgsl::parse_str(include_str!("gpu.wgsl")).unwrap();
        naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty()).validate(&module).unwrap();
    }

    /// The machine's graphics card with no window to show on; none where there is none, or only OpenGL.
    fn headless() -> Option<Gpu> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            flags: wgpu::InstanceFlags::empty(),
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()?;
        if adapter.get_info().backend == wgpu::Backend::Gl {
            return None;
        }
        eprintln!("card: {} ({:?})", adapter.get_info().name, adapter.get_info().backend);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor::default())).ok()?;
        Some(Gpu::from_device(device, queue).unwrap())
    }

    /// The picture's pixels, read back from the card.
    fn read(gpu: &Gpu) -> Vec<Rgba> {
        let texture = &gpu.targets.as_ref().unwrap().picture;
        let (w, h) = (texture.width() as usize, texture.height() as usize);
        let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (row * h) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = gpu.device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row as u32), rows_per_image: None },
            },
            texture.size(),
        );
        gpu.queue.submit([encoder.finish()]);
        buffer.map_async(wgpu::MapMode::Read, .., |_| ());
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let mapped = buffer.get_mapped_range(..).unwrap();
        mapped.chunks_exact(row).flat_map(|r| r[..w * 4].as_chunks::<4>().0.to_vec()).collect()
    }

    /// A plate with a box, a cylinder, a sphere, a part with a step and a pocket, one picked, one being dragged.
    fn plate() -> Scene {
        let mut s = Scene::default();
        s.add("box", Mesh::cuboid([30.0, 20.0, 25.0])).unwrap();
        let c = s.add("cylinder", Mesh::cylinder(12.0, 30.0, 64)).unwrap();
        s.get_mut(c).unwrap().xf = Transform::translate([40.0, 10.0, 0.0]);
        let ball = s.add("ball", Mesh::sphere(15.0, 48)).unwrap();
        s.get_mut(ball).unwrap().xf = Transform::translate([-40.0, 15.0, 0.0]);
        let plinth = Mesh::cuboid([40.0, 30.0, 8.0]);
        let block = Mesh::cuboid([14.0, 10.0, 3.0]).transformed(&Transform::translate([0.0, 0.0, 8.0]));
        let stepped = s.add("stepped", Mesh::merged([&plinth, &block])).unwrap();
        s.get_mut(stepped).unwrap().xf = Transform::translate([0.0, -45.0, 0.0]);
        s
    }

    /// The card's picture of a plate against the processor's: the same pixels but for a few along edges, the same object and triangle
    /// under the pointer, and the same point measured.
    #[test]
    #[ignore = "needs a graphics card: cargo test -p lamina -- --ignored the_card_draws"]
    fn the_card_draws_what_the_processor_draws() {
        let Some(mut gpu) = headless() else { return eprintln!("no graphics card: nothing checked") };
        let scene = Arc::new(plate());
        let ids: Vec<u32> = scene.objects.iter().map(|o| o.id).collect();
        let (w, h) = (800, 600);
        for (camera, selected, moved) in [
            (Camera { target: [0.0, -5.0, 10.0], azimuth: -60.0, elevation: 30.0, distance: 260.0 }, vec![], vec![]),
            (Camera { target: [0.0, -5.0, 10.0], azimuth: 25.0, elevation: 55.0, distance: 200.0 }, vec![ids[1]], vec![]),
            (
                Camera { target: [0.0, -5.0, 10.0], azimuth: -120.0, elevation: 15.0, distance: 300.0 },
                vec![ids[0]],
                vec![(ids[0], Transform::translate([0.0, 0.0, 6.0]))],
            ),
        ] {
            let mut seen = Scene::clone(&scene);
            for (id, t) in &moved {
                let o = seen.get_mut(*id).unwrap();
                o.xf = o.xf.then(t);
            }
            let cpu = render::draw(&seen, &selected, &camera, [220.0, 220.0], [w, h]);
            gpu.draw(&scene, &selected, &moved, &camera, [220.0, 220.0], [w, h]).unwrap();
            let pixels = read(&gpu);
            assert_eq!(pixels.len(), cpu.pixels.len());
            let far = |a: &Rgba, b: &Rgba| a[..3].iter().zip(&b[..3]).any(|(x, y)| x.abs_diff(*y) > 6);
            let model = (0..w * h).filter(|&k| cpu.id_at(k % w, k / w).is_some()).count();
            let wrong = (0..w * h).filter(|&k| far(&pixels[k], &cpu.pixels[k])).count();
            let wrong_in_model = (0..w * h).filter(|&k| cpu.id_at(k % w, k / w).is_some() && far(&pixels[k], &cpu.pixels[k])).count();
            eprintln!(
                "azimuth {}: {wrong} of {} pixels differ by more than 6 levels ({:.2}%), {wrong_in_model} of the {model} on objects ({:.2}%)",
                camera.azimuth,
                w * h,
                100.0 * wrong as f64 / (w * h) as f64,
                100.0 * wrong_in_model as f64 / model as f64
            );
            if let Ok(dir) = std::env::var("LAMINA_GPU_DUMP") {
                let png = |name: &str, p: &[Rgba]| std::fs::write(format!("{dir}/{name}.png"), render::png_rgba([w, h], p)).unwrap();
                png(&format!("card-{}", camera.azimuth), &pixels);
                png(&format!("processor-{}", camera.azimuth), &cpu.pixels);
            }
            assert!(wrong_in_model * 100 < model, "more than 1% of the objects' pixels differ");
            // Off the objects, only the grid differs: the processor steps a line from its end rather than through pixel centres, and
            // loses some of a far line to the plate a hundredth of a millimetre below it, where the card draws it whole.
            assert!(wrong * 100 < 3 * w * h, "more than 3% of the picture differs");
            // Picking, lay flat and measuring read the same at points away from an edge (where the 3x3 round a pixel is one triangle)
            // and off the grid's lines, whose depth is the line's and, on the processor, a point up to half a pixel off the middle.
            let (mut checked, mut same) = (0, 0);
            for y in (5..h - 5).step_by(23) {
                for x in (5..w - 5).step_by(29) {
                    let around = (-1..=1).flat_map(|dy| (-1..=1).map(move |dx| ((x as i64 + dx) as usize, (y as i64 + dy) as usize)));
                    let face = cpu.face_at(x, y);
                    let grid = |k: usize| pixels[k] == render::GRID || cpu.pixels[k] == render::GRID;
                    if around.clone().any(|(a, b)| cpu.face_at(a, b) != face || grid(b * w + a)) {
                        continue;
                    }
                    let hit = gpu.at(x, y).unwrap();
                    checked += 1;
                    let id = (hit.id != 0).then_some(hit.id);
                    assert_eq!(id, cpu.id_at(x, y), "the object at {x}, {y}");
                    if face.is_some() {
                        assert_eq!(Some((hit.id, hit.tri as usize)), face, "the triangle at {x}, {y}");
                    }
                    let (a, b) = (camera.at_depth(x, y, hit.size, hit.inv), cpu.point_at(&camera, x, y));
                    assert_eq!(a.is_some(), b.is_some());
                    if let (Some(a), Some(b)) = (a, b) {
                        let gap = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
                        assert!(gap < 0.05, "the point at {x}, {y}: {a:?} against {b:?}");
                    }
                    same += 1;
                }
            }
            eprintln!("  picking: {same} of {checked} sample points read the same object, triangle and point");
        }
    }

    /// About 500,000 triangles (25 spheres of 20,480), drawn on both: what the window waits for as the camera turns.
    #[test]
    #[ignore = "a benchmark (dev builds are optimised here): cargo test -p lamina -- --ignored --nocapture the_card_and_the_processor_timed"]
    fn the_card_and_the_processor_timed() {
        use std::time::Instant;
        let Some(mut gpu) = headless() else { return eprintln!("no graphics card: nothing timed") };
        let mut s = Scene::default();
        for k in 0..25 {
            let id = s.add("ball", Mesh::sphere(9.0, 192)).unwrap();
            s.get_mut(id).unwrap().xf = Transform::translate([f64::from(k % 5) * 20.0 - 40.0, f64::from(k / 5) * 20.0 - 40.0, 0.0]);
        }
        let scene = Arc::new(s);
        let triangles: usize = scene.objects.iter().map(|o| o.mesh.triangles.len()).sum();
        let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
        let size = [1600, 1000];
        let cameras: Vec<Camera> = (0..30).map(|i| Camera { azimuth: -60.0 + f64::from(i) * 3.0, ..Camera::default() }).collect();
        let t = Instant::now();
        gpu.draw(&scene, &[], &[], &cameras[0], [220.0, 220.0], size).unwrap();
        gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        eprintln!("{triangles} triangles, {}x{} pixels", size[0], size[1]);
        eprintln!("card: first picture (packs and uploads the scene) {:.1} ms", ms(t));
        let mut frames: Vec<f64> = cameras
            .iter()
            .map(|c| {
                let t = Instant::now();
                gpu.draw(&scene, &[], &[], c, [220.0, 220.0], size).unwrap();
                gpu.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
                ms(t)
            })
            .collect();
        frames.sort_by(f64::total_cmp);
        eprintln!("card: each picture as the camera turns, median {:.2} ms, slowest {:.2} ms", frames[15], frames[29]);
        let t = Instant::now();
        let hit = gpu.at(800, 500);
        eprintln!("card: reading back a pixel {:.2} ms ({hit:?})", ms(t));
        let mut cpu: Vec<f64> = cameras[..5]
            .iter()
            .map(|c| {
                let t = Instant::now();
                std::hint::black_box(render::draw(&scene, &[], c, [220.0, 220.0], size));
                ms(t)
            })
            .collect();
        cpu.sort_by(f64::total_cmp);
        eprintln!("processor: each picture, median {:.1} ms, slowest {:.1} ms", cpu[2], cpu[4]);
    }
}

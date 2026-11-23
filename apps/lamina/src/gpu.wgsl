// The 3D view on the graphics card (`gpu.rs`): the picture the processor's rasterizer (`lamina_core::render::draw`) makes, in two
// passes.
//   1. vs_scene / fs_scene: the plate, then the grid's lines (the same shaders, as a line list, into the colour and the depth only),
//      then each object's triangles, flat shaded, into a colour target, an id target (the object and its triangle) and a plane
//      target (the face's unit normal turned to the eye, and its distance from the origin), with their depth: one over the distance
//      ahead of the eye, larger nearer, as `Picture` keeps it.
//   2. vs_full / fs_edges: each pixel darkened where it is the nearer of two neighbours on different objects, or on one object's
//      faces meeting at a crease or a step, as `Raster::outline` marks them; and the depth copied to a target a click reads back
//      a pixel of (a depth texture is copied whole or not at all).
// A colour is 0 to 255 in each channel, as the processor keeps it, and is divided down only on its way out.

struct Globals {
    // The eye, mm.
    eye: vec4<f32>,
    // The picture's right and up, each times the focal length over half the picture's width (or height): `dot(d, right) / ahead`
    // is where a point `d` from the eye lands across the picture, -1 to 1.
    right: vec4<f32>,
    up: vec4<f32>,
    forward: vec4<f32>,
    light: vec4<f32>,
    // x: the cosine of the crease angle; y: the step between parallel faces, mm; z: how much an edge darkens a pixel.
    edges: vec4<f32>,
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var scene_color: texture_2d<f32>;
@group(0) @binding(2) var scene_ids: texture_2d<u32>;
@group(0) @binding(3) var scene_planes: texture_2d<f32>;
@group(0) @binding(4) var scene_depth: texture_depth_2d;

// The nearest the eye sees, mm (`render::NEAR`): the card's depth is NEAR over the distance ahead, 1 at the near plane.
const NEAR: f32 = 1.0;

struct VertexIn {
    @location(0) position: vec3<f32>,
    // The triangle's unit normal; none for the plate and the grid, which are not lit.
    @location(1) normal: vec3<f32>,
    // The triangle's place in its object's mesh.
    @location(2) tri: u32,
    // The object's: its id and colour (red in the low byte), and the transform a drag shows it moved by, as three rows.
    @location(3) object: vec2<u32>,
    @location(4) row0: vec4<f32>,
    @location(5) row1: vec4<f32>,
    @location(6) row2: vec4<f32>,
}

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) @interpolate(flat) color: vec3<f32>,
    @location(1) @interpolate(flat) ids: vec2<u32>,
    @location(2) @interpolate(flat) plane: vec4<f32>,
}

@vertex
fn vs_scene(v: VertexIn) -> VertexOut {
    let p = vec4<f32>(v.position, 1.0);
    let world = vec3<f32>(dot(v.row0, p), dot(v.row1, p), dot(v.row2, p));
    let d = world - g.eye.xyz;
    var o: VertexOut;
    o.clip = vec4<f32>(dot(d, g.right.xyz), dot(d, g.up.xyz), NEAR, dot(d, g.forward.xyz));
    o.ids = vec2<u32>(v.object.x, v.tri);
    let base = unpack4x8unorm(v.object.y).xyz * 255.0;
    // What is not a part's face (the plate) is level, through the origin: `render::FLAT`.
    o.plane = vec4<f32>(0.0, 0.0, 1.0, 0.0);
    var k = 1.0;
    if dot(v.normal, v.normal) > 0.0 {
        // A drag turns and scales evenly, so the turned normal, made unit again, is the face's.
        let n = normalize(vec3<f32>(dot(v.row0.xyz, v.normal), dot(v.row1.xyz, v.normal), dot(v.row2.xyz, v.normal)));
        // Both sides lit alike, as `Raster::shaded` lights them; the plane's normal turned to the eye, through the first corner.
        k = 0.35 + 0.65 * abs(dot(n, g.light.xyz));
        let u = select(n, -n, dot(n, g.forward.xyz) > 0.0);
        o.plane = vec4<f32>(u, dot(u, world));
    }
    o.color = clamp(floor(base * k + vec3<f32>(0.5)), vec3<f32>(0.0), vec3<f32>(255.0));
    return o;
}

struct Targets {
    @location(0) color: vec4<f32>,
    @location(1) ids: vec2<u32>,
    @location(2) plane: vec4<f32>,
}

@fragment
fn fs_scene(in: VertexOut) -> Targets {
    var out: Targets;
    out.color = vec4<f32>(in.color / 255.0, 1.0);
    out.ids = in.ids;
    out.plane = in.plane;
    return out;
}

// One triangle over the whole picture.
@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

// Whether two neighbouring pixels show different surfaces: different objects (or an object and the plate), or one object's faces
// meeting at a crease or a step between parallel faces. The plate and the background are one.
fn differs(a: vec2<i32>, b: vec2<i32>) -> bool {
    let ia = textureLoad(scene_ids, a, 0).x;
    if ia != textureLoad(scene_ids, b, 0).x {
        return true;
    }
    if ia == 0u {
        return false;
    }
    let p = textureLoad(scene_planes, a, 0);
    let q = textureLoad(scene_planes, b, 0);
    let along = dot(p.xyz, q.xyz);
    return along < g.edges.x || (along > 0.999 && abs(p.w - q.w) > g.edges.y);
}

fn depth_at(p: vec2<i32>) -> f32 {
    return textureLoad(scene_depth, p, 0);
}

struct Picture {
    @location(0) color: vec4<f32>,
    @location(1) depth: f32,
}

@fragment
fn fs_edges(@builtin(position) frag: vec4<f32>) -> Picture {
    let p = vec2<i32>(frag.xy);
    let size = vec2<i32>(textureDimensions(scene_ids));
    let here = depth_at(p);
    let right = p + vec2<i32>(1, 0);
    let below = p + vec2<i32>(0, 1);
    let left = p - vec2<i32>(1, 0);
    let above = p - vec2<i32>(0, 1);
    // Of two pixels either side of an edge the nearer is darkened, the first of two as near: the processor marks each pair from
    // its first pixel, so a pixel is marked by the pair it starts when it is as near, and by the pair it ends when it is nearer.
    var edge = (right.x < size.x && differs(p, right) && here >= depth_at(right))
        || (below.y < size.y && differs(p, below) && here >= depth_at(below));
    edge = edge || (left.x >= 0 && differs(left, p) && depth_at(left) < here)
        || (above.y >= 0 && differs(above, p) && depth_at(above) < here);
    var rgb = floor(textureLoad(scene_color, p, 0).xyz * 255.0 + vec3<f32>(0.5));
    if edge {
        rgb = floor(rgb * g.edges.z + vec3<f32>(0.5));
    }
    var out: Picture;
    out.color = vec4<f32>(rgb / 255.0, 1.0);
    out.depth = here;
    return out;
}

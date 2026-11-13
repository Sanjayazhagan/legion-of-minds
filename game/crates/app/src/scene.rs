//! 3D models: a small glTF loader with skeletal animation, skinned on the CPU
//! and drawn through macroquad meshes with baked lighting. Only reads the sim.

use macroquad::models::{draw_mesh, Mesh, Vertex};
use macroquad::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

struct Node {
    name: String,
    parent: Option<usize>,
    t: Vec3,
    r: Quat,
    s: Vec3,
}

struct Part {
    node: usize,
    skinned: bool,
    pos: Vec<Vec3>,
    nrm: Vec<Vec3>,
    /// Normals averaged over vertices that share a position, so the outline
    /// shell pushed out along them has no cracks at hard edges.
    onrm: Vec<Vec3>,
    uv: Vec<Vec2>,
    joints: Vec<[u16; 4]>,
    weights: Vec<[f32; 4]>,
    idx: Vec<u16>,
    tex: Option<usize>,
}

enum Values {
    V3(Vec<Vec3>),
    Q(Vec<Quat>),
}

struct Channel {
    node: usize,
    /// 0 translation, 1 rotation, 2 scale.
    prop: u8,
    times: Vec<f32>,
    values: Values,
}

struct Clip {
    duration: f32,
    channels: Vec<Channel>,
}

pub struct Model {
    nodes: Vec<Node>,
    /// Node indices, parents before children.
    order: Vec<usize>,
    parts: Vec<Part>,
    joints: Vec<usize>,
    inv_bind: Vec<Mat4>,
    clips: HashMap<String, Clip>,
    textures: Vec<Texture2D>,
}

fn load_err(p: &Path, e: impl std::fmt::Display) -> String {
    format!("{}: {e}", p.display())
}

impl Model {
    /// Load a .glb or .gltf (with its .bin and textures next to it).
    pub fn load(path: &Path) -> Result<Model, String> {
        let bytes = std::fs::read(path).map_err(|e| load_err(path, e))?;
        let g = gltf::Gltf::from_slice(&bytes).map_err(|e| load_err(path, e))?;
        let dir = path.parent().unwrap_or(Path::new("."));
        let mut buffers: Vec<Vec<u8>> = Vec::new();
        for b in g.document.buffers() {
            match b.source() {
                gltf::buffer::Source::Bin => buffers.push(g.blob.clone().unwrap_or_default()),
                gltf::buffer::Source::Uri(u) => {
                    buffers.push(std::fs::read(dir.join(u)).map_err(|e| load_err(&dir.join(u), e))?)
                }
            }
        }
        let mut textures = Vec::new();
        for im in g.document.images() {
            let data = match im.source() {
                gltf::image::Source::View { view, .. } => {
                    let b = &buffers[view.buffer().index()];
                    b[view.offset()..view.offset() + view.length()].to_vec()
                }
                gltf::image::Source::Uri { uri, .. } => {
                    std::fs::read(dir.join(uri)).map_err(|e| load_err(&dir.join(uri), e))?
                }
            };
            let t = Texture2D::from_file_with_format(&data, None);
            t.set_filter(FilterMode::Linear);
            textures.push(t);
        }
        let get = |b: gltf::Buffer| Some(&buffers[b.index()][..]);

        let mut nodes: Vec<Node> = g
            .document
            .nodes()
            .map(|n| {
                let (t, r, s) = n.transform().decomposed();
                Node {
                    name: n.name().unwrap_or("").to_string(),
                    parent: None,
                    t: Vec3::from(t),
                    r: Quat::from_array(r),
                    s: Vec3::from(s),
                }
            })
            .collect();
        for n in g.document.nodes() {
            for c in n.children() {
                nodes[c.index()].parent = Some(n.index());
            }
        }
        let mut order = Vec::new();
        let mut stack: Vec<usize> = (0..nodes.len()).filter(|&i| nodes[i].parent.is_none()).collect();
        let kids: Vec<Vec<usize>> = g.document.nodes().map(|n| n.children().map(|c| c.index()).collect()).collect();
        while let Some(i) = stack.pop() {
            order.push(i);
            stack.extend(kids[i].iter().copied());
        }

        let mut parts = Vec::new();
        for n in g.document.nodes() {
            let Some(mesh) = n.mesh() else { continue };
            for prim in mesh.primitives() {
                let r = prim.reader(get);
                let pos: Vec<Vec3> = r.read_positions().map(|p| p.map(Vec3::from).collect()).unwrap_or_default();
                let nrm: Vec<Vec3> =
                    r.read_normals().map(|p| p.map(Vec3::from).collect()).unwrap_or_else(|| vec![Vec3::Y; pos.len()]);
                let uv: Vec<Vec2> = r
                    .read_tex_coords(0)
                    .map(|t| t.into_f32().map(Vec2::from).collect())
                    .unwrap_or_else(|| vec![Vec2::ZERO; pos.len()]);
                let joints: Vec<[u16; 4]> = r.read_joints(0).map(|j| j.into_u16().collect()).unwrap_or_default();
                let weights: Vec<[f32; 4]> = r.read_weights(0).map(|w| w.into_f32().collect()).unwrap_or_default();
                let idx: Vec<u16> = r
                    .read_indices()
                    .map(|i| i.into_u32().map(|v| v as u16).collect())
                    .unwrap_or_else(|| (0..pos.len() as u16).collect());
                let tex = prim.material().pbr_metallic_roughness().base_color_texture().map(|t| t.texture().source().index());
                let skinned = n.skin().is_some() && !joints.is_empty();
                let onrm = smooth_normals(&pos, &nrm);
                parts.push(Part { node: n.index(), skinned, pos, nrm, onrm, uv, joints, weights, idx, tex });
            }
        }
        let (mut joints, mut inv_bind) = (Vec::new(), Vec::new());
        if let Some(skin) = g.document.skins().next() {
            joints = skin.joints().map(|j| j.index()).collect();
            inv_bind = skin
                .reader(get)
                .read_inverse_bind_matrices()
                .map(|m| m.map(|c| Mat4::from_cols_array_2d(&c)).collect())
                .unwrap_or_else(|| vec![Mat4::IDENTITY; joints.len()]);
        }
        let mut clips = HashMap::new();
        for a in g.document.animations() {
            let mut channels = Vec::new();
            let mut duration = 0.0f32;
            for ch in a.channels() {
                let r = ch.reader(get);
                let times: Vec<f32> = r.read_inputs().map(|i| i.collect()).unwrap_or_default();
                duration = duration.max(times.last().copied().unwrap_or(0.0));
                use gltf::animation::util::ReadOutputs;
                let (prop, values) = match r.read_outputs() {
                    Some(ReadOutputs::Translations(v)) => (0, Values::V3(v.map(Vec3::from).collect())),
                    Some(ReadOutputs::Rotations(v)) => (1, Values::Q(v.into_f32().map(Quat::from_array).collect())),
                    Some(ReadOutputs::Scales(v)) => (2, Values::V3(v.map(Vec3::from).collect())),
                    _ => continue,
                };
                channels.push(Channel { node: ch.target().node().index(), prop, times, values });
            }
            clips.insert(a.name().unwrap_or("").to_string(), Clip { duration, channels });
        }
        Ok(Model { nodes, order, parts, joints, inv_bind, clips, textures })
    }

    /// True for a mesh node that is not skinned (weapons, hats, shields).
    pub fn is_rigid_mesh(&self, name: &str) -> bool {
        self.parts.iter().any(|p| !p.skinned && self.nodes[p.node].name == name)
    }

    pub fn node(&self, name: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n.name == name)
    }

    /// Length of `clip` in seconds.
    pub fn clip_len(&self, clip: &str) -> Option<f32> {
        self.clips.get(clip).map(|c| c.duration)
    }

    /// Global transform of every node for `clip` at time `t` (seconds).
    pub fn pose(&self, clip: Option<&str>, t: f32, looped: bool) -> Vec<Mat4> {
        self.globals(&self.sample(clip, t, looped))
    }

    /// `from` faded into `to`: `w` 0 is all `from`, 1 all `to`. Switching
    /// clips through this instead of cutting keeps characters from popping.
    pub fn pose_blend(&self, from: (Option<&str>, f32, bool), to: (Option<&str>, f32, bool), w: f32) -> Vec<Mat4> {
        if w >= 1.0 {
            return self.pose(to.0, to.1, to.2);
        }
        let a = self.sample(from.0, from.1, from.2);
        let b = self.sample(to.0, to.1, to.2);
        let mixed: Vec<(Vec3, Quat, Vec3)> =
            a.iter().zip(&b).map(|(x, y)| (x.0.lerp(y.0, w), x.1.slerp(y.1, w), x.2.lerp(y.2, w))).collect();
        self.globals(&mixed)
    }

    /// Local translation, rotation and scale of every node.
    fn sample(&self, clip: Option<&str>, t: f32, looped: bool) -> Vec<(Vec3, Quat, Vec3)> {
        let mut trs: Vec<(Vec3, Quat, Vec3)> = self.nodes.iter().map(|n| (n.t, n.r, n.s)).collect();
        if let Some(c) = clip.and_then(|c| self.clips.get(c)) {
            let t = if looped && c.duration > 0.0 { t.rem_euclid(c.duration) } else { t.clamp(0.0, c.duration) };
            for ch in &c.channels {
                let ts = &ch.times;
                if ts.is_empty() {
                    continue;
                }
                let k = ts.partition_point(|&x| x <= t);
                let (i0, i1, f) = if k == 0 {
                    (0, 0, 0.0)
                } else if k >= ts.len() {
                    (ts.len() - 1, ts.len() - 1, 0.0)
                } else {
                    let (a, b) = (ts[k - 1], ts[k]);
                    (k - 1, k, if b > a { (t - a) / (b - a) } else { 0.0 })
                };
                let e = &mut trs[ch.node];
                match &ch.values {
                    Values::V3(v) => {
                        let x = v[i0].lerp(v[i1], f);
                        if ch.prop == 0 {
                            e.0 = x
                        } else {
                            e.2 = x
                        }
                    }
                    Values::Q(v) => e.1 = v[i0].slerp(v[i1], f),
                }
            }
        }
        trs
    }

    fn globals(&self, trs: &[(Vec3, Quat, Vec3)]) -> Vec<Mat4> {
        let mut g = vec![Mat4::IDENTITY; self.nodes.len()];
        for &i in &self.order {
            let (t, r, s) = trs[i];
            let local = Mat4::from_scale_rotation_translation(s, r, t);
            g[i] = match self.nodes[i].parent {
                Some(p) => g[p] * local,
                None => local,
            };
        }
        g
    }

    /// Draw with the node globals from `pose`, placed by `world`.
    /// `show(name)` decides which mesh nodes are drawn.
    pub fn draw(&self, world: Mat4, globals: &[Mat4], show: &dyn Fn(&str) -> bool, tint: Color) {
        self.draw_ex(world, globals, show, &|_| (tint, 0.0));
    }

    /// Like `draw`, with a colour per mesh node: `paint(name)` gives the tint
    /// and how much (0 to 1) of the texture's own colour it replaces. Inside
    /// `toon_style` the model is drawn cel-shaded with a rim light and an
    /// outline; otherwise with the plain baked light.
    pub fn draw_ex(&self, world: Mat4, globals: &[Mat4], show: &dyn Fn(&str) -> bool, paint: &dyn Fn(&str) -> (Color, f32)) {
        let jm: Vec<Mat4> = self.joints.iter().zip(&self.inv_bind).map(|(&j, ib)| globals[j] * *ib).collect();
        let style = STYLE.with(|s| s.get());
        let toon = style.toon.filter(|_| materials().is_some());
        // Outline width in world units: about `px` pixels at this distance.
        let (eye, out_w) = match toon {
            Some(t) => {
                let at = world.transform_point3(Vec3::ZERO);
                (style.eye, t.outline_px * style.px_per_dist * (style.eye - at).length())
            }
            None => (style.eye, 0.0),
        };
        SCRATCH.with(|sc| {
            let (verts, shell) = &mut *sc.borrow_mut();
            for p in &self.parts {
                let name = &self.nodes[p.node].name;
                if !show(name) {
                    continue;
                }
                let (tint, recolor) = paint(name);
                verts.clear();
                shell.clear();
                let rigid = world * globals[p.node];
                for i in 0..p.pos.len() {
                    let m = if p.skinned {
                        let (j, w) = (p.joints[i], p.weights[i]);
                        world
                            * (jm[j[0] as usize] * w[0]
                                + jm[j[1] as usize] * w[1]
                                + jm[j[2] as usize] * w[2]
                                + jm[j[3] as usize] * w[3])
                    } else {
                        rigid
                    };
                    let pw = m.transform_point3(p.pos[i]);
                    let nw = m.transform_vector3(p.nrm[i]).normalize_or_zero();
                    match toon {
                        Some(t) => {
                            let (light, rim) = toon_shade(nw, (eye - pw).normalize_or_zero());
                            verts.push(Vertex {
                                position: pw,
                                uv: p.uv[i],
                                color: to_u8(tint, 1.0),
                                normal: vec4(rim * t.rim, recolor, light, 0.0),
                            });
                            if out_w > 0.0 {
                                let on = m.transform_vector3(p.onrm[i]).normalize_or_zero();
                                shell.push(Vertex {
                                    position: pw + on * out_w,
                                    uv: Vec2::ZERO,
                                    color: to_u8(t.outline, 1.0),
                                    normal: Vec4::ZERO,
                                });
                            }
                        }
                        None => verts.push(Vertex {
                            position: pw,
                            uv: p.uv[i],
                            color: to_u8(tint, shade(nw)),
                            normal: Vec4::ZERO,
                        }),
                    }
                }
                let texture = p.tex.and_then(|t| self.textures.get(t).cloned());
                if let (Some(mats), false) = (toon.and(materials()), shell.is_empty()) {
                    let mesh = Mesh { vertices: std::mem::take(shell), indices: p.idx.clone(), texture: None };
                    gl_use_material(&mats.outline);
                    draw_mesh(&mesh);
                    *shell = mesh.vertices;
                }
                let mesh = Mesh { vertices: std::mem::take(verts), indices: p.idx.clone(), texture };
                match toon.and(materials()) {
                    Some(mats) => {
                        gl_use_material(&mats.toon);
                        draw_mesh(&mesh);
                        gl_use_default_material();
                    }
                    None => draw_mesh(&mesh),
                }
                *verts = mesh.vertices;
            }
        });
    }

    /// Draw a static model in its rest pose.
    pub fn draw_static(&self, world: Mat4, tint: Color) {
        STATIC_POSE.with(|c| {
            let key = self as *const Model as usize;
            let mut cache = c.borrow_mut();
            let g = cache.entry(key).or_insert_with(|| self.pose(None, 0.0, false));
            let g = g.clone();
            drop(cache);
            self.draw(world, &g, &|_| true, tint);
        });
    }
}

thread_local! {
    static SCRATCH: RefCell<(Vec<Vertex>, Vec<Vertex>)> = RefCell::new((Vec::new(), Vec::new()));
    static STATIC_POSE: RefCell<HashMap<usize, Vec<Mat4>>> = RefCell::new(HashMap::new());
}

/// Sun from the upper left, plus sky light: a soft toon-ish look.
fn shade(n: Vec3) -> f32 {
    let n = n.normalize_or_zero();
    let sun = Vec3::new(-0.45, 0.8, 0.4).normalize();
    let d = n.dot(sun).max(0.0);
    let sky = 0.5 + 0.5 * n.y;
    0.42 + 0.48 * d + 0.18 * sky
}

fn to_u8(c: Color, l: f32) -> [u8; 4] {
    [
        (c.r * l * 255.0).min(255.0) as u8,
        (c.g * l * 255.0).min(255.0) as u8,
        (c.b * l * 255.0).min(255.0) as u8,
        (c.a * 255.0) as u8,
    ]
}

/// Cel shading for characters: a lit side and a shadow side with a soft
/// edge between them (so shapes read at a glance, like hand-painted toys),
/// plus a rim of light along the silhouette. Returns (light, rim).
fn toon_shade(n: Vec3, to_eye: Vec3) -> (f32, f32) {
    let sun = Vec3::new(-0.45, 0.8, 0.4).normalize();
    let d = n.dot(sun);
    let band = ((d + 0.1) / 0.25).clamp(0.0, 1.0);
    let band = band * band * (3.0 - 2.0 * band);
    let sky = 0.5 + 0.5 * n.y;
    let light = 0.6 + 0.34 * band + 0.08 * sky;
    let facing = n.dot(to_eye).max(0.0);
    let rim = (1.0 - facing).powi(3) * (0.35 + 0.65 * sky);
    (light, rim)
}

/// Normals averaged over every vertex at the same spot.
fn smooth_normals(pos: &[Vec3], nrm: &[Vec3]) -> Vec<Vec3> {
    let key = |p: Vec3| ((p.x * 1e4) as i64, (p.y * 1e4) as i64, (p.z * 1e4) as i64);
    let mut sum: HashMap<(i64, i64, i64), Vec3> = HashMap::new();
    for (p, n) in pos.iter().zip(nrm) {
        *sum.entry(key(*p)).or_insert(Vec3::ZERO) += n.normalize_or_zero();
    }
    pos.iter().zip(nrm).map(|(p, n)| sum[&key(*p)].try_normalize().unwrap_or(*n)).collect()
}

/// How characters are drawn inside `toon_style`.
#[derive(Clone, Copy)]
pub struct Toon {
    /// Outline thickness in screen pixels (0 for none).
    pub outline_px: f32,
    pub outline: Color,
    /// Strength of the rim light.
    pub rim: f32,
}

#[derive(Clone, Copy)]
struct Style {
    toon: Option<Toon>,
    /// Where the camera is.
    eye: Vec3,
    /// World units per screen pixel, per unit of distance from the camera.
    px_per_dist: f32,
}

thread_local! {
    static STYLE: std::cell::Cell<Style> =
        const { std::cell::Cell::new(Style { toon: None, eye: Vec3::ZERO, px_per_dist: 0.0 }) };
    static MATS: RefCell<Option<Option<std::rc::Rc<Mats>>>> = const { RefCell::new(None) };
}

/// Tell the model drawing about the camera of the 3D view being started.
pub fn set_view(eye: Vec3, fovy: f32, viewport_h_px: f32) {
    STYLE.with(|s| {
        let mut st = s.get();
        st.eye = eye;
        st.px_per_dist = 2.0 * (fovy / 2.0).tan() / viewport_h_px.max(1.0);
        s.set(st);
    });
}

/// Draw everything inside `f` in the character style.
pub fn toon_style<R>(t: Toon, f: impl FnOnce() -> R) -> R {
    let before = STYLE.with(|s| s.get());
    STYLE.with(|s| s.set(Style { toon: Some(t), ..before }));
    let r = f();
    STYLE.with(|s| s.set(Style { toon: before.toon, ..s.get() }));
    r
}

struct Mats {
    toon: Material,
    outline: Material,
}

const TOON_VERTEX: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;
attribute vec4 normal;

varying lowp vec2 uv;
varying lowp vec4 color;
varying mediump vec3 extra;

uniform mat4 Model;
uniform mat4 Projection;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
    uv = texcoord;
    extra = normal.xyz;
}"#;

// extra: x rim light, y how much of the texture's colour the tint replaces,
// z the cel-shaded light.
const TOON_FRAGMENT: &str = r#"#version 100
varying lowp vec4 color;
varying lowp vec2 uv;
varying mediump vec3 extra;

uniform sampler2D Texture;

void main() {
    lowp vec4 t = texture2D(Texture, uv);
    mediump float lum = dot(t.rgb, vec3(0.299, 0.587, 0.114));
    mediump vec3 base = mix(t.rgb, vec3(min(lum * 1.5 + 0.12, 1.0)), extra.y);
    mediump vec3 c = base * color.rgb * extra.z;
    mediump float l2 = dot(c, vec3(0.299, 0.587, 0.114));
    c = mix(vec3(l2), c, 1.22);
    // Rim light fades on surfaces that are already bright (bone, steel),
    // so pale models don't burn out to white.
    c += extra.x * (1.0 - 0.8 * l2) * vec3(1.0, 0.96, 0.88);
    gl_FragColor = vec4(c, color.a * t.a);
}"#;

const OUTLINE_VERTEX: &str = r#"#version 100
attribute vec3 position;
attribute vec2 texcoord;
attribute vec4 color0;

varying lowp vec4 color;

uniform mat4 Model;
uniform mat4 Projection;

void main() {
    gl_Position = Projection * Model * vec4(position, 1);
    color = color0 / 255.0;
}"#;

const OUTLINE_FRAGMENT: &str = r#"#version 100
varying lowp vec4 color;

void main() {
    gl_FragColor = color;
}"#;

/// The character shaders, built on first use; `None` if the graphics driver
/// refuses them (characters then fall back to the plain look).
fn materials() -> Option<std::rc::Rc<Mats>> {
    MATS.with(|m| {
        m.borrow_mut()
            .get_or_insert_with(|| {
                let depth = |cull| macroquad::miniquad::PipelineParams {
                    depth_test: macroquad::miniquad::Comparison::LessOrEqual,
                    depth_write: true,
                    cull_face: cull,
                    color_blend: Some(macroquad::miniquad::BlendState::new(
                        macroquad::miniquad::Equation::Add,
                        macroquad::miniquad::BlendFactor::Value(macroquad::miniquad::BlendValue::SourceAlpha),
                        macroquad::miniquad::BlendFactor::OneMinusValue(macroquad::miniquad::BlendValue::SourceAlpha),
                    )),
                    ..Default::default()
                };
                let make = |v: &str, f: &str, cull| {
                    load_material(
                        ShaderSource::Glsl { vertex: v, fragment: f },
                        MaterialParams { pipeline_params: depth(cull), ..Default::default() },
                    )
                };
                let toon = make(TOON_VERTEX, TOON_FRAGMENT, macroquad::miniquad::CullFace::Nothing);
                let outline = make(OUTLINE_VERTEX, OUTLINE_FRAGMENT, macroquad::miniquad::CullFace::Front);
                match (toon, outline) {
                    (Ok(toon), Ok(outline)) => Some(std::rc::Rc::new(Mats { toon, outline })),
                    (a, b) => {
                        eprintln!("character shaders unavailable: {:?} {:?}", a.err(), b.err());
                        None
                    }
                }
            })
            .clone()
    })
}

// ------------------------------------------------------------ Asset library

/// Characters and props by name, loaded once at start-up.
pub struct Library {
    pub models: HashMap<&'static str, Model>,
}

thread_local! {
    static LIB: RefCell<Option<std::rc::Rc<Library>>> = RefCell::new(None);
}

const FILES: &[(&str, &str)] = &[
    ("Barbarian", "adventurers/Barbarian.glb"),
    ("Knight", "adventurers/Knight.glb"),
    ("Mage", "adventurers/Mage.glb"),
    ("Rogue", "adventurers/Rogue.glb"),
    ("Rogue_Hooded", "adventurers/Rogue_Hooded.glb"),
    ("Skeleton_Minion", "skeletons/Skeleton_Minion.glb"),
    ("Skeleton_Warrior", "skeletons/Skeleton_Warrior.glb"),
    ("axe", "adventurers/axe_1handed.gltf"),
    ("quiver", "adventurers/quiver.gltf"),
    ("shield", "adventurers/shield_round.gltf"),
    ("crossbow", "adventurers/crossbow_1handed.gltf"),
    ("spellbook", "adventurers/spellbook_open.gltf"),
    ("arrow", "adventurers/arrow.gltf"),
    ("dagger", "adventurers/dagger.gltf"),
    ("staff", "adventurers/staff.gltf"),
    ("greatsword", "adventurers/sword_2handed.gltf"),
    ("tome", "adventurers/spellbook_closed.gltf"),
    ("greataxe", "adventurers/axe_2handed.gltf"),
    ("wand", "adventurers/wand.gltf"),
    ("sword", "adventurers/sword_1handed.gltf"),
    ("crossbow2h", "adventurers/crossbow_2handed.gltf"),
    ("sk_blade", "skeletons/Skeleton_Blade.gltf"),
    ("sk_axe", "skeletons/Skeleton_Axe.gltf"),
    ("sk_shield", "skeletons/Skeleton_Shield_Small_A.gltf"),
    ("castle_blue", "medieval/building_castle_blue.gltf"),
    ("castle_red", "medieval/building_castle_red.gltf"),
    ("tower_blue", "medieval/building_tower_A_blue.gltf"),
    ("tower_red", "medieval/building_tower_A_red.gltf"),
    ("towerbase_blue", "medieval/building_tower_base_blue.gltf"),
    ("towerbase_red", "medieval/building_tower_base_red.gltf"),
    ("rubble", "medieval/building_destroyed.gltf"),
    ("ball", "medieval/projectile_catapult.gltf"),
    ("trees_a", "medieval/trees_A_large.gltf"),
    ("trees_b", "medieval/trees_B_medium.gltf"),
    ("tree_a", "medieval/tree_single_A.gltf"),
    ("tree_b", "medieval/tree_single_B.gltf"),
    ("rock_a", "medieval/rock_single_A.gltf"),
    ("rock_c", "medieval/rock_single_C.gltf"),
    ("hills", "medieval/hills_A_trees.gltf"),
];

/// Where the `assets` folder is: $BAG_ASSETS, next to the executable, or the
/// workspace when run through cargo.
pub fn asset_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("BAG_ASSETS") {
        return PathBuf::from(d);
    }
    if let Ok(exe) = std::env::current_exe() {
        for up in exe.ancestors().skip(1).take(4) {
            let d = up.join("assets");
            if d.join("adventurers").is_dir() {
                return d;
            }
        }
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets")
}

/// Load every model. Missing files are reported and skipped, so the game
/// still runs (with the 2D fallback art) without the assets folder.
pub fn load_library() {
    let dir = asset_dir();
    let mut models = HashMap::new();
    for (name, file) in FILES {
        match Model::load(&dir.join(file)) {
            Ok(m) => {
                models.insert(*name, m);
            }
            Err(e) => eprintln!("asset not loaded: {e}"),
        }
    }
    LIB.with(|l| *l.borrow_mut() = Some(std::rc::Rc::new(Library { models })));
}

pub fn library() -> Option<std::rc::Rc<Library>> {
    LIB.with(|l| l.borrow().clone())
}

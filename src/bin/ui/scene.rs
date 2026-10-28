//! 3D mode: polygon meshes with a z-buffered, Blinn-Phong-shaded software
//! viewport (`mesh3d::Frame3d`), Maya-style tools (move / rotate / scale,
//! extrude, bevel, edge loop, multi-cut, bridge, booleans, quad draw),
//! joints, lights, cameras and a keyed Y channel. Every edit goes through
//! `SceneState::apply(AppCommand)`.

use crate::commands::AppCommand;
use crate::theme::{self, Palette, R_SM, SP_MD, SP_SM, SP_XS};
use crate::timeline::{self, Clock, Track};
use crate::{ToastKind, Toasts};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Stroke, Vec2};
use obs_rust_bitstream::arena::{Arena, Environment};
use obs_rust_bitstream::material_assign::MaterialAssign;
use obs_rust_bitstream::mesh3d::{self as m3, BoolOp, Camera, Frame3d, PolyMesh, Surface, V3};
use obs_rust_bitstream::ui_canvas::{KeyTrack, Raster};
use obs_rust_bitstream::{Joint, JointHierarchy};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Kind {
    Cube,
    Mesh,
    Joint,
    Light,
    Camera,
}

impl Kind {
    fn icon(&self) -> &'static str {
        match self {
            Kind::Cube => "⬛",
            Kind::Mesh => "◆",
            Kind::Joint => "◉",
            Kind::Light => "☀",
            Kind::Camera => "🎥",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Gizmo {
    Select,
    Move,
    Rotate,
    Scale,
    EdgeLoop,
    MultiCut,
    QuadDraw,
}

impl Gizmo {
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().replace([' ', '-'], "_").as_str() {
            "select" => Some(Gizmo::Select),
            "move" | "translate" => Some(Gizmo::Move),
            "rotate" => Some(Gizmo::Rotate),
            "scale" => Some(Gizmo::Scale),
            "edge_loop" | "edgeloop" => Some(Gizmo::EdgeLoop),
            "multi_cut" | "multicut" => Some(Gizmo::MultiCut),
            "quad_draw" | "quaddraw" => Some(Gizmo::QuadDraw),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Gizmo::Select => "Select",
            Gizmo::Move => "Move",
            Gizmo::Rotate => "Rotate",
            Gizmo::Scale => "Scale",
            Gizmo::EdgeLoop => "Edge Loop",
            Gizmo::MultiCut => "Multi-Cut",
            Gizmo::QuadDraw => "Quad Draw",
        }
    }

    fn hint(&self) -> &'static str {
        match self {
            Gizmo::Select => "Click a mesh to select it. Drag (or Alt-drag) orbits, scroll dollies.",
            Gizmo::Move => "Drag to move on the ground plane; Shift-drag moves Y.",
            Gizmo::Rotate => "Drag left/right to rotate around Y.",
            Gizmo::Scale => "Drag left/right to scale uniformly.",
            Gizmo::EdgeLoop => "Click near an edge of a quad: a loop is inserted through that ring at the click position.",
            Gizmo::MultiCut => "Click near one edge of a face, then near another edge of the same face to cut it.",
            Gizmo::QuadDraw => "Click four points on the ground grid to place a quad. Corners weld to earlier quads.",
        }
    }
}

/// The Display card's modes and their keyboard shortcuts.
pub const DISPLAY_MODES: [(Shading, &str); 3] = [
    (Shading::Shaded, "5"),
    (Shading::Wireframe, "4"),
    (Shading::ShadedWire, "6"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Shading {
    Shaded,
    Wireframe,
    ShadedWire,
}

impl Shading {
    pub fn parse(s: &str) -> Option<Self> {
        match s
            .trim()
            .to_lowercase()
            .replace([' ', '-', '+'], "_")
            .as_str()
        {
            "shaded" | "solid" => Some(Shading::Shaded),
            "wireframe" | "wire" => Some(Shading::Wireframe),
            "shaded_wire" | "shaded_wireframe" | "wireframe_on_shaded" | "shaded__wire" => {
                Some(Shading::ShadedWire)
            }
            _ => None,
        }
    }
    pub fn label(&self) -> &'static str {
        match self {
            Shading::Shaded => "Shaded",
            Shading::Wireframe => "Wireframe",
            Shading::ShadedWire => "Shaded + wire",
        }
    }
    fn key(&self) -> &'static str {
        match self {
            Shading::Shaded => "shaded",
            Shading::Wireframe => "wireframe",
            Shading::ShadedWire => "shaded_wire",
        }
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Obj {
    pub name: String,
    pub kind: Kind,
    pub pos: [f32; 3],
    pub rot_y: f32,
    pub scale: f32,
    pub visible: bool,
    pub parent: Option<usize>,
    pub track_y: KeyTrack,
    pub mesh: Option<PolyMesh>,
    /// Non-uniform local scale (a melted body slumps into a puddle).
    pub squash: [f32; 3],
}

impl Obj {
    fn new(name: &str, kind: Kind, pos: [f32; 3]) -> Self {
        let mesh = (kind == Kind::Cube).then(PolyMesh::cube);
        Self {
            name: name.to_string(),
            kind,
            pos,
            rot_y: 0.0,
            scale: 1.0,
            visible: true,
            parent: None,
            track_y: KeyTrack::new(),
            mesh,
            squash: [1.0; 3],
        }
    }

    pub fn to_world(&self, v: V3) -> V3 {
        let v = [
            v[0] * self.squash[0],
            v[1] * self.squash[1],
            v[2] * self.squash[2],
        ];
        let (s, co) = self.rot_y.to_radians().sin_cos();
        let k = self.scale;
        [
            self.pos[0] + (v[0] * co - v[2] * s) * k,
            self.pos[1] + v[1] * k,
            self.pos[2] + (v[0] * s + v[2] * co) * k,
        ]
    }

    pub fn to_local(&self, w: V3) -> V3 {
        let (s, co) = self.rot_y.to_radians().sin_cos();
        let k = self.scale.max(1e-6);
        let d = [
            (w[0] - self.pos[0]) / k,
            (w[1] - self.pos[1]) / k,
            (w[2] - self.pos[2]) / k,
        ];
        let l = [d[0] * co + d[2] * s, d[1], -d[0] * s + d[2] * co];
        [
            l[0] / self.squash[0].max(1e-6),
            l[1] / self.squash[1].max(1e-6),
            l[2] / self.squash[2].max(1e-6),
        ]
    }

    pub fn world_mesh(&self) -> Option<PolyMesh> {
        self.mesh
            .as_ref()
            .map(|m| m.transformed(|v| self.to_world(v)))
    }
}

/// Physics settings an object keeps between simulation runs.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PhysProps {
    /// Starting temperature (K); `None` = ambient.
    pub temp: Option<f32>,
    /// Heater power (W).
    pub heater: f32,
    /// Pinned in place (infinite mass).
    pub fixed: bool,
    /// Initial velocity (m/s).
    pub vel: [f32; 3],
}

impl Default for PhysProps {
    fn default() -> Self {
        Self {
            temp: None,
            heater: 0.0,
            fixed: false,
            vel: [0.0; 3],
        }
    }
}

/// A click on a face edge: object, face, edge index within the face, t.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgePick {
    pub obj: usize,
    pub face: usize,
    pub edge: usize,
    pub t: f32,
}

pub struct SceneState {
    pub objs: Vec<Obj>,
    pub selected: Option<usize>,
    /// The selection before the current one (second operand of Boolean / Bridge).
    pub prev_selected: Option<usize>,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub grid: bool,
    pub clock: Clock,
    pub joints: JointHierarchy,
    pub gizmo: Gizmo,
    pub shading: Shading,
    pub shelf_tab: usize,
    pub cut_start: Option<EdgePick>,
    pub quad_pts: Vec<V3>,
    /// Last rendered viewport (for recording / streaming).
    pub frame: Option<Arc<Raster>>,
    /// Object name → material name (the library's `MaterialAssign`).
    pub assign: MaterialAssign,
    /// Material name → shading surface, kept in sync with the library by the app.
    pub surfaces: HashMap<String, Surface>,
    /// Physics arena (built from the scene on Play/Step; `None` when reset).
    pub arena: Option<Arena>,
    pub arena_env: Environment,
    pub arena_playing: bool,
    /// (steps done, steps asked) of a background arena run.
    pub arena_progress: Option<(u64, u64)>,
    /// Per-object physics settings.
    pub phys: HashMap<String, PhysProps>,
    /// Scene state before the simulation started (Reset restores it).
    pub arena_start: Option<Vec<Obj>>,
    ids: Vec<u32>,
    ids_w: usize,
    ids_h: usize,
    ppp: f32,
    tex: Option<egui::TextureHandle>,
    counter: usize,
    drag_rot: Option<(f32, f32)>,
}

impl SceneState {
    pub fn new() -> Self {
        let mut objs = vec![
            Obj::new("cube1", Kind::Cube, [0.0, 0.5, 0.0]),
            Obj::new("hip", Kind::Joint, [2.2, 1.0, 0.0]),
            Obj::new("knee", Kind::Joint, [2.4, 0.5, 0.3]),
            Obj::new("key_light", Kind::Light, [-2.5, 3.0, 2.0]),
            Obj::new("camera1", Kind::Camera, [0.0, 1.5, 5.0]),
        ];
        objs[2].parent = Some(1);
        objs[0].track_y.set(0.0, 0.5);
        objs[0].track_y.set(1.5, 1.6);
        objs[0].track_y.set(3.0, 0.5);
        let mut joints = JointHierarchy::new();
        for o in objs.iter().filter(|o| o.kind == Kind::Joint) {
            if let Ok(j) = Joint::new(&o.name) {
                joints.add(j);
            }
        }
        let _ = joints.set_parent("knee", "hip");
        let mut s = Self {
            objs,
            selected: Some(0),
            prev_selected: None,
            yaw: 0.6,
            pitch: 0.45,
            dist: 11.0,
            grid: true,
            clock: Clock::new(3.0),
            joints,
            gizmo: Gizmo::Select,
            shading: Shading::Shaded,
            shelf_tab: 0,
            cut_start: None,
            quad_pts: Vec::new(),
            frame: None,
            assign: MaterialAssign::new(),
            surfaces: HashMap::new(),
            arena: None,
            arena_env: Environment::earth(),
            arena_playing: false,
            arena_progress: None,
            phys: HashMap::new(),
            arena_start: None,
            ids: Vec::new(),
            ids_w: 0,
            ids_h: 0,
            ppp: 1.0,
            tex: None,
            counter: 1,
            drag_rot: None,
        };
        s.sync_joints();
        s
    }

    pub fn camera(&self) -> Camera {
        Camera {
            yaw: self.yaw,
            pitch: self.pitch,
            dist: self.dist,
        }
    }

    pub fn tick(&mut self, dt: f32) {
        self.clock.tick(dt);
        let t = self.clock.time;
        // while a physics simulation exists it owns the transforms; keyframes
        // resume once the arena is reset
        let animate = self.arena.is_none();
        for o in self.objs.iter_mut().filter(|_| animate) {
            if let Some(y) = o.track_y.sample(t) {
                o.pos[1] = y;
            }
        }
        self.sync_joints();
    }

    /// Joint positions follow their scene objects. Done on creation and
    /// load too, so the first frame does not look like an edit.
    fn sync_joints(&mut self) {
        for o in self.objs.iter().filter(|o| o.kind == Kind::Joint) {
            if let Ok(j) = self.joints.get_mut(&o.name) {
                j.set_position(o.pos[0], o.pos[1], o.pos[2]);
            }
        }
    }

    fn select(&mut self, i: Option<usize>) {
        if i != self.selected {
            if self.selected.is_some() {
                self.prev_selected = self.selected;
            }
            self.selected = i;
        }
    }

    pub fn add(&mut self, kind: &str, toasts: &mut Toasts) -> Result<String, String> {
        let (k, base, mesh) = match kind.to_lowercase().as_str() {
            "cube" => (Kind::Cube, "cube", Some(PolyMesh::cube())),
            "plane" => (Kind::Mesh, "plane", Some(PolyMesh::plane())),
            "cylinder" => (Kind::Mesh, "cylinder", Some(PolyMesh::cylinder(16))),
            "joint" => (Kind::Joint, "joint", None),
            "light" => (Kind::Light, "light", None),
            "camera" => (Kind::Camera, "camera", None),
            other => {
                return Err(format!(
                    "unknown kind '{}' (cube, plane, cylinder, joint, light, camera)",
                    other
                ))
            }
        };
        self.counter += 1;
        let name = format!("{}{}", base, self.counter);
        if k == Kind::Joint {
            if let Ok(j) = Joint::new(&name) {
                self.joints.add(j);
            }
        }
        let y = if kind == "plane" { 0.0 } else { 0.5 };
        let mut o = Obj::new(&name, k, [0.0, y, 0.0]);
        o.mesh = mesh;
        self.objs.push(o);
        self.select(Some(self.objs.len() - 1));
        toasts.push(ToastKind::Success, &format!("Added {}", name));
        Ok(format!("added {}", name))
    }

    fn remove_obj(&mut self, i: usize) {
        if i >= self.objs.len() {
            return;
        }
        self.objs.remove(i);
        let fix = |x: Option<usize>| match x {
            Some(p) if p == i => None,
            Some(p) if p > i => Some(p - 1),
            other => other,
        };
        for o in &mut self.objs {
            o.parent = fix(o.parent);
        }
        self.prev_selected = fix(self.prev_selected);
        self.selected = fix(self.selected);
    }

    pub fn delete_selected(&mut self) {
        let Some(i) = self.selected else { return };
        if i >= self.objs.len() {
            return;
        }
        self.remove_obj(i);
        self.selected = if self.objs.is_empty() {
            None
        } else {
            Some(i.min(self.objs.len() - 1))
        };
    }

    pub fn find(&self, name: Option<&str>) -> Result<usize, String> {
        match name.map(|n| n.trim()).filter(|n| !n.is_empty()) {
            None => self
                .selected
                .filter(|i| *i < self.objs.len())
                .ok_or_else(|| "no object selected".to_string()),
            Some(n) => self
                .objs
                .iter()
                .position(|o| o.name.eq_ignore_ascii_case(n))
                .ok_or_else(|| {
                    format!(
                        "no object named '{}' (objects: {})",
                        n,
                        self.objs
                            .iter()
                            .map(|o| o.name.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                }),
        }
    }

    fn find_mesh(&self, name: Option<&str>) -> Result<usize, String> {
        let i = self.find(name)?;
        if self.objs[i].mesh.is_none() {
            return Err(format!("{} isn't a mesh", self.objs[i].name));
        }
        Ok(i)
    }

    /// Second operand: `with`, else the previous selection, else the only other mesh.
    fn other_mesh(&self, a: usize, with: Option<&str>) -> Result<usize, String> {
        if let Some(w) = with.filter(|w| !w.trim().is_empty()) {
            let b = self.find_mesh(Some(w))?;
            return if b == a {
                Err("pick two different meshes".into())
            } else {
                Ok(b)
            };
        }
        if let Some(b) = self
            .prev_selected
            .filter(|&b| b != a && b < self.objs.len() && self.objs[b].mesh.is_some())
        {
            return Ok(b);
        }
        let others: Vec<usize> = (0..self.objs.len())
            .filter(|&i| i != a && self.objs[i].mesh.is_some())
            .collect();
        match others.as_slice() {
            [b] => Ok(*b),
            [] => Err("needs a second mesh: add another cube first".into()),
            _ => Err("select the second mesh first, then the target (or pass `with`)".into()),
        }
    }

    fn mesh_mut(&mut self, i: usize) -> &mut PolyMesh {
        self.objs[i].mesh.get_or_insert_with(PolyMesh::default)
    }

    /// Execute a 3D command. `None` = not a 3D command.
    pub fn apply(
        &mut self,
        cmd: &AppCommand,
        toasts: &mut Toasts,
    ) -> Option<Result<String, String>> {
        use AppCommand as C;
        Some(match cmd {
            C::SceneAdd { kind } => self.add(kind, toasts),
            C::SceneSelect { object } => self.find(Some(object)).map(|i| {
                self.select(Some(i));
                format!("selected {}", self.objs[i].name)
            }),
            C::SceneGizmo { mode } => match Gizmo::from_str(mode) {
                Some(g) => {
                    self.gizmo = g;
                    self.cut_start = None;
                    if g != Gizmo::QuadDraw {
                        self.quad_pts.clear();
                    }
                    Ok(format!("{} tool", g.label()))
                }
                None => Err(format!("unknown tool '{}'", mode)),
            },
            C::SceneShading { mode } => match Shading::parse(mode) {
                Some(s) => {
                    self.shading = s;
                    Ok(format!("viewport: {}", s.label()))
                }
                None => Err(format!(
                    "unknown shading '{}' (shaded, wireframe, shaded_wire)",
                    mode
                )),
            },
            C::SceneTransform {
                object,
                translate,
                rotate_y,
                scale,
            } => self.find(object.as_deref()).map(|i| {
                let t = self.clock.time;
                let o = &mut self.objs[i];
                let mut parts = Vec::new();
                if let Some(d) = translate {
                    for (p, dk) in o.pos.iter_mut().zip(d) {
                        *p += dk;
                    }
                    if d[1] != 0.0 && !o.track_y.is_empty() {
                        o.track_y.set(t, o.pos[1]);
                    }
                    parts.push(format!("moved {:.2}, {:.2}, {:.2}", d[0], d[1], d[2]));
                }
                if let Some(r) = rotate_y {
                    o.rot_y = r % 360.0;
                    parts.push(format!("rotated Y {:.0}°", o.rot_y));
                }
                if let Some(s) = scale {
                    o.scale = s.clamp(0.05, 20.0);
                    parts.push(format!("scaled {:.2}", o.scale));
                }
                format!("{} {}", o.name, parts.join(", "))
            }),
            C::SceneExtrude { distance } => self.find_mesh(None).and_then(|i| {
                let d = distance.unwrap_or(0.5).clamp(-5.0, 5.0);
                let m = self.mesh_mut(i);
                let f = m.top_face().ok_or("mesh has no faces")?;
                m.extrude(f, d)?;
                Ok(format!(
                    "extruded {} top face by {:.2}",
                    self.objs[i].name, d
                ))
            }),
            C::SceneBevel { amount } => self.find_mesh(None).and_then(|i| {
                let a = amount.unwrap_or(0.15).clamp(0.02, 0.45);
                let m = self.mesh_mut(i);
                let f = m.top_face().ok_or("mesh has no faces")?;
                m.bevel(f, a, a * 0.5)?;
                Ok(format!(
                    "bevelled {} top face ({:.2})",
                    self.objs[i].name, a
                ))
            }),
            C::SceneEdgeLoop { object, edge, t } => {
                self.find_mesh(object.as_deref()).and_then(|i| {
                    let m = self.mesh_mut(i);
                    let (a, b) = match edge {
                        Some([a, b]) => (*a, *b),
                        None => m
                            .default_loop_edge()
                            .ok_or("no quad faces to loop through")?,
                    };
                    let n = m.edge_loop(a, b, t.unwrap_or(0.5))?;
                    let faces = m.faces.len();
                    Ok(format!(
                        "inserted an edge loop on {} ({} quads split, {} faces)",
                        self.objs[i].name, n, faces
                    ))
                })
            }
            C::SceneMultiCut {
                object,
                face,
                from_edge,
                to_edge,
                from_t,
                to_t,
            } => self.find_mesh(object.as_deref()).and_then(|i| {
                let m = self.mesh_mut(i);
                let f = match face {
                    Some(f) => *f,
                    None => m.top_face().ok_or("mesh has no faces")?,
                };
                let n = m
                    .faces
                    .get(f)
                    .map(|x| x.len())
                    .ok_or_else(|| format!("face {} doesn't exist", f))?;
                let a = from_edge.unwrap_or(0);
                let b = to_edge.unwrap_or(n / 2);
                m.multi_cut(f, a, from_t.unwrap_or(0.5), b, to_t.unwrap_or(0.5))?;
                let faces = m.faces.len();
                Ok(format!(
                    "cut face {} of {} ({} faces)",
                    f, self.objs[i].name, faces
                ))
            }),
            C::SceneBridge { object, with } => self.find_mesh(object.as_deref()).and_then(|a| {
                let b = self.other_mesh(a, with.as_deref())?;
                let other = self.objs[b].world_mesh().unwrap_or_default();
                let oa = self.objs[a].clone();
                let local = other.transformed(|w| oa.to_local(w));
                let mut merged = self.objs[a].mesh.clone().unwrap_or_default();
                let split = merged.merge(&local);
                let (fa, fb) = merged
                    .facing_pair(split)
                    .ok_or("no pair of facing faces with the same corner count")?;
                merged.bridge(fa, fb)?;
                let (na, nb) = (self.objs[a].name.clone(), self.objs[b].name.clone());
                self.objs[a].mesh = Some(merged);
                self.objs[a].kind = Kind::Mesh;
                self.remove_obj(b);
                let a = if b < a { a - 1 } else { a };
                self.selected = Some(a);
                self.prev_selected = None;
                Ok(format!("bridged {} to {} (merged into {})", na, nb, na))
            }),
            C::SceneBoolean { op, object, with } => {
                let Some(op) = BoolOp::parse(op) else {
                    return Some(Err(format!(
                        "unknown boolean '{}' (union, difference, intersection)",
                        op
                    )));
                };
                self.find_mesh(object.as_deref()).and_then(|a| {
                    let b = self.other_mesh(a, with.as_deref())?;
                    let oa = self.objs[a].clone();
                    let mb = self.objs[b]
                        .world_mesh()
                        .unwrap_or_default()
                        .transformed(|w| oa.to_local(w));
                    let ma = self.objs[a].mesh.clone().unwrap_or_default();
                    let r = m3::boolean(&ma, &mb, op);
                    if r.faces.is_empty() {
                        return Err(format!(
                            "the {} is empty (do the meshes overlap?)",
                            op.label()
                        ));
                    }
                    let (na, nb) = (self.objs[a].name.clone(), self.objs[b].name.clone());
                    let faces = r.faces.len();
                    self.objs[a].mesh = Some(r);
                    self.objs[a].kind = Kind::Mesh;
                    self.remove_obj(b);
                    let a = if b < a { a - 1 } else { a };
                    self.selected = Some(a);
                    self.prev_selected = None;
                    Ok(format!(
                        "{} {} {} → {} ({} faces)",
                        na,
                        op.label(),
                        nb,
                        na,
                        faces
                    ))
                })
            }
            C::SceneQuadDraw { points } => {
                if points.len() != 4 {
                    return Some(Err(format!(
                        "quad draw needs 4 points, got {}",
                        points.len()
                    )));
                }
                let i = match self.objs.iter().position(|o| o.name == "quadDraw1") {
                    Some(i) => i,
                    None => {
                        let mut o = Obj::new("quadDraw1", Kind::Mesh, [0.0, 0.0, 0.0]);
                        o.mesh = Some(PolyMesh::default());
                        self.objs.push(o);
                        self.objs.len() - 1
                    }
                };
                let o = self.objs[i].clone();
                let pts = [
                    o.to_local(points[0]),
                    o.to_local(points[1]),
                    o.to_local(points[2]),
                    o.to_local(points[3]),
                ];
                let m = self.mesh_mut(i);
                m.quad_draw(&pts, 0.12 / o.scale.max(1e-3), None);
                let (v, f) = (m.verts.len(), m.faces.len());
                self.select(Some(i));
                Ok(format!(
                    "quad added to quadDraw1 ({} verts, {} faces)",
                    v, f
                ))
            }
            C::SetKeyframe => self.find(None).map(|i| {
                let t = self.clock.time;
                let o = &mut self.objs[i];
                o.track_y.set(t, o.pos[1]);
                format!("keyed {} Y={:.2} at {:.2}s", o.name, o.pos[1], t)
            }),
            C::DeleteKeyframe => self.find(None).and_then(|i| {
                let t = self.clock.time;
                let o = &mut self.objs[i];
                if o.track_y.remove_near(t, 0.05) {
                    Ok(format!("deleted {} key at {:.2}s", o.name, t))
                } else {
                    Err(format!("no key on {} near {:.2}s", o.name, t))
                }
            }),
            C::TimelinePlay { .. } | C::TimelineScrub { .. } => {
                return timeline::apply(&mut self.clock, cmd)
            }
            C::SceneDelete { object } => self.find(object.as_deref()).map(|i| {
                self.selected = Some(i);
                let n = self.objs[i].name.clone();
                self.delete_selected();
                format!("deleted {}", n)
            }),
            _ => return None,
        })
    }

    /// Name of the mesh object a material command targets (default: the selection).
    pub fn check_mesh_target(&self, object: Option<&str>) -> Result<String, String> {
        let i = match object {
            Some(n) => self
                .objs
                .iter()
                .position(|o| o.name.eq_ignore_ascii_case(n.trim()))
                .ok_or_else(|| format!("no object named '{}'", n))?,
            None => self
                .selected
                .filter(|i| *i < self.objs.len())
                .ok_or("select a mesh first")?,
        };
        let o = &self.objs[i];
        if o.mesh.is_none() {
            return Err(format!(
                "{} is a {:?}, not a mesh; materials go on meshes",
                o.name, o.kind
            ));
        }
        Ok(o.name.clone())
    }

    pub fn assign_material(&mut self, object: &str, material: &str) -> Result<String, String> {
        self.assign
            .assign(object, material)
            .map_err(|e| format!("assign: {:?}", e))?;
        Ok(format!("{} is now {}", object, material))
    }

    /// Shading surface of an object (its material, or the default grey).
    pub fn surface_of(&self, name: &str) -> Surface {
        self.assign
            .material_of(name)
            .ok()
            .and_then(|m| self.surfaces.get(m))
            .copied()
            .unwrap_or_else(Surface::grey)
    }

    pub fn context(&self) -> String {
        let objs: Vec<String> = self
            .objs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let mark = if self.selected == Some(i) {
                    "*"
                } else if self.prev_selected == Some(i) {
                    "+"
                } else {
                    ""
                };
                let mat = self
                    .assign
                    .material_of(&o.name)
                    .map(|m| format!(", material {}", m))
                    .unwrap_or_default();
                let mesh = o
                    .mesh
                    .as_ref()
                    .map(|m| {
                        format!(
                            ", {} verts {} faces{}{}",
                            m.verts.len(),
                            m.faces.len(),
                            if m.is_closed() { " closed" } else { "" },
                            mat
                        )
                    })
                    .unwrap_or_default();
                format!(
                    "{}{} ({:?} at {:.1},{:.1},{:.1} rotY {:.0} scale {:.2}{})",
                    mark, o.name, o.kind, o.pos[0], o.pos[1], o.pos[2], o.rot_y, o.scale, mesh
                )
            })
            .collect();
        let arena = match &self.arena {
            Some(a) => {
                let bodies: Vec<String> = a
                    .bodies
                    .iter()
                    .map(|b| {
                        format!(
                            "{} [{}] {:.0} kg {:.1} K {}{} v=({:.2},{:.2},{:.2})",
                            b.name,
                            b.material,
                            b.mass,
                            b.temp,
                            b.phase.label(),
                            if b.vaporised { " vaporised" } else { "" },
                            b.vel[0],
                            b.vel[1],
                            b.vel[2]
                        )
                    })
                    .collect();
                let e = a.energy();
                let l = &a.ledger;
                format!("running {} t={:.2}s: {}. Energy: KE {:.1} J, PE {:.1} J, internal {:.1} J; heaters in {:.1} J, air/radiation {:+.1} J, drag {:+.1} J, impacts→heat {:.1} J, conducted {:.1} J, residual {:+.3} J", if self.arena_playing { "playing" } else { "paused" }, a.time, bodies.join("; "), e.kinetic, e.potential, e.internal, l.heater_in, l.exchange_in, l.drag_work, l.dissipated, l.conducted, a.energy_residual())
            }
            None => "not running".into(),
        };
        let e = &self.arena_env;
        format!("3D: tool {}, viewport {}, objects (* selected, + previous selection = second operand for boolean/bridge): {}\nPhysics arena: environment {} (g {:.3} m/s², air {:.3} kg/m³, {:.0} Pa, {:.1} K, wind {:?}); {}", self.gizmo.label(), self.shading.label(), objs.join("; "), e.name, e.gravity, e.air_density, e.pressure, e.temperature, e.wind, arena)
    }

    fn light_pos(&self) -> V3 {
        self.objs
            .iter()
            .find(|o| o.kind == Kind::Light && o.visible)
            .map(|o| o.pos)
            .unwrap_or([-2.5, 3.0, 2.0])
    }

    /// Rasterise the scene (meshes, grid, bones, camera gizmos) at `w`×`h`.
    pub fn render(
        &self,
        w: usize,
        h: usize,
        bg: Color32,
        grid: Color32,
        accent: Color32,
    ) -> Frame3d {
        let cam = self.camera();
        let mut f = Frame3d::new(w.max(1), h.max(1), [bg.r(), bg.g(), bg.b()]);
        let gc = [grid.r(), grid.g(), grid.b(), 255];
        if self.grid {
            for i in -5..=5 {
                let v = i as f32;
                f.world_line(&cam, [v, 0.0, -5.0], [v, 0.0, 5.0], gc);
                f.world_line(&cam, [-5.0, 0.0, v], [5.0, 0.0, v], gc);
            }
            f.world_line(
                &cam,
                [0.0, 0.002, 0.0],
                [1.5, 0.002, 0.0],
                [230, 80, 80, 255],
            );
            f.world_line(&cam, [0.0, 0.0, 0.0], [0.0, 1.5, 0.0], [90, 200, 100, 255]);
            f.world_line(
                &cam,
                [0.0, 0.002, 0.0],
                [0.0, 0.002, 1.5],
                [90, 140, 240, 255],
            );
        }
        let light = self.light_pos();
        let acc = [accent.r(), accent.g(), accent.b()];
        // opaque objects first, transparent (glass, water) last so they blend over them
        let mut order: Vec<usize> = (0..self.objs.len()).collect();
        order.sort_by_key(|&i| {
            (self.objs[i].mesh.is_some() && self.surface_of(&self.objs[i].name).transmission > 0.0)
                as u8
        });
        for i in order {
            let o = &self.objs[i];
            if !o.visible {
                continue;
            }
            let sel = self.selected == Some(i);
            let second = self.prev_selected == Some(i);
            let col = if sel {
                [214, 214, 220]
            } else {
                [212, 212, 216]
            };
            match (&o.kind, o.world_mesh()) {
                (_, Some(m)) => {
                    let surf = self.surface_of(&o.name);
                    let has_mat = self.assign.material_of(&o.name).is_ok();
                    // selection: tint toward the accent (lighter for materials so their colour still reads)
                    let tint = sel.then_some((acc, if has_mat { 0.18 } else { 0.4 }));
                    let wire = if sel {
                        Some([acc[0], acc[1], acc[2], 255])
                    } else if second {
                        Some([acc[0], acc[1], acc[2], 140])
                    } else if self.shading != Shading::Shaded {
                        Some([40, 42, 48, 200])
                    } else {
                        None
                    };
                    if self.shading == Shading::Wireframe {
                        let wc = if sel || second {
                            wire.unwrap_or([0, 0, 0, 255])
                        } else {
                            [col[0], col[1], col[2], 220]
                        };
                        for (u, v) in m.edges() {
                            f.world_line(&cam, m.verts[u], m.verts[v], wc);
                        }
                    } else {
                        f.mesh_surface(&cam, &m, &surf, light, ((i as u32) + 1) << 16, wire, tint);
                    }
                }
                (Kind::Joint, None) => {
                    if let Some(pp) = o.parent.and_then(|pi| self.objs.get(pi)) {
                        f.world_line(
                            &cam,
                            pp.pos,
                            o.pos,
                            if sel {
                                [acc[0], acc[1], acc[2], 255]
                            } else {
                                [col[0], col[1], col[2], 200]
                            },
                        );
                    }
                }
                (Kind::Camera, None) => {
                    let c = o.pos;
                    let tip = [c[0], c[1], c[2] - 0.8];
                    let wc = if sel {
                        [acc[0], acc[1], acc[2], 255]
                    } else {
                        [col[0], col[1], col[2], 220]
                    };
                    let corners = [(-0.3, -0.2), (0.3, -0.2), (0.3, 0.2), (-0.3, 0.2)];
                    for k in 0..4 {
                        let (dx, dy) = corners[k];
                        let (ex, ey) = corners[(k + 1) % 4];
                        f.world_line(&cam, c, [tip[0] + dx, tip[1] + dy, tip[2]], wc);
                        f.world_line(
                            &cam,
                            [tip[0] + dx, tip[1] + dy, tip[2]],
                            [tip[0] + ex, tip[1] + ey, tip[2]],
                            wc,
                        );
                    }
                }
                _ => {}
            }
        }
        f
    }

    /// (object, face) under a viewport position (logical points from the rect origin).
    fn pick(&self, local: Vec2) -> Option<(usize, usize)> {
        let (x, y) = (local.x * self.ppp, local.y * self.ppp);
        if self.ids_w == 0 || x < 0.0 || y < 0.0 || x >= self.ids_w as f32 || y >= self.ids_h as f32
        {
            return None;
        }
        let id = self.ids[y as usize * self.ids_w + x as usize];
        if id == 0 {
            return None;
        }
        Some(((id >> 16) as usize - 1, (id & 0xFFFF) as usize))
    }

    /// Nearest edge of a picked face to a screen point, with the t along it.
    fn edge_pick(&self, obj: usize, face: usize, at: Pos2, rect: egui::Rect) -> Option<EdgePick> {
        let m = self.objs.get(obj)?.world_mesh()?;
        let f = m.faces.get(face)?;
        let cam = self.camera();
        let mut best: Option<(f32, usize, f32)> = None;
        for k in 0..f.len() {
            let (a, b) = (m.verts[f[k]], m.verts[f[(k + 1) % f.len()]]);
            let (Some(pa), Some(pb)) = (
                cam.project(a, rect.width(), rect.height()),
                cam.project(b, rect.width(), rect.height()),
            ) else {
                continue;
            };
            let pa = Vec2::new(pa[0], pa[1]);
            let pb = Vec2::new(pb[0], pb[1]);
            let q = at - rect.min;
            let ab = pb - pa;
            let t = ((q - pa).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
            let d = (pa + ab * t - q).length();
            if best.map(|x| d < x.0).unwrap_or(true) {
                best = Some((d, k, t));
            }
        }
        best.map(|(_, edge, t)| EdgePick {
            obj,
            face,
            edge,
            t: t.clamp(0.05, 0.95),
        })
    }

    /// Screen point → ground plane (y = 0).
    fn ground_point(&self, at: Pos2, rect: egui::Rect) -> Option<V3> {
        let f = rect.height() * 1.1;
        let q = at - rect.center();
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let to_world = |d: f32| -> V3 {
            let (xv, yv, zv) = (q.x * d / f, -q.y * d / f, self.dist - d);
            let y = yv * cp + zv * sp;
            let z0 = -yv * sp + zv * cp;
            [xv * cy + z0 * sy, y, -xv * sy + z0 * cy]
        };
        let (p1, p2) = (to_world(1.0), to_world(2.0));
        let dy = p2[1] - p1[1];
        if dy.abs() < 1e-6 {
            return None;
        }
        let t = -p1[1] / dy;
        if t < -1.0 {
            return None;
        }
        let p = m3::lerp(p1, p2, t);
        Some([
            (p[0] * 20.0).round() / 20.0,
            0.0,
            (p[2] * 20.0).round() / 20.0,
        ])
    }

    // ---- panels -----------------------------------------------------------------

    pub fn tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        theme::card(ui, p, "View", |ui| {
            for (label, yaw, pitch) in [
                ("Perspective", 0.6, 0.45),
                ("Front", 0.0, 0.0),
                ("Side", std::f32::consts::FRAC_PI_2, 0.0),
                ("Top", 0.0, 1.5),
            ] {
                if ui
                    .add_sized(
                        [ui.available_width(), 28.0],
                        theme::toggle_button(
                            p,
                            label,
                            (self.yaw - yaw).abs() < 1e-3 && (self.pitch - pitch).abs() < 1e-3,
                        ),
                    )
                    .clicked()
                {
                    self.yaw = yaw;
                    self.pitch = pitch;
                }
            }
            ui.checkbox(&mut self.grid, "Show grid");
        });
        theme::card(ui, p, "Display", |ui| {
            for (s, key) in DISPLAY_MODES {
                if ui
                    .add_sized(
                        [ui.available_width(), 26.0],
                        theme::toggle_button(
                            p,
                            &format!("{}   ({})", s.label(), key),
                            self.shading == s,
                        ),
                    )
                    .clicked()
                {
                    bus.push(AppCommand::SceneShading {
                        mode: s.key().into(),
                    });
                }
            }
        });
        theme::card(ui, p, "Create", |ui| {
            for (label, kind) in [
                ("Cube", "cube"),
                ("Plane", "plane"),
                ("Cylinder", "cylinder"),
                ("Joint", "joint"),
                ("Light", "light"),
                ("Camera", "camera"),
            ] {
                if ui
                    .add_sized(
                        [ui.available_width(), 26.0],
                        theme::toggle_button(p, label, false),
                    )
                    .clicked()
                {
                    bus.push(AppCommand::SceneAdd { kind: kind.into() });
                }
            }
        });
        self.arena_tools(ui, p, bus);
        theme::card(ui, p, self.gizmo.label(), |ui| {
            ui.label(theme::muted(p, self.gizmo.hint()));
            match self.gizmo {
                Gizmo::EdgeLoop => {
                    if ui
                        .add(theme::toggle_button(p, "Insert at the default edge", false))
                        .clicked()
                    {
                        bus.push(AppCommand::SceneEdgeLoop {
                            object: None,
                            edge: None,
                            t: None,
                        });
                    }
                }
                Gizmo::MultiCut => {
                    if self.cut_start.is_some() {
                        ui.label(
                            egui::RichText::new(
                                "First point set: click a second edge of the same face.",
                            )
                            .color(p.accent),
                        );
                    }
                    if ui
                        .add(theme::toggle_button(p, "Cut the top face in half", false))
                        .clicked()
                    {
                        bus.push(AppCommand::SceneMultiCut {
                            object: None,
                            face: None,
                            from_edge: None,
                            to_edge: None,
                            from_t: None,
                            to_t: None,
                        });
                    }
                }
                Gizmo::QuadDraw => {
                    ui.label(theme::muted(
                        p,
                        &format!("{} of 4 points placed", self.quad_pts.len()),
                    ));
                    if !self.quad_pts.is_empty()
                        && ui
                            .add(theme::toggle_button(p, "Clear points", false))
                            .clicked()
                    {
                        self.quad_pts.clear();
                    }
                }
                _ => {}
            }
        });
    }

    pub fn center(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        let (resp, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = resp.rect;
        let alt = ui.input(|i| i.modifiers.alt);
        let shift = ui.input(|i| i.modifiers.shift);
        let click_tool = matches!(
            self.gizmo,
            Gizmo::EdgeLoop | Gizmo::MultiCut | Gizmo::QuadDraw
        );
        let orbit = resp.dragged_by(egui::PointerButton::Middle)
            || (resp.dragged_by(egui::PointerButton::Primary)
                && (alt || self.gizmo == Gizmo::Select || click_tool));
        if orbit {
            let d = resp.drag_delta();
            self.yaw += d.x * 0.008;
            self.pitch = (self.pitch + d.y * 0.008).clamp(-1.5, 1.5);
        } else if resp.dragged_by(egui::PointerButton::Primary) {
            if let Some(i) = self.selected.filter(|i| *i < self.objs.len()) {
                let d = resp.drag_delta();
                let k = self.dist / (rect.height() * 1.1);
                let (sy, cy) = self.yaw.sin_cos();
                let name = Some(self.objs[i].name.clone());
                if resp.drag_started() {
                    self.drag_rot = Some((self.objs[i].rot_y, self.objs[i].scale));
                }
                let (r0, s0) = self
                    .drag_rot
                    .unwrap_or((self.objs[i].rot_y, self.objs[i].scale));
                let total =
                    ui.input(
                        |inp| match (inp.pointer.press_origin(), inp.pointer.latest_pos()) {
                            (Some(a), Some(b)) => b - a,
                            _ => Vec2::ZERO,
                        },
                    );
                match self.gizmo {
                    Gizmo::Move if d != Vec2::ZERO => {
                        let t = if shift {
                            [0.0, -d.y * k, 0.0]
                        } else {
                            [(d.x * cy + d.y * sy) * k, 0.0, (-d.x * sy + d.y * cy) * k]
                        };
                        bus.push(AppCommand::SceneTransform {
                            object: name,
                            translate: Some(t),
                            rotate_y: None,
                            scale: None,
                        });
                    }
                    Gizmo::Rotate if d != Vec2::ZERO => bus.push(AppCommand::SceneTransform {
                        object: name,
                        translate: None,
                        rotate_y: Some((r0 + total.x * 0.6) % 360.0),
                        scale: None,
                    }),
                    Gizmo::Scale if d != Vec2::ZERO => bus.push(AppCommand::SceneTransform {
                        object: name,
                        translate: None,
                        rotate_y: None,
                        scale: Some((s0 * (1.0 + total.x * 0.01)).clamp(0.05, 20.0)),
                    }),
                    _ => {}
                }
            }
        }
        if resp.drag_stopped() {
            self.drag_rot = None;
        }
        if resp.hovered() {
            let s = ui.input(|i| i.raw_scroll_delta.y);
            if s != 0.0 {
                self.dist = (self.dist * (1.0 - s * 0.0015)).clamp(2.0, 60.0);
            }
            let c = match self.gizmo {
                Gizmo::Select => {
                    if resp.dragged() {
                        egui::CursorIcon::Grabbing
                    } else {
                        egui::CursorIcon::Default
                    }
                }
                Gizmo::Move => egui::CursorIcon::Move,
                Gizmo::Rotate => egui::CursorIcon::Alias,
                Gizmo::Scale => egui::CursorIcon::ResizeHorizontal,
                _ => egui::CursorIcon::Crosshair,
            };
            ui.ctx().set_cursor_icon(c);
        }

        // ---- render
        let ppp = ui.ctx().pixels_per_point();
        let (pw, ph) = (
            (rect.width() * ppp).round().max(1.0) as usize,
            (rect.height() * ppp).round().max(1.0) as usize,
        );
        let frame = self.render(pw, ph, p.canvas, p.border.gamma_multiply(0.9), p.accent);
        let img = egui::ColorImage::from_rgba_unmultiplied([pw, ph], &frame.rgba);
        match &mut self.tex {
            Some(t) => t.set(img, egui::TextureOptions::LINEAR),
            None => {
                self.tex = Some(ui.ctx().load_texture(
                    "viewport3d",
                    img,
                    egui::TextureOptions::LINEAR,
                ))
            }
        }
        if let Some(t) = &self.tex {
            painter.image(
                t.id(),
                rect,
                egui::Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        self.frame = Raster::from_rgba(pw as u32, ph as u32, frame.rgba).map(Arc::new);
        self.ids = frame.ids;
        self.ids_w = pw;
        self.ids_h = ph;
        self.ppp = ppp;

        // ---- overlays: icons and labels
        let cam = self.camera();
        let proj = |v: V3| {
            cam.project(v, rect.width(), rect.height())
                .map(|q| rect.min + Vec2::new(q[0], q[1]))
        };
        let mut best: Option<(usize, f32)> = None;
        let click = if resp.clicked() {
            resp.interact_pointer_pos()
        } else {
            None
        };
        for (i, o) in self.objs.iter().enumerate() {
            if !o.visible {
                continue;
            }
            let sel = self.selected == Some(i);
            let col = if sel {
                p.accent
            } else {
                p.text.gamma_multiply(0.8)
            };
            let Some(sp) = proj(o.pos) else { continue };
            match o.kind {
                Kind::Joint => {
                    painter.circle_stroke(
                        sp,
                        6.0,
                        Stroke::new(if sel { 2.0_f32 } else { 1.2 }, col),
                    );
                }
                Kind::Light => {
                    painter.circle_filled(sp, 5.0, Color32::from_rgb(255, 214, 10));
                    for k in 0..8 {
                        let a = k as f32 * std::f32::consts::FRAC_PI_4;
                        let d = Vec2::new(a.cos(), a.sin());
                        painter
                            .line_segment([sp + d * 8.0, sp + d * 13.0], Stroke::new(1.2_f32, col));
                    }
                }
                _ => {}
            }
            if let Some(cp) = click {
                let d = cp.distance(sp);
                if o.mesh.is_none() && d < 22.0 && best.map(|b| d < b.1).unwrap_or(true) {
                    best = Some((i, d));
                }
            }
            painter.text(
                sp + Vec2::new(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                &o.name,
                egui::FontId::proportional(11.0),
                if sel { p.accent } else { p.muted },
            );
        }
        // pending tool state
        if let Some(cs) = self.cut_start {
            if let Some(m) = self.objs.get(cs.obj).and_then(|o| o.world_mesh()) {
                if let Some(f) = m.faces.get(cs.face) {
                    let v = m3::lerp(
                        m.verts[f[cs.edge]],
                        m.verts[f[(cs.edge + 1) % f.len()]],
                        cs.t,
                    );
                    if let Some(sp) = proj(v) {
                        painter.circle_filled(sp, 4.5, p.accent);
                        if let Some(hp) = resp.hover_pos() {
                            painter.line_segment([sp, hp], Stroke::new(1.5_f32, p.accent));
                        }
                    }
                }
            }
        }
        for (k, q) in self.quad_pts.iter().enumerate() {
            if let Some(sp) = proj(*q) {
                painter.circle_filled(sp, 4.0, p.accent);
                if let Some(nq) = self.quad_pts.get(k + 1).and_then(|n| proj(*n)) {
                    painter.line_segment([sp, nq], Stroke::new(1.5_f32, p.accent));
                }
            }
        }

        // ---- clicks
        if let Some(cp) = click {
            let picked = self.pick(cp - rect.min);
            match self.gizmo {
                Gizmo::EdgeLoop => {
                    if let Some(ep) = picked.and_then(|(o, f)| self.edge_pick(o, f, cp, rect)) {
                        let o = &self.objs[ep.obj];
                        if let Some(m) = &o.mesh {
                            let f = &m.faces[ep.face];
                            let (a, b) = (f[ep.edge], f[(ep.edge + 1) % f.len()]);
                            bus.push(AppCommand::SceneSelect {
                                object: o.name.clone(),
                            });
                            bus.push(AppCommand::SceneEdgeLoop {
                                object: Some(o.name.clone()),
                                edge: Some([a, b]),
                                t: Some(ep.t),
                            });
                        }
                    }
                }
                Gizmo::MultiCut => match picked.and_then(|(o, f)| self.edge_pick(o, f, cp, rect)) {
                    Some(ep) => match self.cut_start {
                        Some(cs)
                            if cs.obj == ep.obj && cs.face == ep.face && cs.edge != ep.edge =>
                        {
                            let name = self.objs[ep.obj].name.clone();
                            bus.push(AppCommand::SceneSelect {
                                object: name.clone(),
                            });
                            bus.push(AppCommand::SceneMultiCut {
                                object: Some(name),
                                face: Some(ep.face),
                                from_edge: Some(cs.edge),
                                to_edge: Some(ep.edge),
                                from_t: Some(cs.t),
                                to_t: Some(ep.t),
                            });
                            self.cut_start = None;
                        }
                        _ => self.cut_start = Some(ep),
                    },
                    None => self.cut_start = None,
                },
                Gizmo::QuadDraw => {
                    if let Some(g) = self.ground_point(cp, rect) {
                        let snapped = self
                            .objs
                            .iter()
                            .find(|o| o.name == "quadDraw1")
                            .and_then(|o| o.world_mesh())
                            .and_then(|m| {
                                m.verts
                                    .iter()
                                    .copied()
                                    .find(|v| m3::length(m3::sub(*v, g)) < 0.15)
                            })
                            .unwrap_or(g);
                        self.quad_pts.push(snapped);
                        if self.quad_pts.len() == 4 {
                            bus.push(AppCommand::SceneQuadDraw {
                                points: std::mem::take(&mut self.quad_pts),
                            });
                        }
                    }
                }
                _ => {
                    let target = picked.map(|(o, _)| o).or(best.map(|b| b.0));
                    match target {
                        Some(i) => bus.push(AppCommand::SceneSelect {
                            object: self.objs[i].name.clone(),
                        }),
                        None => self.select(None),
                    }
                }
            }
        }
        painter.text(
            rect.min + Vec2::new(SP_MD, SP_MD),
            egui::Align2::LEFT_TOP,
            format!(
                "Perspective  ·  {} objects  ·  {} tool  ·  {}",
                self.objs.len(),
                self.gizmo.label(),
                self.shading.label()
            ),
            egui::FontId::proportional(12.0),
            p.muted,
        );
    }

    fn arena_tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        theme::card(ui, p, "Physics Arena", |ui| {
            let cur = self.arena_env.name.clone();
            egui::ComboBox::from_id_salt("arena_env")
                .width(ui.available_width() - 8.0)
                .selected_text(&cur)
                .show_ui(ui, |ui| {
                    for k in Environment::PRESETS {
                        let e = Environment::preset(k).unwrap_or_else(Environment::earth);
                        if ui.selectable_label(cur == e.name, &e.name).clicked() {
                            bus.push(AppCommand::ArenaEnv {
                                preset: Some(k.into()),
                                gravity: None,
                                air_density: None,
                                temperature_c: None,
                                pressure_kpa: None,
                                wind: None,
                            });
                        }
                    }
                });
            let e = self.arena_env.clone();
            let (mut g, mut rho, mut tc, mut wx) =
                (e.gravity, e.air_density, e.temperature - 273.15, e.wind[0]);
            egui::Grid::new("arena_env_grid")
                .num_columns(2)
                .spacing([SP_SM, 4.0])
                .show(ui, |ui| {
                    ui.label(theme::muted(p, "Gravity"));
                    ui.add(
                        egui::DragValue::new(&mut g)
                            .speed(0.05)
                            .range(0.0..=50.0)
                            .suffix(" m/s²")
                            .max_decimals(3),
                    );
                    ui.end_row();
                    ui.label(theme::muted(p, "Air density"));
                    ui.add(
                        egui::DragValue::new(&mut rho)
                            .speed(0.01)
                            .range(0.0..=50.0)
                            .suffix(" kg/m³")
                            .max_decimals(3),
                    );
                    ui.end_row();
                    ui.label(theme::muted(p, "Temperature"));
                    ui.add(
                        egui::DragValue::new(&mut tc)
                            .speed(1.0)
                            .range(-273.15..=5000.0)
                            .suffix(" °C")
                            .max_decimals(1),
                    );
                    ui.end_row();
                    ui.label(theme::muted(p, "Wind x"));
                    ui.add(
                        egui::DragValue::new(&mut wx)
                            .speed(0.1)
                            .range(-100.0..=100.0)
                            .suffix(" m/s")
                            .max_decimals(1),
                    );
                    ui.end_row();
                });
            if g != e.gravity
                || rho != e.air_density
                || tc != e.temperature - 273.15
                || wx != e.wind[0]
            {
                bus.push(AppCommand::ArenaEnv {
                    preset: None,
                    gravity: (g != e.gravity).then_some(g),
                    air_density: (rho != e.air_density).then_some(rho),
                    temperature_c: (tc != e.temperature - 273.15).then_some(tc),
                    pressure_kpa: None,
                    wind: (wx != e.wind[0]).then_some([wx, e.wind[1], e.wind[2]]),
                });
            }
            ui.label(theme::muted(
                p,
                &format!("Pressure {:.1} kPa", e.pressure / 1000.0),
            ));
            ui.horizontal(|ui| {
                let label = if self.arena_playing { "Pause" } else { "Play" };
                if ui.add(theme::primary_button(p, label)).clicked() {
                    bus.push(AppCommand::ArenaPlay {
                        playing: Some(!self.arena_playing),
                    });
                }
                if ui
                    .add(theme::toggle_button(p, "Step", false))
                    .on_hover_text("Advance 1/60 s (4 fixed steps of 1/240 s)")
                    .clicked()
                {
                    bus.push(AppCommand::ArenaStep { steps: Some(4) });
                }
                if ui.add(theme::toggle_button(p, "Reset", false)).clicked() {
                    bus.push(AppCommand::ArenaReset);
                }
            });
            if let Some((done, total)) = self.arena_progress {
                ui.add(
                    egui::ProgressBar::new(done as f32 / total.max(1) as f32)
                        .text(format!("stepping {} / {} in the background", done, total)),
                );
            }
            match &self.arena {
                Some(a) => {
                    ui.label(theme::muted(
                        p,
                        &format!(
                            "t = {:.2} s  ·  {} bodies  ·  KE {:.1} J",
                            a.time,
                            a.bodies.iter().filter(|b| !b.vaporised).count(),
                            a.total_kinetic()
                        ),
                    ));
                    energy_ledger(ui, p, a);
                    for (t, ev) in a.events.log.iter().rev().take(4) {
                        ui.label(
                            egui::RichText::new(format!("{:.2}s  {}", t, ev))
                                .size(10.5)
                                .color(p.warn),
                        );
                    }
                }
                None => {
                    ui.label(theme::muted(p, "Play drops every visible mesh under these conditions. Mass = material density × volume."));
                }
            }
        });
    }

    fn physics_card(&self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>, i: usize) {
        let o = &self.objs[i];
        let name = o.name.clone();
        let props = self.phys.get(&name).copied().unwrap_or_default();
        theme::card(ui, p, "Physics", |ui| {
            if let Some(b) = self.arena.as_ref().and_then(|a| a.body(&name)) {
                theme::kv(ui, p, "Material", b.material.clone());
                theme::kv(ui, p, "Mass", format!("{:.3} kg", b.mass));
                theme::kv(
                    ui,
                    p,
                    "Temperature",
                    format!("{:.1} K ({:.1} °C)", b.temp, b.temp - 273.15),
                );
                theme::kv(
                    ui,
                    p,
                    "Phase",
                    if b.vaporised {
                        "vaporised".to_string()
                    } else {
                        b.phase.label().to_string()
                    },
                );
                theme::kv(
                    ui,
                    p,
                    "Speed",
                    format!(
                        "{:.2} m/s",
                        (b.vel[0] * b.vel[0] + b.vel[1] * b.vel[1] + b.vel[2] * b.vel[2]).sqrt()
                    ),
                );
            } else {
                ui.label(theme::muted(
                    p,
                    "Not simulated yet. Press Play in Physics Arena.",
                ));
            }
            let mut tc = props
                .temp
                .map(|t| t - 273.15)
                .unwrap_or(self.arena_env.temperature - 273.15);
            let mut heat = props.heater;
            let mut fixed = props.fixed;
            egui::Grid::new("phys_grid")
                .num_columns(2)
                .spacing([SP_SM, 4.0])
                .show(ui, |ui| {
                    ui.label(theme::muted(p, "Start temp"));
                    ui.add(
                        egui::DragValue::new(&mut tc)
                            .speed(1.0)
                            .range(-273.15..=6000.0)
                            .suffix(" °C")
                            .max_decimals(1),
                    );
                    ui.end_row();
                    ui.label(theme::muted(p, "Heater"));
                    ui.add(
                        egui::DragValue::new(&mut heat)
                            .speed(10.0)
                            .range(-1e7..=1e8)
                            .suffix(" W")
                            .max_decimals(0),
                    );
                    ui.end_row();
                    ui.label(theme::muted(p, "Pinned"));
                    ui.checkbox(&mut fixed, "");
                    ui.end_row();
                });
            let t0 = props
                .temp
                .map(|t| t - 273.15)
                .unwrap_or(self.arena_env.temperature - 273.15);
            if tc != t0 || heat != props.heater || fixed != props.fixed {
                bus.push(AppCommand::ArenaBody {
                    object: Some(name.clone()),
                    temperature_c: (tc != t0).then_some(tc),
                    heater_w: (heat != props.heater).then_some(heat),
                    fixed: (fixed != props.fixed).then_some(fixed),
                    velocity: None,
                });
            }
        });
    }

    pub fn inspector(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        theme::card(ui, p, "Outliner", |ui| {
            let mut pick = None;
            let mut vis = None;
            for (i, o) in self.objs.iter().enumerate() {
                let on = self.selected == Some(i);
                let second = self.prev_selected == Some(i);
                egui::Frame::NONE
                    .fill(if on {
                        p.accent_soft()
                    } else {
                        Color32::TRANSPARENT
                    })
                    .corner_radius(CornerRadius::same((R_SM + 1.0) as u8))
                    .inner_margin(egui::Margin::symmetric(SP_XS as i8, 1))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            if o.parent.is_some() {
                                ui.add_space(SP_MD);
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(if o.visible { "👁" } else { "–" })
                                            .color(p.muted),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                vis = Some(i);
                            }
                            let label = format!(
                                "{}  {}{}",
                                o.kind.icon(),
                                o.name,
                                if second { "  ·  2nd" } else { "" }
                            );
                            if ui
                                .add(
                                    egui::Button::new(egui::RichText::new(label).color(if on {
                                        p.accent
                                    } else {
                                        p.text
                                    }))
                                    .frame(false),
                                )
                                .clicked()
                            {
                                pick = Some(i);
                            }
                        });
                    });
            }
            if let Some(i) = vis {
                self.objs[i].visible = !self.objs[i].visible;
            }
            if let Some(i) = pick {
                bus.push(AppCommand::SceneSelect {
                    object: self.objs[i].name.clone(),
                });
            }
            if self.objs.is_empty() {
                ui.label(theme::muted(
                    p,
                    "Empty scene. Create an object on the left.",
                ));
            }
        });
        let t = self.clock.time;
        let Some(i) = self.selected.filter(|i| *i < self.objs.len()) else {
            theme::card(ui, p, "Transform", |ui| {
                ui.label(theme::muted(p, "Select an object to edit it."));
            });
            return;
        };
        theme::card(ui, p, "Transform", |ui| {
            let o = &self.objs[i];
            let name = Some(o.name.clone());
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(&o.name).strong().color(p.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(theme::toggle_button(p, "Delete", false)).clicked() {
                        bus.push(AppCommand::SceneDelete { object: None });
                    }
                });
            });
            let (mut pos, mut rot, mut sc) = (o.pos, o.rot_y, o.scale);
            egui::Grid::new("xf")
                .num_columns(2)
                .spacing([SP_SM, 6.0])
                .show(ui, |ui| {
                    ui.label(theme::muted(p, "Position"));
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut pos[0])
                                .speed(0.05)
                                .prefix("x ")
                                .max_decimals(2),
                        );
                        ui.add(
                            egui::DragValue::new(&mut pos[1])
                                .speed(0.05)
                                .prefix("y ")
                                .max_decimals(2),
                        );
                        ui.add(
                            egui::DragValue::new(&mut pos[2])
                                .speed(0.05)
                                .prefix("z ")
                                .max_decimals(2),
                        );
                    });
                    ui.end_row();
                    ui.label(theme::muted(p, "Rotate Y"));
                    ui.add(egui::DragValue::new(&mut rot).speed(1.0).suffix("°"));
                    ui.end_row();
                    ui.label(theme::muted(p, "Scale"));
                    ui.add(egui::DragValue::new(&mut sc).speed(0.02).range(0.05..=20.0));
                    ui.end_row();
                });
            let d = [pos[0] - o.pos[0], pos[1] - o.pos[1], pos[2] - o.pos[2]];
            if d != [0.0; 3] {
                bus.push(AppCommand::SceneTransform {
                    object: name.clone(),
                    translate: Some(d),
                    rotate_y: None,
                    scale: None,
                });
            }
            if rot != o.rot_y {
                bus.push(AppCommand::SceneTransform {
                    object: name.clone(),
                    translate: None,
                    rotate_y: Some(rot),
                    scale: None,
                });
            }
            if sc != o.scale {
                bus.push(AppCommand::SceneTransform {
                    object: name,
                    translate: None,
                    rotate_y: None,
                    scale: Some(sc),
                });
            }
        });
        if let Some(m) = &self.objs[i].mesh {
            let (v, f, closed) = (m.verts.len(), m.faces.len(), m.is_closed());
            let other = self
                .prev_selected
                .filter(|&b| b != i && b < self.objs.len() && self.objs[b].mesh.is_some())
                .map(|b| self.objs[b].name.clone());
            theme::card(ui, p, "Mesh", |ui| {
                theme::kv(ui, p, "Vertices", format!("{}", v));
                theme::kv(ui, p, "Faces", format!("{}", f));
                theme::kv(
                    ui,
                    p,
                    "Closed",
                    if closed { "yes" } else { "no" }.to_string(),
                );
                ui.label(theme::muted(
                    p,
                    &match &other {
                        Some(n) => format!("Second operand: {}", n),
                        None => {
                            "Select another mesh, then this one, to Bridge or Boolean them.".into()
                        }
                    },
                ));
                ui.add_enabled_ui(other.is_some(), |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (label, op) in [
                            ("Union", "union"),
                            ("Difference", "difference"),
                            ("Intersect", "intersection"),
                        ] {
                            if ui.add(theme::toggle_button(p, label, false)).clicked() {
                                bus.push(AppCommand::SceneBoolean {
                                    op: op.into(),
                                    object: None,
                                    with: other.clone(),
                                });
                            }
                        }
                        if ui.add(theme::toggle_button(p, "Bridge", false)).clicked() {
                            bus.push(AppCommand::SceneBridge {
                                object: None,
                                with: other.clone(),
                            });
                        }
                    });
                });
            });
        }
        if self.objs[i].mesh.is_some() {
            let name = self.objs[i].name.clone();
            let cur = self.assign.material_of(&name).ok().map(|s| s.to_string());
            theme::card(ui, p, "Material", |ui| {
                let mut choice: Option<String> = None;
                egui::ComboBox::from_id_salt("mat_pick")
                    .width(ui.available_width() - 8.0)
                    .selected_text(cur.clone().unwrap_or_else(|| "Default grey".into()))
                    .show_ui(ui, |ui| {
                        let mut names: Vec<String> = self.surfaces.keys().cloned().collect();
                        for pr in obs_rust_bitstream::material_pbr::presets() {
                            if !names.iter().any(|n| n == pr.name) {
                                names.push(pr.key.to_string());
                            }
                        }
                        for n in names {
                            if ui
                                .selectable_label(cur.as_deref() == Some(n.as_str()), &n)
                                .clicked()
                            {
                                choice = Some(n);
                            }
                        }
                    });
                if let Some(m) = choice {
                    bus.push(AppCommand::MaterialAssign {
                        object: Some(name.clone()),
                        material: m,
                    });
                }
                if let Some(m) = &cur {
                    if let Some(sf) = self.surfaces.get(m) {
                        theme::kv(ui, p, "Metalness", format!("{:.2}", sf.metalness));
                        theme::kv(ui, p, "Roughness", format!("{:.2}", sf.roughness));
                        theme::kv(ui, p, "IOR", format!("{:.3}", sf.ior));
                    }
                }
                if ui
                    .add(theme::toggle_button(p, "Open Materials Lab", false))
                    .clicked()
                {
                    bus.push(AppCommand::SwitchMode {
                        mode: "materials".into(),
                    });
                }
            });
        }
        if self.objs[i].mesh.is_some() {
            self.physics_card(ui, p, bus, i);
        }
        theme::card(ui, p, "Animation · Y position", |ui| {
            let o = &self.objs[i];
            theme::kv(ui, p, "Keys", format!("{}", o.track_y.keys().len()));
            ui.horizontal(|ui| {
                if ui
                    .add(theme::primary_button(p, "Add key"))
                    .on_hover_text("Key the current Y at the playhead  (K)")
                    .clicked()
                {
                    bus.push(AppCommand::SetKeyframe);
                }
                let near = o.track_y.has_key_near(t, 0.05);
                if ui
                    .add_enabled(near, theme::toggle_button(p, "Delete key", false))
                    .clicked()
                {
                    bus.push(AppCommand::DeleteKeyframe);
                }
            });
            if o.kind == Kind::Joint {
                if let Ok(j) = self.joints.get(&o.name) {
                    let (x, y, z) = j.position();
                    theme::kv(
                        ui,
                        p,
                        "JointHierarchy",
                        format!("{:.2}, {:.2}, {:.2}", x, y, z),
                    );
                }
            }
        });
    }

    pub fn timeline(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        ui.horizontal(|ui| timeline::transport(ui, p, &mut self.clock, bus));
        ui.add_space(SP_XS);
        let tracks: Vec<Track> = self
            .objs
            .iter()
            .enumerate()
            .filter(|(i, o)| !o.track_y.is_empty() || self.selected == Some(*i))
            .map(|(i, o)| Track {
                label: &o.name,
                keys: &o.track_y,
                selected: self.selected == Some(i),
            })
            .collect();
        let names: Vec<String> = tracks.iter().map(|t| t.label.to_string()).collect();
        let out = timeline::ruler(ui, p, "scene-ruler", &mut self.clock, &tracks);
        if let Some(k) = out.picked_track {
            if let Some(name) = names.get(k) {
                self.selected = self.objs.iter().position(|o| &o.name == name);
            }
        }
    }
}

/// Energy books of the running arena: stocks change by exactly the flows.
fn energy_ledger(ui: &mut egui::Ui, p: &Palette, a: &obs_rust_bitstream::arena::Arena) {
    let Some(e0) = a.ledger.start else { return };
    let e = a.energy();
    let l = &a.ledger;
    let kj = |j: f64| {
        if j.abs() >= 1e6 {
            format!("{:+.3} MJ", j / 1e6)
        } else {
            format!("{:+.2} kJ", j / 1e3)
        }
    };
    egui::CollapsingHeader::new(
        egui::RichText::new("Energy ledger")
            .size(11.0)
            .color(p.text),
    )
    .id_salt("energy-ledger")
    .default_open(true)
    .show(ui, |ui| {
        let row = |ui: &mut egui::Ui, k: &str, v: String| theme::kv(ui, p, k, v);
        row(ui, "Kinetic", kj(e.kinetic));
        row(ui, "Potential", kj(e.potential));
        row(
            ui,
            "Δ Thermal",
            kj(e.internal - (e0 - a.ledger_start_mech())),
        );
        row(ui, "Heaters in", kj(l.heater_in));
        row(ui, "Air / radiation", kj(l.exchange_in));
        row(ui, "Drag work", kj(l.drag_work));
        row(ui, "Impacts → heat", kj(l.dissipated));
        row(ui, "Conducted", kj(l.conducted));
        let r = a.energy_residual();
        ui.label(
            egui::RichText::new(format!("Residual {:+.3} J (stocks − start − flows)", r))
                .size(10.5)
                .color(
                    if r.abs() < 1e-3 * (1.0 + e.kinetic + l.heater_in.abs() + l.exchange_in.abs())
                    {
                        p.success
                    } else {
                        p.warn
                    },
                ),
        );
    });
}

// ---- project files ------------------------------------------------------------

/// 3D scene in a `.seraphina` project.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SceneFile {
    pub objs: Vec<Obj>,
    pub selected: Option<usize>,
    pub prev_selected: Option<usize>,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub grid: bool,
    pub clock: Clock,
    pub joints: JointHierarchy,
    pub gizmo: Gizmo,
    pub shading: Shading,
    pub shelf_tab: usize,
    pub assign: MaterialAssign,
    /// Next auto-name number ("cube3").
    pub counter: usize,
    /// Physics: the running (or paused) arena with its bodies, materials,
    /// environment and energy ledger.
    #[serde(default)]
    pub arena: Option<Arena>,
    #[serde(default = "Environment::earth")]
    pub arena_env: Environment,
    #[serde(default)]
    pub arena_playing: bool,
    #[serde(default)]
    pub phys: HashMap<String, PhysProps>,
    /// The scene before the simulation started (Reset restores it).
    #[serde(default)]
    pub arena_start: Option<Vec<Obj>>,
}

impl SceneState {
    pub fn to_file(&self) -> SceneFile {
        SceneFile {
            objs: self.objs.clone(),
            selected: self.selected,
            prev_selected: self.prev_selected,
            yaw: self.yaw,
            pitch: self.pitch,
            dist: self.dist,
            grid: self.grid,
            clock: self.clock.clone(),
            joints: self.joints.clone(),
            gizmo: self.gizmo,
            shading: self.shading,
            shelf_tab: self.shelf_tab,
            assign: self.assign.clone(),
            counter: self.counter,
            arena: self.arena.clone(),
            arena_env: self.arena_env.clone(),
            arena_playing: self.arena_playing,
            phys: self.phys.clone(),
            arena_start: self.arena_start.clone(),
        }
    }

    pub fn load_file(&mut self, f: SceneFile) -> Result<(), String> {
        let n = f.objs.len();
        if let Some(bad) = f.objs.iter().find(|o| o.parent.is_some_and(|p| p >= n)) {
            return Err(format!("3D object '{}' has a missing parent", bad.name));
        }
        *self = Self {
            objs: f.objs,
            selected: f.selected.filter(|i| *i < n),
            prev_selected: f.prev_selected.filter(|i| *i < n),
            yaw: f.yaw,
            pitch: f.pitch,
            dist: f.dist,
            grid: f.grid,
            clock: f.clock,
            joints: f.joints,
            gizmo: f.gizmo,
            shading: f.shading,
            shelf_tab: f.shelf_tab,
            assign: f.assign,
            counter: f.counter,
            arena: f.arena,
            arena_env: f.arena_env,
            arena_playing: f.arena_playing,
            phys: f.phys,
            arena_start: f.arena_start,
            ..Self::new()
        };
        self.sync_joints();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The offline help tip for 3D describes the viewport the app has: every
    /// Display mode with its key, and no claim that rendering is missing.
    #[test]
    fn offline_3d_tip_matches_the_display_modes() {
        use obs_rust_bitstream::assistant::{
            AssistantBackend, ChatMessage, OfflineBackend, Role, WorkspaceContext,
        };
        let tip = OfflineBackend
            .respond(
                &[ChatMessage::new(Role::User, "help")],
                &WorkspaceContext {
                    mode: "3D".into(),
                    ..Default::default()
                },
            )
            .expect("offline tip");
        for (s, key) in DISPLAY_MODES {
            assert!(tip.contains(&format!("{} ({key})", s.label())), "{tip}");
        }
        assert!(!tip.to_lowercase().contains("no renderer"), "{tip}");
        assert!(!tip.contains("Wireframe only"), "{tip}");
    }

    fn toasts() -> Toasts {
        Toasts {
            now: 0.0,
            items: Vec::new(),
        }
    }

    #[test]
    fn modelling_tools_work_on_primitives_through_commands() {
        let mut s = SceneState::new();
        let mut t = toasts();
        let run = |s: &mut SceneState, t: &mut Toasts, c: AppCommand| s.apply(&c, t).unwrap();
        // edge loop on the cube
        run(
            &mut s,
            &mut t,
            AppCommand::SceneEdgeLoop {
                object: None,
                edge: None,
                t: None,
            },
        )
        .unwrap();
        let m = s.objs[0].mesh.as_ref().unwrap();
        assert_eq!(m.faces.len(), 10);
        assert!(m.is_closed());
        // multi-cut on a plane
        run(
            &mut s,
            &mut t,
            AppCommand::SceneAdd {
                kind: "plane".into(),
            },
        )
        .unwrap();
        run(
            &mut s,
            &mut t,
            AppCommand::SceneMultiCut {
                object: None,
                face: None,
                from_edge: None,
                to_edge: None,
                from_t: None,
                to_t: None,
            },
        )
        .unwrap();
        assert_eq!(
            s.objs[s.selected.unwrap()]
                .mesh
                .as_ref()
                .unwrap()
                .faces
                .len(),
            2
        );
        // quad draw welds
        let q = |x: f32| {
            [
                [x, 0.0, 0.0],
                [x + 1.0, 0.0, 0.0],
                [x + 1.0, 0.0, 1.0],
                [x, 0.0, 1.0],
            ]
        };
        run(
            &mut s,
            &mut t,
            AppCommand::SceneQuadDraw {
                points: q(3.0).to_vec(),
            },
        )
        .unwrap();
        run(
            &mut s,
            &mut t,
            AppCommand::SceneQuadDraw {
                points: q(4.0).to_vec(),
            },
        )
        .unwrap();
        let qd = s
            .objs
            .iter()
            .find(|o| o.name == "quadDraw1")
            .unwrap()
            .mesh
            .as_ref()
            .unwrap();
        assert_eq!((qd.verts.len(), qd.faces.len()), (6, 2));
        assert!(run(
            &mut s,
            &mut t,
            AppCommand::SceneQuadDraw {
                points: vec![[0.0; 3]; 3]
            }
        )
        .is_err());
    }

    #[test]
    fn boolean_and_bridge_between_two_objects() {
        let mut t = toasts();
        for (op, vol) in [
            ("union", 1.5f32),
            ("difference", 0.5),
            ("intersection", 0.5),
        ] {
            let mut s = SceneState::new();
            s.apply(
                &AppCommand::SceneAdd {
                    kind: "cube".into(),
                },
                &mut t,
            )
            .unwrap()
            .unwrap();
            s.apply(
                &AppCommand::SceneTransform {
                    object: None,
                    translate: Some([0.5, 0.0, 0.0]),
                    rotate_y: None,
                    scale: None,
                },
                &mut t,
            )
            .unwrap()
            .unwrap();
            // select cube1 last so the new cube is the second operand
            s.apply(
                &AppCommand::SceneSelect {
                    object: "cube1".into(),
                },
                &mut t,
            )
            .unwrap()
            .unwrap();
            let n = s.objs.len();
            let msg = s
                .apply(
                    &AppCommand::SceneBoolean {
                        op: op.into(),
                        object: None,
                        with: None,
                    },
                    &mut t,
                )
                .unwrap()
                .unwrap();
            assert!(msg.contains(op), "{}", msg);
            assert_eq!(s.objs.len(), n - 1);
            let m = s.objs[s.selected.unwrap()].mesh.as_ref().unwrap();
            assert!(
                (m.volume() - vol).abs() < 2e-3,
                "{} volume {}",
                op,
                m.volume()
            );
        }
        let mut s = SceneState::new();
        s.apply(
            &AppCommand::SceneAdd {
                kind: "cube".into(),
            },
            &mut t,
        )
        .unwrap()
        .unwrap();
        s.apply(
            &AppCommand::SceneTransform {
                object: None,
                translate: Some([2.0, 0.0, 0.0]),
                rotate_y: None,
                scale: None,
            },
            &mut t,
        )
        .unwrap()
        .unwrap();
        s.apply(
            &AppCommand::SceneSelect {
                object: "cube1".into(),
            },
            &mut t,
        )
        .unwrap()
        .unwrap();
        s.apply(
            &AppCommand::SceneBridge {
                object: None,
                with: None,
            },
            &mut t,
        )
        .unwrap()
        .unwrap();
        let m = s.objs[0].mesh.as_ref().unwrap();
        assert!(m.is_closed());
        assert!((m.volume() - 3.0).abs() < 1e-3, "{}", m.volume());
        // a boolean with nothing else to combine is a clear error
        let mut lone = SceneState::new();
        assert!(lone
            .apply(
                &AppCommand::SceneBoolean {
                    op: "union".into(),
                    object: None,
                    with: None
                },
                &mut t
            )
            .unwrap()
            .is_err());
    }

    #[test]
    fn shaded_viewport_renders_selection_highlight_and_picks() {
        let mut s = SceneState::new();
        s.dist = 4.0;
        let f = s.render(
            240,
            160,
            Color32::from_rgb(20, 20, 22),
            Color32::from_gray(60),
            Color32::from_rgb(214, 180, 92),
        );
        let mesh_px = f.ids.iter().filter(|&&i| i >> 16 == 1).count();
        assert!(mesh_px > 2000, "{}", mesh_px);
        // highlighted (gold tinted) vs. unselected grey
        let goldish = (0..f.ids.len())
            .filter(|&i| {
                f.ids[i] >> 16 == 1 && f.rgba[i * 4] as i32 - f.rgba[i * 4 + 2] as i32 > 20
            })
            .count();
        assert!(
            goldish > mesh_px / 2,
            "selected mesh is tinted ({} of {})",
            goldish,
            mesh_px
        );
        s.selected = None;
        let g = s.render(
            240,
            160,
            Color32::from_rgb(20, 20, 22),
            Color32::from_gray(60),
            Color32::from_rgb(214, 180, 92),
        );
        let goldish = (0..g.ids.len())
            .filter(|&i| {
                g.ids[i] >> 16 == 1 && g.rgba[i * 4] as i32 - g.rgba[i * 4 + 2] as i32 > 20
            })
            .count();
        assert!(goldish < mesh_px / 10);
        s.shading = Shading::Wireframe;
        let w = s.render(
            240,
            160,
            Color32::from_rgb(20, 20, 22),
            Color32::from_gray(60),
            Color32::from_rgb(214, 180, 92),
        );
        assert_eq!(
            w.ids.iter().filter(|&&i| i != 0).count(),
            0,
            "wireframe draws no faces"
        );
    }
}

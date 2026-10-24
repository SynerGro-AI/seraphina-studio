//! Seraphina Studio — unified creative-suite shell (egui / eframe).
//!
//! cargo run --release
//!
//! Every button, shortcut and AI tool call goes through one typed
//! command bus (`commands::AppCommand`, dispatched by `App::dispatch`).
//! Seraphina (assistant_panel) drives the same bus through an LLM
//! provider (xAI by default), the Seraphina.AGI CLI for memory, or a
//! labelled offline help mode.

#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod animate;
mod assistant_panel;
mod camera;
mod commands;
mod composite;
mod crash;
mod lab;
mod llm;
mod materials;
mod maya;
mod net;
mod output;
mod paint;
mod paint_undo;
mod prefs;
mod project;
mod scene;
mod secrets;
mod settings;
mod single;
mod stream;
#[cfg(test)]
mod test_dir;
mod text_raster;
mod theme;
mod tiled_tex;
mod timeline;
mod usage;
mod videogen;
mod workspace;

use assistant_panel::{ActionLine, LineStatus, PanelRequest};
use commands::{parse_tool_call, AppCommand, Origin};
use eframe::egui::{self, Color32, CornerRadius, Key, Stroke, Vec2};
use egui_phosphor::regular as ic;
use obs_rust_bitstream::assistant::WorkspaceContext;
use obs_rust_bitstream::ui_canvas::{format_clock, BlendMode, KeyTrack, Raster};
use std::collections::HashMap;
use theme::{Palette, SP_LG, SP_MD, SP_SM, SP_XS};

/// Find a bundled file: as given, next to the exe, or one/two/three levels up.
pub fn resolve_asset(path: &str) -> std::path::PathBuf {
    resolve_asset_from(path, std::env::current_exe().ok())
}

/// `resolve_asset` with the exe path passed in, so the unzipped release
/// layout (files next to SeraphinaStudio.exe) can be tested.
fn resolve_asset_from(path: &str, exe: Option<std::path::PathBuf>) -> std::path::PathBuf {
    let p = std::path::Path::new(path);
    if p.is_absolute() || p.exists() {
        return p.to_path_buf();
    }
    if let Some(exe) = exe {
        let mut dir = exe.parent().map(|d| d.to_path_buf());
        for _ in 0..4 {
            if let Some(d) = &dir {
                let c = d.join(path);
                if c.exists() {
                    return c;
                }
                dir = d.parent().map(|x| x.to_path_buf());
            }
        }
    }
    p.to_path_buf()
}

// ---- toasts -------------------------------------------------------------

/// B3: answers to the first-send privacy prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Consent {
    All,
    MessagesOnly,
    No,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    Info,
    Success,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct Toast {
    pub kind: ToastKind,
    pub text: String,
    pub born: f64,
    /// Seconds on screen.
    pub life: f64,
}

#[derive(Default)]
pub struct Toasts {
    pub now: f64,
    pub items: Vec<Toast>,
}

impl Toasts {
    const LIFE: f64 = 3.6;

    pub fn push(&mut self, kind: ToastKind, text: &str) {
        self.push_for(kind, text, Self::LIFE);
    }

    /// A toast that stays up for `life` seconds (startup notices).
    pub fn push_for(&mut self, kind: ToastKind, text: &str, life: f64) {
        self.items.push(Toast {
            kind,
            text: text.to_string(),
            born: self.now,
            life,
        });
        if self.items.len() > 4 {
            self.items.remove(0);
        }
    }

    fn show(&mut self, ctx: &egui::Context, p: &Palette) {
        let now = self.now;
        self.items.retain(|t| now - t.born < t.life);
        if self.items.is_empty() {
            return;
        }
        egui::Area::new(egui::Id::new("toasts"))
            .anchor(egui::Align2::CENTER_BOTTOM, Vec2::new(0.0, -48.0))
            .order(egui::Order::Foreground)
            .interactable(false)
            .show(ctx, |ui| {
                for t in &self.items {
                    let age = now - t.born;
                    let a = ((age / 0.18).min(1.0) * ((t.life - age) / 0.35).min(1.0))
                        .clamp(0.0, 1.0) as f32;
                    let dot = match t.kind {
                        ToastKind::Info => p.accent,
                        ToastKind::Success => p.success,
                        ToastKind::Warn => p.warn,
                        ToastKind::Error => p.danger,
                    };
                    egui::Frame::NONE
                        .fill(p.surface.gamma_multiply(a))
                        .stroke(egui::Stroke::new(1.0_f32, p.border.gamma_multiply(a)))
                        .corner_radius(CornerRadius::same(99))
                        .shadow(egui::epaint::Shadow {
                            offset: [0, 6],
                            blur: 18,
                            spread: 0,
                            color: Color32::from_black_alpha((60.0 * a) as u8),
                        })
                        .inner_margin(egui::Margin::symmetric(SP_LG as i8 - 1, SP_SM as i8 - 1))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(8.0), egui::Sense::hover());
                                ui.painter()
                                    .circle_filled(r.center(), 4.0, dot.gamma_multiply(a));
                                ui.label(
                                    egui::RichText::new(&t.text)
                                        .color(p.text.gamma_multiply(a))
                                        .size(13.0),
                                );
                            });
                        });
                    ui.add_space(SP_XS);
                }
            });
        ctx.request_repaint();
    }
}

// ---- app ----------------------------------------------------------------

/// World bounding-box centre of an object's mesh (its position if it has none).
fn a_center(o: &scene::Obj) -> [f32; 3] {
    match o.world_mesh().filter(|m| !m.verts.is_empty()) {
        Some(m) => {
            let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for v in &m.verts {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            [
                (lo[0] + hi[0]) * 0.5,
                (lo[1] + hi[1]) * 0.5,
                (lo[2] + hi[2]) * 0.5,
            ]
        }
        None => o.pos,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Paint,
    Composite,
    Scene,
    Animate,
    Stream,
    Materials,
}

impl Mode {
    /// Header order, as in the mockup.
    pub const ALL: [Mode; 6] = [
        Mode::Paint,
        Mode::Composite,
        Mode::Scene,
        Mode::Animate,
        Mode::Stream,
        Mode::Materials,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Mode::Stream => "Stream",
            Mode::Materials => "Materials",
            Mode::Paint => "Paint",
            Mode::Scene => "3D",
            Mode::Animate => "Animate",
            Mode::Composite => "Composite",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Mode::Stream => ic::BROADCAST,
            Mode::Materials => ic::ATOM,
            Mode::Paint => ic::PAINT_BRUSH,
            Mode::Scene => ic::CUBE,
            Mode::Animate => ic::FILM_STRIP,
            Mode::Composite => ic::STACK,
        }
    }

    pub fn style(&self) -> (&'static str, &'static str) {
        match self {
            Mode::Stream => ("OBS", "OBS-style streaming and bitstream inspection"),
            Mode::Materials => (
                "Lab",
                "Materials Lab: periodic table, composition builder, PBR materials",
            ),
            Mode::Paint => ("Ps", "Photoshop-style raster editing"),
            Mode::Scene => ("Ma", "Maya-style 3D layout"),
            Mode::Animate => ("Tb", "Toon Boom-style frame-by-frame animation"),
            Mode::Composite => ("Ae", "After Effects-style keyframed compositing"),
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        match s.trim().to_lowercase().as_str() {
            "stream" | "obs" => Some(Mode::Stream),
            "materials" | "material" | "lab" | "materials_lab" | "materials lab" => {
                Some(Mode::Materials)
            }
            "paint" | "photoshop" => Some(Mode::Paint),
            "3d" | "scene" | "maya" => Some(Mode::Scene),
            "animate" | "animation" => Some(Mode::Animate),
            "composite" | "compositing" => Some(Mode::Composite),
            _ => None,
        }
    }

    fn has_timeline(&self) -> bool {
        matches!(self, Mode::Scene | Mode::Animate | Mode::Composite)
    }

    fn has_shelf(&self) -> bool {
        matches!(self, Mode::Paint | Mode::Scene)
    }
}

/// How many AI batches stay undoable. Each snapshot is a full copy of the
/// documents (Paint plus all Animate frames: tens of MB), so older ones are
/// dropped instead of growing memory for the whole session.
pub const MAX_AI_SNAPSHOTS: usize = 10;

/// State captured before an AI batch so one click can undo all of it.
pub struct Snapshot {
    mode: Mode,
    paint: paint::PaintDoc,
    scene_objs: Vec<scene::Obj>,
    scene_sel: Option<usize>,
    comp_layers: Vec<composite::CompLayer>,
    comp_sel: usize,
    scene_assign: obs_rust_bitstream::material_assign::MaterialAssign,
    mats: materials::MatSnap,
    anim_frames: Vec<Raster>,
    live: bool,
    rec: bool,
}

/// 3D scene state for the general undo history.
#[derive(Clone)]
struct SceneSnap {
    objs: Vec<scene::Obj>,
    assign: obs_rust_bitstream::material_assign::MaterialAssign,
    selected: Option<usize>,
    prev: Option<usize>,
}

/// Composite layer parameters (pixels never change in Composite, so they are not copied).
#[derive(Clone)]
struct CompSnap {
    params: Vec<(f32, bool, BlendMode, KeyTrack)>,
    selected: usize,
}

/// Linear undo/redo stack. Repeated edits of the same kind on the same target
/// within `COALESCE_S` (a drag, a slider) collapse into one step.
pub struct History<T> {
    undo: Vec<T>,
    redo: Vec<T>,
    last: Option<(String, f64)>,
}

const HISTORY_LIMIT: usize = 64;
const COALESCE_S: f64 = 0.6;

impl<T> History<T> {
    fn new() -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            last: None,
        }
    }
    /// Record `before` unless this edit continues the previous one (keys starting with `!` never merge). Returns true when a step was pushed.
    fn record(&mut self, key: String, now: f64, before: T) -> bool {
        let merge = !key.starts_with('!')
            && matches!(&self.last, Some((k, t)) if *k == key && now - *t < COALESCE_S);
        self.last = Some((key, now));
        if merge {
            return false;
        }
        self.undo.push(before);
        if self.undo.len() > HISTORY_LIMIT {
            self.undo.remove(0);
        }
        self.redo.clear();
        true
    }
    fn cancel(&mut self) {
        self.undo.pop();
        self.last = None;
    }
    fn undo(&mut self, current: T) -> Option<T> {
        let s = self.undo.pop()?;
        self.redo.push(current);
        self.last = None;
        Some(s)
    }
    fn redo(&mut self, current: T) -> Option<T> {
        let s = self.redo.pop()?;
        self.undo.push(current);
        self.last = None;
        Some(s)
    }
    pub fn depth(&self) -> (usize, usize) {
        (self.undo.len(), self.redo.len())
    }
}

pub struct App {
    pub ctx: egui::Context,
    /// The running physics arena while it plays or steps in the background;
    /// `scene.arena` then holds its latest snapshot.
    pub arena_run:
        Option<obs_rust_bitstream::sim_worker::SimWorker<obs_rust_bitstream::arena::Arena>>,
    /// Frame time not yet handed to a busy arena worker (s).
    pub arena_carry: f32,
    pub dark: bool,
    pub pal: Palette,
    pub mode: Mode,
    pub show_tools: bool,
    pub show_inspector: bool,
    pub show_timeline: bool,
    pub show_shelf: bool,
    pub show_assistant: bool,
    pub show_shortcuts: bool,
    pub show_settings: bool,
    pub show_account: bool,
    /// 0 = Seraphina, 1 = Grok Imagine.
    pub dock_tab: usize,
    pub shelf_tab: usize,
    pub native_frame: bool,
    /// Repaint at ~10 fps even when idle (SERAPHINA_CONTINUOUS; for screen capture).
    pub continuous: bool,
    pub toasts: Toasts,
    pub stream: stream::StreamState,
    pub output: output::Output,
    pub paint: paint::PaintState,
    pub scene: scene::SceneState,
    pub anim: animate::AnimState,
    pub comp: composite::CompState,
    pub materials: materials::MaterialsState,
    pub lab: lab::LabState,
    pub assistant: assistant_panel::AssistantPanel,
    pub video: videogen::VideoGenPanel,
    pub bus: Vec<AppCommand>,
    pub log: Vec<(Origin, String, Result<String, String>)>,
    snapshots: HashMap<u64, Snapshot>,
    scene_hist: History<SceneSnap>,
    comp_hist: History<CompSnap>,
    mat_hist: History<materials::MatSnap>,
    sel_ov: paint::SelOverlay,
    last_time: Option<f64>,
    /// `.seraphina` file, dirty state, autosave and recovery.
    pub project: workspace::ProjectState,
    /// Crash reports from an earlier run, shown once (D1).
    pub crash_notice: Option<crash::CrashNotice>,
    /// B3: the first-send privacy prompt is showing.
    pub consent_open: bool,
    /// Settings read before the window opens (renderer, UI scale).
    pub app_settings: settings::AppSettings,
    /// The window is too small for the chosen UI scale right now.
    pub ui_scale_capped: bool,
    /// UI zoom seen last frame, and passes left before re-fitting the
    /// canvas (egui's first pass after a zoom change has a bogus screen
    /// size, so a fit then would zoom the canvas to ~600%).
    ui_zoom_seen: f32,
    refit_in: u8,
    /// Settings ▸ xAI API key (C7): the text box and a running key test.
    xai_edit: String,
    xai_test_rx: Option<std::sync::mpsc::Receiver<Result<usize, String>>>,
    /// The key in use, looked up once (not every frame) and on change.
    xai_cache: Option<Option<(String, secrets::KeySource)>>,
    /// xAI tokens and cost, and the spend cap (C3).
    pub meter: usage::Meter,
    /// Test scratch folders this App uses, removed when it drops (even
    /// when the test panics). Last, so everything using them drops first.
    #[cfg(test)]
    test_dirs: Vec<test_dir::TestDir>,
}

pub fn install_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    egui_phosphor::add_to_fonts(&mut fonts, egui_phosphor::Variant::Regular);
    // Hack carries geometric glyphs (◆ ● →) the UI font lacks.
    if let Some(f) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        f.push("Hack".to_owned());
    }
    ctx.set_fonts(fonts);
}

impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        let mut app = Self::with(
            ctx,
            assistant_panel::AssistantPanel::new(prefs::Prefs::load(prefs::default_path())),
            videogen::VideoGenPanel::new(),
        );
        app.project.native_dialogs = project::native_dialogs_available();
        app.start_session(project::autosave_dir());
        app.crash_notice = crash::CrashNotice::check();
        app.app_settings = settings::AppSettings::load().0;
        app.meter = usage::Meter::load(app.app_settings.spend_cap_usd);
        if let Some(why) = crash::take_fallback_note() {
            app.toasts.push_for(
                ToastKind::Warn,
                &format!(
                    "Using {} because the other renderer couldn't start ({}). Change it in Settings > Graphics.",
                    crash::renderer(),
                    why
                ),
                15.0,
            );
        }
        app
    }

    /// Enable autosave in `dir` and look for a crashed session there.
    pub fn start_session(&mut self, dir: std::path::PathBuf) {
        let mut st = workspace::ProjectState::new(project::Session::start(dir));
        st.include_chat = self.project.include_chat;
        st.script_files = self.project.script_files;
        st.docs_dir = std::mem::take(&mut self.project.docs_dir);
        st.native_dialogs = self.project.native_dialogs;
        self.project = st;
        self.mark_saved();
    }

    pub fn with(
        ctx: &egui::Context,
        assistant: assistant_panel::AssistantPanel,
        video: videogen::VideoGenPanel,
    ) -> Self {
        let pal = Palette::dark();
        install_fonts(ctx);
        // Ctrl+= / Ctrl+- / Ctrl+0 belong to the Paint canvas (handle_keys);
        // egui's built-in keyboard zoom would scale the whole UI as well.
        ctx.options_mut(|o| o.zoom_with_keyboard = false);
        theme::apply(ctx, &pal);
        let mut toasts = Toasts::default();
        let paint = paint::PaintState::new(&mut toasts);
        let mut app = Self {
            ctx: ctx.clone(),
            dark: true,
            pal,
            mode: Mode::Paint,
            show_tools: true,
            show_inspector: true,
            show_timeline: true,
            show_shelf: true,
            show_assistant: true,
            show_shortcuts: false,
            show_settings: false,
            show_account: false,
            dock_tab: 0,
            shelf_tab: 0,
            native_frame: true,
            continuous: false,
            stream: stream::StreamState::new(),
            output: output::Output::load(),
            paint,
            scene: scene::SceneState::new(),
            anim: animate::AnimState::new(),
            comp: composite::CompState::new(),
            materials: materials::MaterialsState::new(),
            lab: lab::LabState::new(),
            assistant,
            video,
            bus: Vec::new(),
            log: Vec::new(),
            snapshots: HashMap::new(),
            arena_run: None,
            arena_carry: 0.0,
            scene_hist: History::new(),
            comp_hist: History::new(),
            mat_hist: History::new(),
            sel_ov: paint::SelOverlay::default(),
            toasts,
            last_time: None,
            project: workspace::ProjectState::new(project::Session::default()),
            crash_notice: None,
            consent_open: false,
            app_settings: settings::AppSettings::default(),
            ui_scale_capped: false,
            ui_zoom_seen: 1.0,
            refit_in: 0,
            xai_edit: String::new(),
            xai_test_rx: None,
            meter: usage::Meter::ephemeral(0.0),
            xai_cache: None,
            #[cfg(test)]
            test_dirs: Vec::new(),
        };
        if let Some(w) = app.assistant.prefs.load_warning.clone() {
            app.toasts.push(ToastKind::Warn, &w);
        }
        app.mark_saved();
        app
    }

    pub fn set_dark(&mut self, dark: bool) {
        self.dark = dark;
        self.pal = Palette::for_mode(dark);
        theme::apply(&self.ctx, &self.pal);
    }

    fn now(&self) -> f64 {
        self.toasts.now
    }

    pub fn current_tool(&self) -> String {
        match self.mode {
            Mode::Stream => format!(
                "Read {} bit{}",
                self.stream.width,
                if self.stream.width == 1 { "" } else { "s" }
            ),
            Mode::Paint => self.paint.tool.label().to_string(),
            Mode::Scene => format!("{} Tool", self.scene.gizmo.label()),
            Mode::Animate => if self.anim.erase { "Eraser" } else { "Pencil" }.to_string(),
            Mode::Composite => "Keyframe".to_string(),
            Mode::Materials => self
                .materials
                .selected
                .and_then(obs_rust_bitstream::element_data::get)
                .map(|e| e.name.to_string())
                .unwrap_or_else(|| "Periodic table".into()),
        }
    }

    pub fn zoom_label(&self) -> String {
        match self.mode {
            Mode::Paint => format!("{:.1}%", self.paint.zoom * 100.0),
            Mode::Scene => format!("dolly {:.1}", self.scene.dist),
            _ => "Fit".into(),
        }
    }

    pub fn resolution_label(&self) -> String {
        match self.mode {
            Mode::Paint => format!("{} × {} px", self.paint.width(), self.paint.height()),
            Mode::Animate => format!("{} × {} px", animate::W, animate::H),
            Mode::Composite => format!("{} × {} px", composite::PW, composite::PH),
            Mode::Scene => "Viewport".into(),
            Mode::Stream => format!("{} bytes", self.stream.insp.data().len()),
            Mode::Materials => "118 elements".into(),
        }
    }

    pub fn document_label(&self) -> String {
        match self.mode {
            Mode::Paint => format!("{}.psd", self.paint.doc_name),
            Mode::Stream => self.stream.source.clone(),
            Mode::Scene => "Untitled scene".into(),
            Mode::Animate => format!("{} frames", self.anim.frames.len()),
            Mode::Composite => "Comp 1".into(),
            Mode::Materials => format!("{} materials", self.materials.lib.profiles.len()),
        }
    }

    fn workspace_ctx(&self) -> WorkspaceContext {
        WorkspaceContext {
            mode: self.mode.label().to_string(),
            tool: self.current_tool(),
            detail: String::new(),
        }
    }

    /// State description for Seraphina's system prompt.
    pub fn app_context(&self) -> String {
        let vid = match &self.video.provider {
            Some(_) => format!("configured, {} clips", self.video.clips.len()),
            None => "not configured (no key)".into(),
        };
        format!(
            "Active mode: {} (modes: Paint, Composite, 3D, Animate, Stream, Materials). Theme {}. Panels: tools {}, inspector {}, timeline {}, shelf {}, assistant {}.\nPaint: {}\n{}\n{}\n{}\n{}\n{}\n{} {}\nGrok Imagine video: {}.",
            self.mode.label(),
            if self.dark { "dark" } else { "light" },
            self.show_tools,
            self.show_inspector,
            self.show_timeline,
            self.show_shelf,
            self.show_assistant,
            self.paint.context(),
            self.scene.context(),
            self.anim.context(),
            self.comp.context(),
            self.stream.context(),
            self.output.context(),
            self.materials.context(),
            self.lab.context(),
            vid
        )
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            mode: self.mode,
            paint: self.paint.doc.clone(),
            scene_objs: self.scene.objs.clone(),
            scene_sel: self.scene.selected,
            comp_layers: self.comp.layers.clone(),
            comp_sel: self.comp.selected,
            scene_assign: self.scene.assign.clone(),
            mats: self.materials.snap(),
            anim_frames: self.anim.frames.clone(),
            live: self.output.is_live(),
            rec: self.output.is_recording(),
        }
    }

    fn restore(&mut self, s: Snapshot) {
        self.mode = s.mode;
        self.paint.set_doc(s.paint);
        self.scene.objs = s.scene_objs;
        self.scene.selected = s.scene_sel;
        self.comp.layers = s.comp_layers;
        self.comp.selected = s.comp_sel;
        self.scene.assign = s.scene_assign;
        self.arena_cancel();
        self.scene.arena = None;
        self.scene.arena_playing = false;
        self.scene.arena_start = None;
        self.materials.restore(s.mats);
        self.anim.restore(s.anim_frames);
        // undoing an AI batch that started output really stops it
        if !s.live && self.output.is_live() {
            let _ = self.output.stop_live();
        }
        if !s.rec && self.output.is_recording() {
            if let Err(e) = self.output.stop_record() {
                self.toasts.push(ToastKind::Error, &e);
            }
        }
    }

    /// The single entry point for every action in the app.
    pub fn dispatch(&mut self, cmd: &AppCommand, origin: Origin) -> Result<String, String> {
        if self.arena_run.is_some() {
            let name = cmd.name();
            match cmd {
                // stopping or throwing away a run doesn't wait for it
                AppCommand::ArenaReset
                | AppCommand::ArenaPlay {
                    playing: Some(false),
                }
                | AppCommand::NewProject
                | AppCommand::OpenProject { .. } => self.arena_cancel(),
                _ if name.starts_with("arena")
                    || name.starts_with("scene")
                    || name.starts_with("material")
                    || matches!(
                        cmd,
                        AppCommand::Undo | AppCommand::Redo | AppCommand::SaveProject { .. }
                    ) =>
                {
                    self.arena_sync()
                }
                _ => {}
            }
        }
        let now = self.now();
        let recorded = self.record_history(cmd, now);
        let r = self.dispatch_inner(cmd);
        match (recorded, &r) {
            (Some(Mode::Scene), Err(_)) => self.scene_hist.cancel(),
            (Some(Mode::Composite), Err(_)) => self.comp_hist.cancel(),
            (Some(Mode::Materials), Err(_)) => self.mat_hist.cancel(),
            _ => {}
        }
        self.finish(cmd, origin, r)
    }

    /// Which edits are undoable, and the coalescing key for each.
    fn history_key(&self, cmd: &AppCommand) -> Option<(Mode, String)> {
        use AppCommand as C;
        let target = |o: &Option<String>| o.clone().unwrap_or_default();
        match cmd {
            C::SceneTransform {
                object,
                translate,
                rotate_y,
                scale,
            } => Some((
                Mode::Scene,
                format!(
                    "xf:{}:{}{}{}",
                    target(object),
                    translate.is_some() as u8,
                    rotate_y.is_some() as u8,
                    scale.is_some() as u8
                ),
            )),
            C::SceneAdd { .. }
            | C::SceneExtrude { .. }
            | C::SceneBevel { .. }
            | C::SceneDelete { .. }
            | C::SceneEdgeLoop { .. }
            | C::SceneMultiCut { .. }
            | C::SceneBridge { .. }
            | C::SceneBoolean { .. }
            | C::SceneQuadDraw { .. } => Some((Mode::Scene, format!("!{}", cmd.name()))),
            C::MaterialAssign { .. } => Some((Mode::Scene, "!material_assign".into())),
            // starting a simulation is one undo step (back to the pre-sim scene)
            C::ArenaPlay { .. } | C::ArenaStep { .. } if self.scene.arena.is_none() => {
                Some((Mode::Scene, "!arena_start".into()))
            }
            C::MaterialsAdd { .. }
            | C::MaterialsRemove { .. }
            | C::MaterialsClear
            | C::MaterialsBuild { .. }
            | C::MaterialsBuildAtom { .. }
            | C::MaterialsPreset { .. }
            | C::MaterialsSave { .. } => Some((Mode::Materials, format!("!{}", cmd.name()))),
            C::SetKeyframe | C::DeleteKeyframe if self.mode == Mode::Scene => {
                Some((Mode::Scene, format!("!{}", cmd.name())))
            }
            C::SetKeyframe | C::DeleteKeyframe if self.mode == Mode::Composite => {
                Some((Mode::Composite, format!("!{}", cmd.name())))
            }
            C::SetLayerOpacity { layer, .. } if self.mode == Mode::Composite => {
                Some((Mode::Composite, format!("opacity:{:?}", layer)))
            }
            C::SetLayerBlend { .. } | C::SetLayerVisibility { .. }
                if self.mode == Mode::Composite =>
            {
                Some((Mode::Composite, format!("!{}", cmd.name())))
            }
            _ => None,
        }
    }

    fn record_history(&mut self, cmd: &AppCommand, now: f64) -> Option<Mode> {
        let (mode, key) = self.history_key(cmd)?;
        let pushed = match mode {
            Mode::Scene => {
                let s = self.scene_snap();
                self.scene_hist.record(key, now, s)
            }
            Mode::Materials => {
                let s = self.materials.snap();
                self.mat_hist.record(key, now, s)
            }
            _ => {
                let s = self.comp_snap();
                self.comp_hist.record(key, now, s)
            }
        };
        pushed.then_some(mode)
    }

    fn scene_snap(&self) -> SceneSnap {
        SceneSnap {
            objs: self.scene.objs.clone(),
            assign: self.scene.assign.clone(),
            selected: self.scene.selected,
            prev: self.scene.prev_selected,
        }
    }

    fn set_scene_snap(&mut self, s: SceneSnap) {
        self.scene.objs = s.objs;
        self.scene.assign = s.assign;
        // undo ends any running simulation (its start state is what's restored)
        self.arena_cancel();
        self.scene.arena = None;
        self.scene.arena_playing = false;
        self.scene.arena_start = None;
        let n = self.scene.objs.len();
        self.scene.selected = s.selected.filter(|i| *i < n);
        self.scene.prev_selected = s.prev.filter(|i| *i < n);
    }

    fn comp_snap(&self) -> CompSnap {
        CompSnap {
            params: self
                .comp
                .layers
                .iter()
                .map(|l| {
                    (
                        l.layer.opacity,
                        l.layer.visible,
                        l.layer.blend,
                        l.opacity.clone(),
                    )
                })
                .collect(),
            selected: self.comp.selected,
        }
    }

    fn set_comp_snap(&mut self, s: CompSnap) {
        for (l, (o, v, b, k)) in self.comp.layers.iter_mut().zip(s.params) {
            l.layer.opacity = o;
            l.layer.visible = v;
            l.layer.blend = b;
            l.opacity = k;
        }
        self.comp.selected = s.selected.min(self.comp.layers.len().saturating_sub(1));
    }

    fn dispatch_inner(&mut self, cmd: &AppCommand) -> Result<String, String> {
        use AppCommand as C;
        let r: Result<String, String> = match cmd {
            C::SwitchMode { mode } => match Mode::parse(mode) {
                Some(m) => {
                    self.mode = m;
                    Ok(format!("switched to {}", m.label()))
                }
                None => Err(format!("unknown mode '{}'", mode)),
            },
            C::SetTheme { dark } => {
                self.set_dark(*dark);
                Ok(format!("{} theme", if *dark { "dark" } else { "light" }))
            }
            C::SetUiScale { percent } => {
                if !percent.is_finite() {
                    return Err("percent must be a number".into());
                }
                let ctx = self.ctx.clone();
                self.set_ui_scale(&ctx, percent / 100.0);
                Ok(format!(
                    "UI scale {:.0}%",
                    self.app_settings.scale() * 100.0
                ))
            }
            C::TogglePanel { panel, visible } => {
                let key = panel.trim().to_lowercase();
                if matches!(key.as_str(), "videogen" | "video") {
                    self.dock_tab = 1;
                }
                let slot = match key.as_str() {
                    "tools" => Some(&mut self.show_tools),
                    "inspector" | "layers" => Some(&mut self.show_inspector),
                    "timeline" => Some(&mut self.show_timeline),
                    "shelf" => Some(&mut self.show_shelf),
                    "assistant" | "seraphina" | "videogen" | "video" => {
                        Some(&mut self.show_assistant)
                    }
                    "shortcuts" => Some(&mut self.show_shortcuts),
                    _ => None,
                };
                match slot {
                    Some(s) => {
                        *s = visible.unwrap_or(!*s);
                        Ok(format!(
                            "{} panel {}",
                            panel,
                            if *s { "shown" } else { "hidden" }
                        ))
                    }
                    None => Err(format!("unknown panel '{}'", panel)),
                }
            }
            C::Undo | C::Redo => match self.mode {
                Mode::Paint => self.paint.apply(cmd).unwrap_or(Err("unsupported".into())),
                Mode::Animate => {
                    let done = if matches!(cmd, C::Undo) {
                        self.anim.undo()
                    } else {
                        self.anim.redo()
                    };
                    if done {
                        Ok(format!("Animate {}", cmd.name()))
                    } else {
                        Err(format!("nothing to {} in Animate", cmd.name()))
                    }
                }
                Mode::Scene => {
                    let cur = self.scene_snap();
                    let r = if matches!(cmd, C::Undo) {
                        self.scene_hist.undo(cur)
                    } else {
                        self.scene_hist.redo(cur)
                    };
                    match r {
                        Some(s) => {
                            self.set_scene_snap(s);
                            Ok(format!("3D {}", cmd.name()))
                        }
                        None => Err(format!("nothing to {} in 3D", cmd.name())),
                    }
                }
                Mode::Composite => {
                    let cur = self.comp_snap();
                    let r = if matches!(cmd, C::Undo) {
                        self.comp_hist.undo(cur)
                    } else {
                        self.comp_hist.redo(cur)
                    };
                    match r {
                        Some(s) => {
                            self.set_comp_snap(s);
                            Ok(format!("Composite {}", cmd.name()))
                        }
                        None => Err(format!("nothing to {} in Composite", cmd.name())),
                    }
                }
                Mode::Materials => {
                    let cur = self.materials.snap();
                    let r = if matches!(cmd, C::Undo) {
                        self.mat_hist.undo(cur)
                    } else {
                        self.mat_hist.redo(cur)
                    };
                    match r {
                        Some(s) => {
                            self.materials.restore(s);
                            Ok(format!("Materials {}", cmd.name()))
                        }
                        None => Err(format!("nothing to {} in Materials", cmd.name())),
                    }
                }
                m => Err(format!("{} mode has no undo history", m.label())),
            },
            C::SaveProject { path, save_as } => self.cmd_save_project(path.as_deref(), *save_as),
            C::OpenProject { path } => self.cmd_open_project(path.as_deref()),
            C::NewProject => self.cmd_new_project(),
            C::SavePsd { path } => self.cmd_export(workspace::SaveTarget::Psd, path.as_deref()),
            C::ExportPng { path } => self.cmd_export(workspace::SaveTarget::Png, path.as_deref()),
            C::OpenFile { path } => {
                self.mode = Mode::Paint;
                self.paint.open(path)
            }
            C::VideoGenerate { prompt, duration } => {
                self.show_assistant = true;
                self.dock_tab = 1;
                let cost = self.video.video_cost(*duration);
                self.cap_allows(cost, self.video.provider.is_some())?;
                self.video.generate(prompt, *duration)
            }
            C::VideoExtend { clip, prompt } => {
                self.show_assistant = true;
                self.dock_tab = 1;
                let cost = self.video.extend_cost();
                self.cap_allows(cost, self.video.provider.is_some())?;
                self.video.extend(*clip, prompt)
            }
            C::GenerateImage { prompt } => {
                self.show_assistant = true;
                self.dock_tab = 1;
                let cost = self.video.image_cost();
                self.cap_allows(cost, self.video.imager.is_some())?;
                self.video.generate_image(prompt)
            }
            C::VideoStitch { clips } => {
                self.dock_tab = 1;
                self.video.stitch(clips.clone())
            }
            C::RunReader { .. } | C::ResetReader => {
                self.mode = Mode::Stream;
                self.stream.apply(cmd).unwrap_or(Err("unsupported".into()))
            }
            C::SetStreaming { live } => {
                if *live {
                    self.output.start_live()
                } else {
                    self.output.stop_live()
                }
            }
            C::SetRecording { recording } => {
                if *recording {
                    self.output.start_record(None)
                } else {
                    self.output.stop_record()
                }
            }
            C::RetryStream => self.output.retry(),
            C::SetOutputSource { source, device } => {
                let busy = self.output.is_live() || self.output.is_recording();
                let dev = device.as_deref().map(str::trim);
                match output::Source::parse(source) {
                    Some(s)
                        if busy
                            && (s != self.output.cfg.source
                                || dev.is_some_and(|d| d != self.output.cfg.camera_device)) =>
                    {
                        Err("stop recording and streaming before switching the source".into())
                    }
                    Some(s) if dev.is_some() && s != output::Source::Camera => {
                        Err("only the camera source takes a device".into())
                    }
                    Some(s) => {
                        self.output.cfg.source = s;
                        if let Some(d) = dev {
                            self.output.cfg.camera_device = d.to_string();
                        }
                        let what = match (s, self.output.cfg.camera_device.as_str()) {
                            (output::Source::Camera, "") => "Camera (first camera found)".into(),
                            (output::Source::Camera, d) => format!("Camera ({d})"),
                            _ => s.label().to_string(),
                        };
                        self.output
                            .save_config()
                            .map(|_| format!("output source: {what}"))
                    }
                    None => Err(format!(
                        "unknown source '{}' (canvas, screen, camera)",
                        source
                    )),
                }
            }
            C::SetStreamTarget { url } => {
                let u = url.trim();
                if !u.is_empty() && !(u.starts_with("rtmp://") || u.starts_with("rtmps://")) {
                    Err(format!("'{}' is not an rtmp:// or rtmps:// URL", u))
                } else {
                    self.output.cfg.rtmp_url = u.to_string();
                    self.output.save_config().map(|_| {
                        if u.is_empty() {
                            "RTMP URL cleared".to_string()
                        } else {
                            format!("RTMP URL set to {}", u)
                        }
                    })
                }
            }
            C::SetStreamKey { key } => self.output.set_key(key, true),
            C::SceneAdd { .. }
            | C::SceneSelect { .. }
            | C::SceneGizmo { .. }
            | C::SceneTransform { .. }
            | C::SceneExtrude { .. }
            | C::SceneBevel { .. }
            | C::SceneDelete { .. }
            | C::SceneEdgeLoop { .. }
            | C::SceneMultiCut { .. }
            | C::SceneBridge { .. }
            | C::SceneBoolean { .. }
            | C::SceneQuadDraw { .. }
            | C::SceneShading { .. } => {
                self.mode = Mode::Scene;
                self.scene
                    .apply(cmd, &mut self.toasts)
                    .unwrap_or(Err("unsupported".into()))
            }
            C::MaterialAssign { object, material } => {
                self.mode = Mode::Scene;
                match self.scene.check_mesh_target(object.as_deref()) {
                    Ok(obj) => match self.materials.lib.resolve(material) {
                        Ok(name) => {
                            self.scene.surfaces = self
                                .materials
                                .lib
                                .profiles
                                .iter()
                                .map(|(k, p)| (k.clone(), materials::surface_of(&p.pbr)))
                                .collect();
                            self.scene.assign_material(&obj, &name)
                        }
                        Err(e) => Err(e),
                    },
                    Err(e) => Err(e),
                }
            }
            C::ArenaPlay { playing } => {
                self.mode = Mode::Scene;
                let want = playing.unwrap_or(!self.scene.arena_playing);
                if want && self.scene.arena.is_none() {
                    self.arena_build()?;
                }
                self.scene.arena_playing = want && self.scene.arena.is_some();
                Ok(if self.scene.arena_playing {
                    format!("arena playing ({})", self.scene.arena_env.name)
                } else {
                    "arena paused".into()
                })
            }
            C::ArenaStep { steps } => {
                self.mode = Mode::Scene;
                let n = steps.unwrap_or(4).clamp(1, 240 * 600);
                if self.scene.arena.is_none() {
                    self.arena_build()?;
                }
                self.scene.arena_playing = false;
                if n as usize > obs_rust_bitstream::arena::MAX_CATCH_UP {
                    // more than 1 s of simulation: run it off the UI thread
                    // in 1 s slices, publishing each one
                    let a = self.scene.arena.clone().ok_or("no arena")?;
                    let t0 = a.time;
                    let mut w = obs_rust_bitstream::sim_worker::SimWorker::spawn("arena", a);
                    w.submit(move |a: &mut obs_rust_bitstream::arena::Arena, c| {
                        let slice = obs_rust_bitstream::arena::MAX_CATCH_UP as u32;
                        let mut done = 0;
                        while done < n && !c.cancelled() {
                            for _ in 0..slice.min(n - done) {
                                a.step();
                            }
                            done += slice.min(n - done);
                            c.publish(a, done as u64, n as u64);
                        }
                    });
                    self.arena_run = Some(w);
                    return Ok(format!(
                        "stepping {} × 1/240 s in the background (t = {:.3} → {:.3} s); the arena panel shows progress",
                        n,
                        t0,
                        t0 + n as f64 / 240.0
                    ));
                }
                if let Some(a) = &mut self.scene.arena {
                    for _ in 0..n {
                        a.step();
                    }
                }
                self.arena_write_back();
                Ok(format!(
                    "stepped {} × 1/240 s (t = {:.3} s)",
                    n,
                    self.scene.arena.as_ref().map(|a| a.time).unwrap_or(0.0)
                ))
            }
            C::ArenaReset => {
                self.mode = Mode::Scene;
                self.scene.arena_playing = false;
                self.scene.arena = None;
                match self.scene.arena_start.take() {
                    Some(objs) => {
                        self.scene.objs = objs;
                        Ok("arena reset".into())
                    }
                    None => Ok("arena already reset".into()),
                }
            }
            C::ArenaEnv {
                preset,
                gravity,
                air_density,
                temperature_c,
                pressure_kpa,
                wind,
            } => {
                self.mode = Mode::Scene;
                let mut e = match preset {
                    Some(k) => obs_rust_bitstream::arena::Environment::preset(k).ok_or_else(|| format!("unknown environment '{}' (earth, moon, mars, vacuum, space, custom)", k))?,
                    None => self.scene.arena_env.clone(),
                };
                let custom = gravity.is_some()
                    || air_density.is_some()
                    || temperature_c.is_some()
                    || pressure_kpa.is_some()
                    || wind.is_some();
                if let Some(g) = gravity {
                    e.gravity = g.clamp(0.0, 1000.0);
                }
                if let Some(r) = air_density {
                    e.air_density = r.clamp(0.0, 100.0);
                }
                if let Some(t) = temperature_c {
                    e.temperature = (t + 273.15).max(0.0);
                }
                if let Some(pk) = pressure_kpa {
                    e.pressure = (*pk as f64 * 1000.0).max(0.0);
                }
                if let Some(w) = wind {
                    e.wind = *w;
                }
                if custom && preset.is_none() && !e.name.ends_with("(custom)") {
                    e.name = format!("{} (custom)", e.name);
                }
                self.scene.arena_env = e.clone();
                if let Some(a) = &mut self.scene.arena {
                    a.env = e.clone();
                }
                Ok(format!(
                    "environment {}: g {:.3} m/s², air {:.3} kg/m³, {:.1} kPa, {:.1} °C",
                    e.name,
                    e.gravity,
                    e.air_density,
                    e.pressure / 1000.0,
                    e.temperature - 273.15
                ))
            }
            C::ArenaBody {
                object,
                temperature_c,
                heater_w,
                fixed,
                velocity,
            } => {
                self.mode = Mode::Scene;
                let name = self.scene.check_mesh_target(object.as_deref())?;
                let pr = self.scene.phys.entry(name.clone()).or_default();
                if let Some(t) = temperature_c {
                    pr.temp = Some((t + 273.15).max(0.0));
                }
                if let Some(h) = heater_w {
                    pr.heater = *h;
                }
                if let Some(f) = fixed {
                    pr.fixed = *f;
                }
                if let Some(v) = velocity {
                    pr.vel = *v;
                }
                let pr = *pr;
                let pressure = self.scene.arena_env.pressure;
                if let Some(a) = self.scene.arena.as_mut() {
                    // an outside intervention: the energy books restart from here
                    a.ledger = Default::default();
                }
                if let Some(b) = self.scene.arena.as_mut().and_then(|a| a.body_mut(&name)) {
                    if let Some(t) = pr.temp.filter(|_| temperature_c.is_some()) {
                        b.temp = t as f64;
                        b.phase = b.env.phase_at(t, pressure);
                        b.latent = 0.0;
                    }
                    b.heater = pr.heater;
                    b.fixed = pr.fixed;
                    if velocity.is_some() {
                        b.vel = pr.vel;
                    }
                }
                Ok(format!(
                    "{}: start {:.1} °C, heater {:.0} W{}",
                    name,
                    pr.temp
                        .map(|t| t - 273.15)
                        .unwrap_or(self.scene.arena_env.temperature - 273.15),
                    pr.heater,
                    if pr.fixed { ", pinned" } else { "" }
                ))
            }
            C::LabTab { .. }
            | C::ChemReact { .. }
            | C::CircuitRun { .. }
            | C::CpuRun { .. }
            | C::CpuHeatSweep { .. }
            | C::RelSpeed { .. }
            | C::NbodyPreset { .. }
            | C::NbodyPlay { .. }
            | C::NbodyMeasurePrecession => {
                self.mode = Mode::Materials;
                let die = match cmd {
                    C::CpuRun {
                        die_object: Some(o),
                        ..
                    } if !o.is_empty() => self.arena_temp(o),
                    _ => None,
                };
                self.lab
                    .apply(cmd, die)
                    .unwrap_or(Err("unsupported".into()))
            }
            C::MaterialsSelect { .. }
            | C::MaterialsSearch { .. }
            | C::MaterialsAdd { .. }
            | C::MaterialsRemove { .. }
            | C::MaterialsClear
            | C::MaterialsBuild { .. }
            | C::MaterialsBuildAtom { .. }
            | C::MaterialsPreset { .. }
            | C::MaterialsSave { .. } => {
                self.mode = Mode::Materials;
                self.lab.tab = 0;
                self.materials
                    .apply(cmd)
                    .unwrap_or(Err("unsupported".into()))
            }
            C::TimelinePlay { .. }
            | C::TimelineScrub { .. }
            | C::SetKeyframe
            | C::DeleteKeyframe => match self.mode {
                Mode::Scene => self.scene.apply(cmd, &mut self.toasts),
                Mode::Animate => self.anim.apply(cmd, &mut self.toasts),
                Mode::Composite => self.comp.apply(cmd),
                m => Some(Err(format!(
                    "{} mode has no timeline; switch to 3D, Animate or Composite first",
                    m.label()
                ))),
            }
            .unwrap_or(Err("unsupported".into())),
            C::SelectLayer { .. }
            | C::SetLayerOpacity { .. }
            | C::SetLayerBlend { .. }
            | C::SetLayerVisibility { .. }
                if self.mode == Mode::Composite =>
            {
                self.comp.apply(cmd).unwrap_or(Err("unsupported".into()))
            }
            _ => match self.paint.apply(cmd) {
                Some(r) => r,
                None => Err(format!("{} isn't handled", cmd.name())),
            },
        };
        r
    }

    fn finish(
        &mut self,
        cmd: &AppCommand,
        origin: Origin,
        r: Result<String, String>,
    ) -> Result<String, String> {
        if origin == Origin::User {
            match &r {
                Ok(t) => {
                    match cmd {
                        AppCommand::SelectTool { tool } => {
                            self.assistant.prefs.tool_used("Paint", tool)
                        }
                        AppCommand::SceneGizmo { mode } => {
                            self.assistant.prefs.tool_used("3D", mode)
                        }
                        AppCommand::SetBrush { size: Some(s), .. } => self
                            .assistant
                            .prefs
                            .param_chosen("paint.brush_size", (*s / 200.0).clamp(0.0, 1.0)),
                        AppCommand::SetBrush {
                            hardness: Some(h), ..
                        } => self
                            .assistant
                            .prefs
                            .param_chosen("paint.brush_hardness", h.clamp(0.0, 1.0)),
                        _ => {}
                    }
                    if matches!(
                        cmd,
                        AppCommand::SavePsd { .. }
                            | AppCommand::ExportPng { .. }
                            | AppCommand::OpenFile { .. }
                            | AppCommand::VideoGenerate { .. }
                            | AppCommand::GenerateImage { .. }
                            | AppCommand::VideoExtend { .. }
                            | AppCommand::VideoStitch { .. }
                            | AppCommand::SetStreaming { .. }
                            | AppCommand::SetRecording { .. }
                            | AppCommand::RetryStream
                            | AppCommand::SetStreamKey { .. }
                            | AppCommand::SetStreamTarget { .. }
                            | AppCommand::SetOutputSource { .. }
                            | AppCommand::Crop { .. }
                            | AppCommand::ApplyFilter { .. }
                    ) {
                        let mut s = t.clone();
                        if let Some(c) = s.get_mut(0..1) {
                            c.make_ascii_uppercase();
                        }
                        self.toasts.push(ToastKind::Success, &s);
                    }
                }
                Err(e) => self.toasts.push(ToastKind::Error, e),
            }
        }
        self.log.push((origin, cmd.name(), r.clone()));
        if self.log.len() > 200 {
            let _ = self.log.remove(0);
        }
        r
    }

    /// Run everything the UI queued this frame.
    pub fn process_bus(&mut self) {
        let cmds = std::mem::take(&mut self.bus);
        if cmds.is_empty() {
            return;
        }
        for c in cmds {
            let _ = self.dispatch(&c, Origin::User);
        }
        // Panels drawn earlier this frame (status bar, learning counter) are now stale.
        self.ctx.request_repaint();
    }

    /// Execute Seraphina's tool calls through the Triad gate.
    pub fn run_tool_calls(&mut self, calls: Vec<llm::ToolCall>) {
        let batch = self.assistant.alloc_batch();
        let snap = self.snapshot();
        let mut lines = Vec::new();
        let mut results = Vec::new();
        for c in calls {
            // geometric wheel: the call must parse into a known command
            let cmd = match parse_tool_call(&c.name, &c.arguments) {
                Ok(cmd) => cmd,
                Err(e) => {
                    lines.push(ActionLine {
                        status: LineStatus::Rejected,
                        text: format!("Seraphina tried {}: {}", c.name, e),
                        cmd: None,
                        ran: false,
                    });
                    results.push((c.id, format!("error: {}", e)));
                    continue;
                }
            };
            // mercy wheel: irreversible or external actions wait for the user
            if cmd.needs_user() {
                lines.push(ActionLine {
                    status: LineStatus::NeedsUser,
                    text: format!(
                        "Seraphina wants to run {} ({})",
                        cmd.name(),
                        c.arguments.chars().take(80).collect::<String>()
                    ),
                    cmd: Some(cmd),
                    ran: false,
                });
                results.push((
                    c.id,
                    "blocked: needs the user's approval; they now see a Run button".into(),
                ));
                continue;
            }
            // verification wheel: dispatch must succeed against real state
            match self.dispatch(&cmd, Origin::Seraphina) {
                Ok(t) => {
                    lines.push(ActionLine {
                        status: LineStatus::Done,
                        text: format!("Seraphina: {}", t),
                        cmd: Some(cmd),
                        ran: true,
                    });
                    results.push((c.id, format!("ok: {}", t)));
                }
                Err(e) => {
                    lines.push(ActionLine {
                        status: LineStatus::Rejected,
                        text: format!("Seraphina tried {}: {}", cmd.name(), e),
                        cmd: Some(cmd),
                        ran: false,
                    });
                    results.push((c.id, format!("error: {}", e)));
                }
            }
        }
        if lines.iter().any(|l| {
            l.status == LineStatus::Done && l.cmd.as_ref().map(|c| c.mutates()).unwrap_or(false)
        }) {
            self.snapshots.insert(batch, snap);
            // batch ids increase, so the smallest keys are the oldest
            while self.snapshots.len() > MAX_AI_SNAPSHOTS {
                let oldest = *self.snapshots.keys().min().unwrap();
                self.snapshots.remove(&oldest);
            }
        }
        let ctx = self.app_context();
        self.assistant.submit_results(batch, lines, results, &ctx);
    }

    pub fn undo_batch(&mut self, batch: u64) -> bool {
        let found = match self.snapshots.remove(&batch) {
            Some(s) => {
                self.restore(s);
                self.toasts
                    .push(ToastKind::Info, "Undid Seraphina's actions.");
                true
            }
            None => {
                self.toasts.push(
                    ToastKind::Info,
                    &format!(
                        "That Seraphina action is too old to undo (the last {} are kept).",
                        MAX_AI_SNAPSHOTS
                    ),
                );
                false
            }
        };
        if found {
            self.assistant.mark_undone(batch);
        }
        found
    }

    fn handle_panel_requests(&mut self) {
        for r in std::mem::take(&mut self.assistant.requests) {
            match r {
                PanelRequest::UndoBatch(b) => {
                    self.undo_batch(b);
                }
                PanelRequest::Run { cmd, batch, line } => {
                    let res = self.dispatch(&cmd, Origin::User);
                    self.assistant.mark_ran(batch, line, res);
                }
            }
        }
    }

    pub fn pump_assistant(&mut self) {
        self.assistant.blocked = self.meter.check(0.0).err();
        self.assistant.share_app_state = self.app_settings.share_app_state;
        self.assistant.share_memory = self.app_settings.share_memory;
        if self.assistant.want_send
            && !self.app_settings.ai_consent
            && self.assistant.would_leave_device()
        {
            // B3: ask first; the message stays in the input box
            self.assistant.want_send = false;
            self.consent_open = true;
        }
        if self.assistant.want_send {
            let ctx = self.app_context();
            let w = self.workspace_ctx();
            self.assistant.start(&ctx, &w);
        }
        self.assistant.poll();
        self.pump_usage();
        if let Some(calls) = self.assistant.take_tool_calls() {
            self.run_tool_calls(calls);
        }
        self.handle_panel_requests();
    }

    /// B3: the answer to the first-send privacy prompt.
    pub fn consent_choice(&mut self, c: Consent) {
        self.consent_open = false;
        match c {
            Consent::All | Consent::MessagesOnly => {
                let all = c == Consent::All;
                self.app_settings.ai_consent = true;
                self.app_settings.share_app_state = all;
                self.app_settings.share_memory = all;
                self.save_settings();
                self.assistant.want_send = true;
            }
            Consent::No => self.assistant.note(
                "Nothing was sent. Your message is still in the box; Seraphina will ask again when you send.",
            ),
        }
    }

    fn save_settings(&mut self) {
        if cfg!(test) {
            return; // never touch the real settings file from tests
        }
        if let Err(e) = self.app_settings.save() {
            self.toasts
                .push(ToastKind::Error, &format!("Couldn't save settings: {}", e));
        }
    }

    fn consent_modal(&mut self, ctx: &egui::Context) {
        if !self.consent_open {
            return;
        }
        let p = self.pal;
        let to = self.assistant.backend_label();
        let mut choice = None;
        let r = egui::Modal::new(egui::Id::new("ai-consent"))
            .frame(
                egui::Frame::NONE
                    .fill(p.panel)
                    .stroke(egui::Stroke::new(1.0_f32, p.line))
                    .corner_radius(egui::CornerRadius::same(theme::R_LG as u8))
                    .inner_margin(egui::Margin::same(SP_LG as i8)),
            )
            .show(ctx, |ui| {
                ui.set_width(500.0);
                ui.label(egui::RichText::new("Send your chat outside this PC?").size(16.0).strong().color(p.text));
                ui.add_space(SP_SM);
                ui.add(egui::Label::new(egui::RichText::new(format!("Seraphina's chat runs on {}, outside this PC. Each message you send goes there together with:", to)).color(p.text)).wrap());
                ui.add_space(SP_SM);
                for line in [
                    "• this conversation (your messages, Seraphina's replies and the results of her actions)",
                    "• app state: the current mode, layer and object names, and panel layout",
                    "• your most recent Seraphina.AGI memory notes, if the CLI is installed",
                ] {
                    ui.add(egui::Label::new(egui::RichText::new(line).color(p.muted)).wrap());
                }
                ui.add_space(SP_SM);
                ui.add(egui::Label::new(theme::muted(&p, "Pixels, files and your API key are not sent with chat. The service bills your key for every message; set a spend cap and change these choices any time in Settings ▸ Privacy.")).wrap());
                ui.add_space(SP_MD);
                ui.horizontal(|ui| {
                    if ui.add(theme::primary_button(&p, "Allow and send")).clicked() {
                        choice = Some(Consent::All);
                    }
                    if ui.button("Send only my messages").on_hover_text("Leave out app state and memory notes").clicked() {
                        choice = Some(Consent::MessagesOnly);
                    }
                    if ui.button("Don't send").clicked() {
                        choice = Some(Consent::No);
                    }
                });
            });
        if r.should_close() && choice.is_none() {
            choice = Some(Consent::No);
        }
        if let Some(c) = choice {
            self.consent_choice(c);
        }
    }

    /// Settings ▸ Privacy (B3).
    fn privacy_ui(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        ui.label(theme::section(p, "Privacy"));
        let s = &mut self.app_settings;
        let before = (s.ai_consent, s.share_app_state, s.share_memory);
        theme::kv(
            ui,
            p,
            "Chat to xAI",
            if s.ai_consent {
                "allowed"
            } else {
                "asks before the first message"
            },
        );
        ui.checkbox(
            &mut s.share_app_state,
            "Send app state (mode, layer and object names)",
        )
        .on_hover_text(
            "Seraphina works better with it; without it she has to ask what is in your project.",
        );
        ui.checkbox(
            &mut s.share_memory,
            "Send recent Seraphina.AGI memory notes",
        );
        if s.ai_consent && ui.small_button("Ask me again before sending").clicked() {
            s.ai_consent = false;
        }
        if before != (s.ai_consent, s.share_app_state, s.share_memory) {
            self.save_settings();
        }
    }

    /// Count what the workers spent and toast a round the cap held back.
    fn pump_usage(&mut self) {
        self.video.tick_spent();
        for u in std::mem::take(&mut self.assistant.spent)
            .into_iter()
            .chain(std::mem::take(&mut self.video.spent))
        {
            self.meter.record(&u);
        }
        if let Some(why) = self.assistant.cap_hit.take() {
            self.toasts.push(ToastKind::Warn, &why);
        }
    }

    /// Spend-cap check before a billed Imagine job.
    fn cap_allows(&self, usd: f64, configured: bool) -> Result<(), String> {
        if configured {
            self.meter.check(usd)
        } else {
            Ok(())
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // file shortcuts work even while a text field has focus
        if self.project.prompt.is_none() && self.project.dialog.is_none() {
            use egui::{KeyboardShortcut, Modifiers};
            let take = |m: Modifiers, k: Key| {
                ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(m, k)))
            };
            let save_as = take(Modifiers::COMMAND | Modifiers::SHIFT, Key::S);
            if save_as || take(Modifiers::COMMAND, Key::S) {
                self.bus.push(AppCommand::SaveProject {
                    path: None,
                    save_as,
                });
            }
            if take(Modifiers::COMMAND, Key::O) {
                self.bus.push(AppCommand::OpenProject { path: None });
            }
            if take(Modifiers::COMMAND, Key::N) {
                self.bus.push(AppCommand::NewProject);
            }
        }
        if ctx.wants_keyboard_input() {
            return;
        }
        let (cmd, shift) = ctx.input(|i| (i.modifiers.command, i.modifiers.shift));
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        let mut out = Vec::new();
        for (k, m) in [
            (Key::Num1, Mode::Paint),
            (Key::Num2, Mode::Composite),
            (Key::Num3, Mode::Scene),
            (Key::Num4, Mode::Animate),
            (Key::Num5, Mode::Stream),
            (Key::Num6, Mode::Materials),
        ] {
            if cmd && pressed(k) {
                out.push(AppCommand::SwitchMode {
                    mode: m.label().into(),
                });
            }
        }
        if pressed(Key::F1) {
            self.show_shortcuts = !self.show_shortcuts;
        }
        if cmd && pressed(Key::Period) {
            out.push(AppCommand::TogglePanel {
                panel: "assistant".into(),
                visible: None,
            });
        }
        if pressed(Key::Escape) && (self.show_shortcuts || self.show_settings || self.show_account)
        {
            self.show_shortcuts = false;
            self.show_settings = false;
            self.show_account = false;
        }
        if cmd && pressed(Key::Z) {
            out.push(if shift {
                AppCommand::Redo
            } else {
                AppCommand::Undo
            });
        } else if cmd && pressed(Key::Y) {
            out.push(AppCommand::Redo);
        }
        if self.mode.has_timeline() && !cmd {
            if pressed(Key::Space) {
                out.push(AppCommand::TimelinePlay { playing: None });
            }
            if pressed(Key::K) {
                out.push(AppCommand::SetKeyframe);
            }
        }
        match self.mode {
            Mode::Stream => {
                if pressed(Key::Enter) || pressed(Key::ArrowRight) {
                    out.push(AppCommand::RunReader {
                        width: None,
                        reads: Some(1),
                        wrap: None,
                    });
                }
                if pressed(Key::R) && !cmd {
                    out.push(AppCommand::ResetReader);
                }
            }
            Mode::Paint => {
                if cmd && pressed(Key::Num0) {
                    out.push(AppCommand::Zoom {
                        percent: None,
                        fit: Some(true),
                    });
                } else if cmd && (pressed(Key::Plus) || pressed(Key::Equals)) {
                    self.paint.zoom_by(true);
                } else if cmd && pressed(Key::Minus) {
                    self.paint.zoom_by(false);
                } else if cmd && pressed(Key::D) {
                    out.push(AppCommand::Deselect);
                } else if cmd && pressed(Key::A) {
                    out.push(AppCommand::SelectAll);
                } else if !cmd {
                    for t in paint::Tool::ALL {
                        if t == paint::Tool::Burn || !pressed(t.key()) {
                            continue;
                        }
                        let t = if t == paint::Tool::Dodge && shift {
                            paint::Tool::Burn
                        } else {
                            t
                        };
                        if self.paint.tool != t {
                            out.push(AppCommand::SelectTool {
                                tool: t.id().into(),
                            });
                        }
                    }
                    if pressed(Key::Enter) {
                        if let Some(c) = self.paint.crop {
                            out.push(AppCommand::Crop {
                                x: c.x as f32,
                                y: c.y as f32,
                                w: c.w as f32,
                                h: c.h as f32,
                            });
                        } else if !self.paint.pen.is_empty() {
                            out.push(AppCommand::PenStroke);
                        }
                    }
                    if pressed(Key::Escape) {
                        self.paint.crop = None;
                    }
                }
            }
            Mode::Animate => {
                if !cmd {
                    let c = self.anim.current();
                    if pressed(Key::Comma) {
                        out.push(AppCommand::TimelineScrub {
                            time: (c.saturating_sub(1) as f32 + 0.01) / self.anim.fps,
                        });
                    }
                    if pressed(Key::Period) {
                        out.push(AppCommand::TimelineScrub {
                            time: ((c + 1).min(self.anim.frames.len() - 1) as f32 + 0.01)
                                / self.anim.fps,
                        });
                    }
                    if pressed(Key::B) {
                        self.anim.erase = false;
                    }
                    if pressed(Key::E) {
                        self.anim.erase = true;
                    }
                }
            }
            Mode::Scene => {
                if pressed(Key::Delete) || pressed(Key::Backspace) {
                    out.push(AppCommand::SceneDelete { object: None });
                }
                if !cmd {
                    for (k, g) in [
                        (Key::Q, "select"),
                        (Key::W, "move"),
                        (Key::E, "rotate"),
                        (Key::R, "scale"),
                    ] {
                        if pressed(k) {
                            out.push(AppCommand::SceneGizmo { mode: g.into() });
                        }
                    }
                    for (k, s) in [
                        (Key::Num4, "wireframe"),
                        (Key::Num5, "shaded"),
                        (Key::Num6, "shaded_wire"),
                    ] {
                        if pressed(k) {
                            out.push(AppCommand::SceneShading { mode: s.into() });
                        }
                    }
                }
            }
            Mode::Composite | Mode::Materials => {}
        }
        self.bus.extend(out);
    }

    // ---- chrome -----------------------------------------------------------

    fn header(&mut self, ctx: &egui::Context) {
        let p = self.pal;
        egui::TopBottomPanel::top("header")
            .exact_height(58.0)
            .frame(
                egui::Frame::NONE
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(SP_MD as i8, 0)),
            )
            .show(ctx, |ui| {
                let full = ui.max_rect();
                if !self.native_frame {
                    let bg = ui.interact(
                        full,
                        egui::Id::new("titlebar"),
                        egui::Sense::click_and_drag(),
                    );
                    if bg.drag_started() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    }
                    if bg.double_clicked() {
                        let m = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!m));
                    }
                }
                ui.horizontal_centered(|ui| {
                    let (gr, _) =
                        ui.allocate_exact_size(Vec2::new(34.0, 38.0), egui::Sense::hover());
                    theme::gem(ui.painter(), gr, &p);
                    ui.add_space(SP_XS);
                    ui.vertical(|ui| {
                        ui.add_space(10.0);
                        ui.label(
                            egui::RichText::new("Seraphina Studio")
                                .size(17.0)
                                .strong()
                                .color(p.accent),
                        );
                        let dirty = self.is_dirty();
                        ui.label(
                            egui::RichText::new(self.title_label())
                                .size(10.5)
                                .color(if dirty { p.text } else { p.muted }),
                        )
                        .on_hover_text(if dirty {
                            "Unsaved changes  (Ctrl+S to save)"
                        } else {
                            "All changes saved"
                        });
                    });
                    ui.add_space(SP_LG);
                    let tab_w = ((full.width() - 660.0) / 5.0).clamp(96.0, 150.0);
                    for (i, m) in Mode::ALL.iter().enumerate() {
                        let (badge, tip) = m.style();
                        let r =
                            theme::mode_tab(ui, &p, m.icon(), m.label(), self.mode == *m, tab_w)
                                .on_hover_text(format!("{} · {}  (Ctrl+{})", tip, badge, i + 1));
                        if r.clicked() {
                            self.bus.push(AppCommand::SwitchMode {
                                mode: m.label().into(),
                            });
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 2.0;
                        if !self.native_frame {
                            if theme::icon_button_sized(ui, &p, ic::X, "Close", false, 30.0, 16.0)
                                .clicked()
                            {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                            let max = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                            if theme::icon_button_sized(
                                ui,
                                &p,
                                if max { ic::CORNERS_IN } else { ic::SQUARE },
                                if max { "Restore" } else { "Maximise" },
                                false,
                                30.0,
                                15.0,
                            )
                            .clicked()
                            {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                            }
                            if theme::icon_button_sized(
                                ui,
                                &p,
                                ic::MINUS,
                                "Minimise",
                                false,
                                30.0,
                                16.0,
                            )
                            .clicked()
                            {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                            }
                            theme::vdivider(ui, &p, 26.0);
                        }
                        if theme::icon_button_sized(
                            ui,
                            &p,
                            ic::USER_CIRCLE,
                            "Account (local profile)",
                            self.show_account,
                            30.0,
                            19.0,
                        )
                        .clicked()
                        {
                            self.show_account = !self.show_account;
                        }
                        if theme::icon_button_sized(
                            ui,
                            &p,
                            ic::GEAR_SIX,
                            "Settings",
                            self.show_settings,
                            30.0,
                            17.0,
                        )
                        .clicked()
                        {
                            self.show_settings = !self.show_settings;
                        }
                        if theme::icon_button_sized(
                            ui,
                            &p,
                            ic::KEYBOARD,
                            "Keyboard shortcuts  (F1)",
                            self.show_shortcuts,
                            30.0,
                            17.0,
                        )
                        .clicked()
                        {
                            self.show_shortcuts = !self.show_shortcuts;
                        }
                        let (ti, tt) = if self.dark {
                            (ic::SUN, "Light theme")
                        } else {
                            (ic::MOON, "Dark theme")
                        };
                        if theme::icon_button_sized(ui, &p, ti, tt, false, 30.0, 17.0).clicked() {
                            self.bus.push(AppCommand::SetTheme { dark: !self.dark });
                        }
                        let vg = self.show_assistant && self.dock_tab == 1;
                        if theme::icon_button_sized(
                            ui,
                            &p,
                            ic::FILM_REEL,
                            "Grok Imagine video",
                            vg,
                            30.0,
                            17.0,
                        )
                        .clicked()
                        {
                            self.show_assistant = !vg;
                            self.dock_tab = 1;
                        }
                        let sr = self.show_assistant && self.dock_tab == 0;
                        if theme::icon_button_sized(
                            ui,
                            &p,
                            ic::SPARKLE,
                            "Seraphina  (Ctrl+.)",
                            sr,
                            30.0,
                            17.0,
                        )
                        .clicked()
                        {
                            self.show_assistant = !sr;
                            self.dock_tab = 0;
                        }
                    });
                });
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        let p = self.pal;
        egui::TopBottomPanel::bottom("status")
            .exact_height(26.0)
            .frame(
                egui::Frame::NONE
                    .fill(p.panel)
                    .inner_margin(egui::Margin::symmetric(SP_MD as i8, 3)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    let item = |ui: &mut egui::Ui, k: &str, v: String| {
                        ui.label(egui::RichText::new(k).size(11.5).color(p.muted));
                        ui.label(egui::RichText::new(v).size(11.5).color(p.text));
                        theme::vdivider(ui, &p, 14.0);
                    };
                    item(ui, "Tool:", self.current_tool());
                    item(ui, "Zoom:", self.zoom_label());
                    item(ui, "Resolution:", self.resolution_label());
                    item(ui, "Document:", self.document_label());
                    if self.mode == Mode::Paint {
                        if let Some((x, y)) = self.paint.cursor_px {
                            item(ui, "Cursor:", format!("{}, {}", x, y));
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.meter.session.calls > 0 || self.meter.cap_usd > 0.0 {
                            let capped = self.meter.check(0.0).is_err();
                            let col = if capped { p.warn } else { p.muted };
                            let r = ui
                                .add(
                                    egui::Label::new(
                                        egui::RichText::new(format!(
                                            "{} {}",
                                            ic::COINS,
                                            self.meter.label()
                                        ))
                                        .size(11.5)
                                        .color(col),
                                    )
                                    .sense(egui::Sense::click()),
                                )
                                .on_hover_text(self.meter.tooltip());
                            if r.clicked() {
                                self.show_settings = true;
                            }
                            theme::vdivider(ui, &p, 14.0);
                        }
                        let st = self.output.status();
                        let live = self.output.is_live();
                        let rec = self.output.is_recording();
                        if theme::icon_button(ui, &p, ic::GEAR, "Output settings", false).clicked()
                        {
                            self.show_settings = true;
                        }
                        if (live || rec)
                            && theme::icon_button(
                                ui,
                                &p,
                                ic::STOP,
                                "Stop streaming and recording",
                                false,
                            )
                            .clicked()
                        {
                            self.bus.push(AppCommand::SetStreaming { live: false });
                            self.bus.push(AppCommand::SetRecording { recording: false });
                        }
                        if live || rec {
                            if let Some(e) = &st.error {
                                let short: String = e.chars().take(48).collect();
                                ui.label(
                                    egui::RichText::new(format!("{} {}", ic::WARNING, short))
                                        .size(11.5)
                                        .color(p.danger),
                                )
                                .on_hover_text(e.as_str());
                            }
                            ui.label(
                                egui::RichText::new(format!(
                                    "{:.0} fps · {:.0} kb/s · {} dropped",
                                    st.fps, st.kbps, st.dropped
                                ))
                                .size(11.5)
                                .color(p.text),
                            );
                            theme::vdivider(ui, &p, 14.0);
                        }
                        let rec_t = match st.recording {
                            Some(t) => format!("REC {}", format_clock(t as u64)),
                            None => "REC off".into(),
                        };
                        let r = ui.add(
                            egui::Label::new(
                                egui::RichText::new(&rec_t).size(11.5).color(if rec {
                                    p.danger
                                } else {
                                    p.muted
                                }),
                            )
                            .sense(egui::Sense::click()),
                        );
                        if r.on_hover_text("Click to start / stop recording (ffmpeg → MP4)")
                            .clicked()
                        {
                            self.bus.push(AppCommand::SetRecording { recording: !rec });
                        }
                        let (dot, txt) = if let Some(r) = st.reconnect_in {
                            (p.warn, format!("Reconnecting in {:.0}s", r.ceil()))
                        } else if let Some(t) = st.live {
                            (p.success, format!("Live {}", format_clock(t as u64)))
                        } else {
                            (p.muted, "Offline".to_string())
                        };
                        let r = ui.add(
                            egui::Label::new(egui::RichText::new(&txt).size(11.5).color(if live {
                                dot
                            } else {
                                p.muted
                            }))
                            .sense(egui::Sense::click()),
                        );
                        if r.on_hover_text("Click to go live / end (ffmpeg → RTMP)")
                            .clicked()
                        {
                            self.bus.push(AppCommand::SetStreaming { live: !live });
                        }
                        let (dr, _) =
                            ui.allocate_exact_size(Vec2::splat(10.0), egui::Sense::hover());
                        ui.painter().circle_filled(dr.center(), 4.0, dot);
                        ui.label(egui::RichText::new("Streaming:").size(11.5).color(p.muted));
                        ui.label(
                            egui::RichText::new(ic::BROADCAST)
                                .size(13.0)
                                .color(p.accent),
                        );
                        theme::vdivider(ui, &p, 14.0);
                        ui.label(
                            egui::RichText::new(format!(
                                "Online learning: {} updates",
                                self.assistant.prefs.bank.updates
                            ))
                            .size(11.5)
                            .color(p.muted),
                        );
                    });
                });
            });
    }

    fn windows(&mut self, ctx: &egui::Context) {
        let p = self.pal;
        let mut open = self.show_shortcuts;
        egui::Window::new("Keyboard shortcuts")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let rows: &[(&str, &[(&str, &str)])] = &[
                    (
                        "Global",
                        &[
                            (
                                "Ctrl+1 … 6",
                                "Paint, Composite, 3D, Animate, Stream, Materials",
                            ),
                            ("Ctrl+.", "Toggle Seraphina"),
                            ("Ctrl+Z / Ctrl+Shift+Z", "Undo / redo"),
                            ("F1", "This sheet"),
                        ],
                    ),
                    (
                        "Paint",
                        &[
                            ("V M L W C", "Move, Marquee, Lasso, Quick Select, Crop"),
                            (
                                "J S B E G",
                                "Healing, Clone (Alt-click source), Brush, Eraser, Gradient",
                            ),
                            ("O / Shift+O", "Dodge / Burn"),
                            ("P T H Z I", "Pen, Type, Hand, Zoom, Eyedropper"),
                            (
                                "Enter / Esc",
                                "Apply crop or stroke the pen path / cancel crop",
                            ),
                            ("Ctrl+A / Ctrl+D", "Select all / deselect"),
                            ("Ctrl+0, Ctrl+= / −", "Fit, zoom"),
                        ],
                    ),
                    (
                        "3D",
                        &[
                            ("Q W E R", "Select, Move, Rotate, Scale"),
                            ("Alt-drag", "Orbit"),
                            ("Delete", "Delete object"),
                        ],
                    ),
                    (
                        "Timeline modes",
                        &[
                            ("Space", "Play / pause"),
                            ("K", "Set key"),
                            (", / .", "Previous / next frame (Animate)"),
                        ],
                    ),
                    (
                        "Stream",
                        &[("Enter / →", "Step one read"), ("R", "Reset reader")],
                    ),
                ];
                for (group, items) in rows {
                    ui.label(theme::section(&p, group));
                    egui::Grid::new(*group)
                        .num_columns(2)
                        .spacing([SP_LG, 4.0])
                        .show(ui, |ui| {
                            for (k, v) in *items {
                                ui.label(egui::RichText::new(*k).monospace().color(p.accent));
                                ui.label(egui::RichText::new(*v).color(p.text));
                                ui.end_row();
                            }
                        });
                    ui.add_space(SP_SM);
                }
            });
        self.show_shortcuts = open;

        let mut open = self.show_settings;
        egui::Window::new("Settings").open(&mut open).collapsible(false).resizable(false).default_width(420.0).anchor(egui::Align2::RIGHT_TOP, Vec2::new(-SP_LG, 64.0)).show(ctx, |ui| {
            let max_h = (ctx.screen_rect().height() - 150.0).max(200.0);
            egui::ScrollArea::vertical().max_height(max_h).show(ui, |ui| {
            ui.label(theme::section(&p, "Appearance"));
            if let Some(i) = theme::segmented(ui, &p, "theme-seg", &["Dark", "Light"], if self.dark { 0 } else { 1 }, 80.0) {
                self.bus.push(AppCommand::SetTheme { dark: i == 0 });
            }
            self.ui_scale_ui(ui, &p);
            ui.add_space(SP_SM);
            self.graphics_settings_ui(ui, &p);
            ui.add_space(SP_SM);
            ui.label(theme::section(&p, "Seraphina"));
            theme::kv(ui, &p, "Language model", self.assistant.backend_label());
            theme::kv(ui, &p, "Seraphina.AGI CLI", self.assistant.cli.as_ref().map(|c| c.version.clone()).unwrap_or_else(|| "not found".into()));
            theme::kv(ui, &p, "Learning updates", format!("{}", self.assistant.prefs.bank.updates));
            self.xai_key_ui(ui, &p);
            ui.add(egui::Label::new(theme::muted(&p, "Env: XAI_API_KEY / SERAPHINA_GROK_API_KEY override the saved key. Endpoint: SERAPHINA_BASE_URL / SERAPHINA_GROK_ENDPOINT. Model: SERAPHINA_MODEL / SERAPHINA_GROK_MODEL (default grok-4.7). Video: SERAPHINA_VIDEO_MODEL (grok-imagine-video-1.5) / SERAPHINA_VIDEO_RESOLUTION; image: SERAPHINA_IMAGE_MODEL. Restart after changing.")).wrap());
            ui.add_space(SP_SM);
            ui.label(theme::section(&p, "Grok Imagine"));
            theme::kv(ui, &p, "Video provider", if self.video.provider.is_some() { "configured" } else { "no key" });
            ui.add_space(SP_SM);
            self.privacy_ui(ui, &p);
            ui.add_space(SP_SM);
            self.spending_ui(ui, &p);
            ui.add_space(SP_SM);
            if self.output.settings_ui(ui, &p, &mut self.bus) {
                if let Err(e) = self.output.save_config() {
                    self.toasts.push(ToastKind::Error, &format!("Couldn't save output settings: {}", e));
                }
            }
            ui.add_space(SP_SM);
            ui.label(theme::section(&p, "Panels"));
            ui.horizontal_wrapped(|ui| {
                for (name, on) in [("tools", self.show_tools), ("inspector", self.show_inspector), ("timeline", self.show_timeline), ("shelf", self.show_shelf)] {
                    if ui.add(theme::toggle_button(&p, name, on)).clicked() {
                        self.bus.push(AppCommand::TogglePanel { panel: name.into(), visible: Some(!on) });
                    }
                }
            });
            });
        });
        self.show_settings = open;

        let mut open = self.show_account;
        egui::Window::new("Account")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .default_width(300.0)
            .anchor(egui::Align2::RIGHT_TOP, Vec2::new(-SP_LG, 64.0))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(
                        egui::RichText::new(ic::USER_CIRCLE)
                            .size(30.0)
                            .color(p.accent),
                    );
                    ui.vertical(|ui| {
                        ui.label(egui::RichText::new("Local profile").strong().color(p.text));
                        ui.label(theme::muted(
                            &p,
                            "Not signed in. Accounts aren't implemented.",
                        ));
                    });
                });
                ui.add_space(SP_SM);
                let path = self
                    .assistant
                    .prefs
                    .path
                    .as_ref()
                    .map(|x| x.display().to_string())
                    .unwrap_or_else(|| "in memory".into());
                ui.add(
                    egui::Label::new(theme::muted(
                        &p,
                        &format!("Preferences stay on this machine: {}", path),
                    ))
                    .wrap(),
                );
            });
        self.show_account = open;
    }

    fn edge_resize(&self, ctx: &egui::Context) {
        if self.native_frame {
            return;
        }
        let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
            return;
        };
        let r = ctx.screen_rect();
        let m = 5.0;
        let (l, rt, t, b) = (
            pos.x - r.left() < m,
            r.right() - pos.x < m,
            pos.y - r.top() < m,
            r.bottom() - pos.y < m,
        );
        use egui::viewport::ResizeDirection as D;
        let dir = match (l, rt, t, b) {
            (true, _, true, _) => Some(D::NorthWest),
            (_, true, true, _) => Some(D::NorthEast),
            (true, _, _, true) => Some(D::SouthWest),
            (_, true, _, true) => Some(D::SouthEast),
            (true, ..) => Some(D::West),
            (_, true, ..) => Some(D::East),
            (_, _, true, _) => Some(D::North),
            (_, _, _, true) => Some(D::South),
            _ => None,
        };
        if let Some(d) = dir {
            let icon = match d {
                D::West | D::East => egui::CursorIcon::ResizeHorizontal,
                D::North | D::South => egui::CursorIcon::ResizeVertical,
                D::NorthWest | D::SouthEast => egui::CursorIcon::ResizeNwSe,
                _ => egui::CursorIcon::ResizeNeSw,
            };
            ctx.set_cursor_icon(icon);
            if ctx.input(|i| i.pointer.primary_pressed()) {
                ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(d));
            }
        }
    }

    /// Hand the active mode's composite to the ffmpeg frame pump.
    fn feed_output(&mut self, ctx: &egui::Context) {
        if !self.output.wants_frames() {
            if self.output.is_live() || self.output.is_recording() {
                ctx.request_repaint_after(std::time::Duration::from_millis(250));
            }
            return;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64(
            1.0 / self.output.cfg.fps.max(1) as f64,
        ));
        if !self.output.frame_due() {
            return;
        }
        let frame: Option<std::sync::Arc<Raster>> = match self.mode {
            Mode::Paint => Some(std::sync::Arc::new(self.paint.flattened())),
            Mode::Composite => Some(std::sync::Arc::new(self.comp.frame(self.comp.clock.time))),
            Mode::Scene => self.scene.frame.clone(),
            Mode::Animate => self.anim.frames.get(self.anim.current()).map(|r| {
                let mut out = Raster::filled(r.width, r.height, [250, 248, 242, 255]);
                for (d, s) in out
                    .data
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(r.data.as_chunks::<4>().0)
                {
                    let a = s[3] as u32;
                    for c in 0..3 {
                        d[c] = ((s[c] as u32 * a + d[c] as u32 * (255 - a)) / 255) as u8;
                    }
                }
                std::sync::Arc::new(out)
            }),
            Mode::Stream => Some(std::sync::Arc::new(self.stream.bit_raster())),
            Mode::Materials => self.scene.frame.clone(),
        };
        if let Some(f) = frame {
            self.output.push_frame(f);
        }
    }

    /// Dispatch a JSON-lines command script (see `--run`). Commands that need
    /// the user's approval (file writes, network, going live) are skipped.
    pub fn run_script(&mut self, path: &str) -> usize {
        let text = match std::fs::read_to_string(path) {
            Ok(t) => t,
            Err(e) => {
                self.toasts
                    .push(ToastKind::Error, &format!("script {}: {}", path, e));
                return 0;
            }
        };
        let mut n = 0;
        for (i, line) in text.lines().enumerate() {
            let l = line.trim();
            if l.is_empty() || l.starts_with('#') || l.starts_with("//") {
                continue;
            }
            match serde_json::from_str::<AppCommand>(l) {
                Ok(c) if c.needs_user() && !(self.project.script_files && c.is_project_file()) => {
                    self.toasts.push(
                        ToastKind::Error,
                        &format!(
                            "script line {}: {} needs the user; skipped",
                            i + 1,
                            c.name()
                        ),
                    )
                }
                Ok(c) => match self.dispatch(&c, Origin::User) {
                    Ok(msg) => {
                        eprintln!("script line {}: {}: {}", i + 1, c.name(), msg);
                        n += 1;
                    }
                    Err(e) => {
                        eprintln!("script line {}: {} failed: {}", i + 1, c.name(), e);
                        self.toasts
                            .push(ToastKind::Error, &format!("script line {}: {}", i + 1, e));
                    }
                },
                Err(e) => self
                    .toasts
                    .push(ToastKind::Error, &format!("script line {}: {}", i + 1, e)),
            }
        }
        n
    }

    /// Turn every visible mesh into a rigid body with its material's properties.
    fn arena_build(&mut self) -> Result<(), String> {
        use obs_rust_bitstream::arena::{Arena, Body};
        let env = self.scene.arena_env.clone();
        let mut a = Arena::new(env.clone());
        for o in self.scene.objs.iter().filter(|o| o.visible) {
            let Some(m) = o.world_mesh() else { continue };
            if m.verts.is_empty() {
                continue;
            }
            let (mut lo, mut hi) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
            for v in &m.verts {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            let mut half = [0.0f32; 3];
            let mut c = [0.0f32; 3];
            for k in 0..3 {
                half[k] = ((hi[k] - lo[k]) * 0.5).max(0.005);
                c[k] = (hi[k] + lo[k]) * 0.5;
            }
            let vol = if m.is_closed() {
                Some(m.volume().abs()).filter(|v| *v > 1e-9)
            } else {
                None
            };
            let (mat, menv) = match self.scene.assign.material_of(&o.name).ok().and_then(|n| {
                self.materials
                    .lib
                    .profiles
                    .get(n)
                    .map(|p| (n.to_string(), p.env.clone()))
            }) {
                Some(x) => x,
                None => ("unassigned".to_string(), materials::default_env()),
            };
            let pr = self.scene.phys.get(&o.name).copied().unwrap_or_default();
            let mut b = Body::new(
                &o.name,
                &mat,
                menv,
                half,
                c,
                pr.temp.unwrap_or(env.temperature),
                env.pressure,
            );
            if let Some(v) = vol {
                b = b.with_volume(v);
            }
            b.heater = pr.heater;
            // open meshes (planes) are static floors
            b.fixed = pr.fixed || !m.is_closed();
            b.vel = pr.vel;
            a.add(b);
        }
        if a.bodies.is_empty() {
            return Err("nothing to simulate: add a visible mesh".into());
        }
        self.scene.arena_start = Some(self.scene.objs.clone());
        self.scene.arena = Some(a);
        Ok(())
    }

    /// Copy body positions / shapes back onto the scene objects.
    fn arena_write_back(&mut self) {
        let (Some(a), Some(start)) = (&self.scene.arena, &self.scene.arena_start) else {
            return;
        };
        for b in &a.bodies {
            let (Some(o), Some(o0)) = (
                self.scene.objs.iter_mut().find(|o| o.name == b.name),
                start.iter().find(|o| o.name == b.name),
            ) else {
                continue;
            };
            // offset of the object origin from its bounding-box centre at the start
            let c0 = a_center(o0);
            let off = [o0.pos[0] - c0[0], o0.pos[1] - c0[1], o0.pos[2] - c0[2]];
            let sq = [
                b.half[0] / b.half0[0].max(1e-6),
                b.half[1] / b.half0[1].max(1e-6),
                b.half[2] / b.half0[2].max(1e-6),
            ];
            o.squash = [
                o0.squash[0] * sq[0],
                o0.squash[1] * sq[1],
                o0.squash[2] * sq[2],
            ];
            o.pos = [
                b.pos[0] + off[0] * sq[0],
                b.pos[1] + off[1] * sq[1],
                b.pos[2] + off[2] * sq[2],
            ];
            o.visible = o0.visible && !b.vaporised;
        }
    }

    /// Arena temperature (K) of a body by object name (case-insensitive).
    fn arena_temp(&self, name: &str) -> Option<f64> {
        self.scene
            .arena
            .as_ref()?
            .bodies
            .iter()
            .find(|b| b.name.eq_ignore_ascii_case(name))
            .map(|b| b.temp)
    }

    fn tick_lab(&mut self, dt: f32) {
        if self.lab.tick(dt) && self.mode == Mode::Materials {
            self.ctx.request_repaint();
        }
        if let Some(name) = self.lab.cpu.follow.clone() {
            if let Some(t) = self.arena_temp(&name) {
                let was = self.lab.cpu.report.as_ref().map(|r| r.correct);
                if let Some(msg) = self.lab.follow_die(t) {
                    let now = self.lab.cpu.report.as_ref().map(|r| r.correct);
                    if was == Some(true) && now == Some(false) {
                        self.toasts.push(
                            ToastKind::Error,
                            &format!("CPU on '{}' failed: {}", name, msg),
                        );
                    }
                }
            }
        }
    }

    /// Bring the worker's state home to `scene.arena`, waiting for queued work.
    pub fn arena_sync(&mut self) {
        if let Some(mut w) = self.arena_run.take() {
            // banked frame time still belongs to the run
            let secs = std::mem::take(&mut self.arena_carry);
            if secs > 0.0 {
                w.submit(move |a: &mut obs_rust_bitstream::arena::Arena, _| {
                    a.advance(secs);
                });
            }
            self.scene.arena = Some(w.finish());
            self.scene.arena_progress = None;
            self.arena_carry = 0.0;
            self.arena_write_back();
        }
    }

    /// Stop the worker early, keeping the state it reached.
    fn arena_cancel(&mut self) {
        if let Some(w) = &self.arena_run {
            w.cancel();
        }
        self.arena_sync();
    }

    /// Physics runs on a worker thread (A8): each frame hands it the frame
    /// time if it is idle (or banks it, up to 1 s, while it is busy) and
    /// shows the newest snapshot, so a heavy arena slows the simulation
    /// instead of the UI.
    fn tick_arena(&mut self, dt: f32) {
        let busy = self.arena_run.as_ref().is_some_and(|w| w.pending() > 0);
        if !self.scene.arena_playing && !busy {
            // a background run finished, or play stopped: take the state home
            self.arena_sync();
            return;
        }
        if self.scene.arena_playing {
            if self.arena_run.is_none() {
                if let Some(a) = self.scene.arena.clone() {
                    self.arena_run =
                        Some(obs_rust_bitstream::sim_worker::SimWorker::spawn("arena", a));
                }
            }
            self.arena_carry = (self.arena_carry + dt.max(0.0)).min(1.0);
            if let Some(w) = self.arena_run.as_mut().filter(|w| w.pending() == 0) {
                let secs = std::mem::take(&mut self.arena_carry);
                w.submit(move |a: &mut obs_rust_bitstream::arena::Arena, _| {
                    a.advance(secs);
                });
            }
        }
        if let Some(w) = &self.arena_run {
            let pr = w.progress();
            self.scene.arena_progress = (pr.1 > 0).then_some(pr);
            if let Some(a) = w.latest() {
                self.scene.arena = Some(a);
                self.arena_write_back();
            }
        }
        self.ctx.request_repaint();
    }

    /// The 3D viewport draws with the library's current PBR values.
    fn sync_scene_materials(&mut self) {
        if self.scene.surfaces.len() != self.materials.lib.profiles.len()
            || self
                .materials
                .lib
                .profiles
                .iter()
                .any(|(k, pr)| self.scene.surfaces.get(k) != Some(&materials::surface_of(&pr.pbr)))
        {
            self.scene.surfaces = self
                .materials
                .lib
                .profiles
                .iter()
                .map(|(k, pr)| (k.clone(), materials::surface_of(&pr.pbr)))
                .collect();
        }
    }

    /// One frame of UI. Separate from eframe so tests can drive it.
    pub fn frame(&mut self, ctx: &egui::Context) {
        self.fit_ui_scale(ctx);
        let now = ctx.input(|i| i.time);
        let dt = self
            .last_time
            .map(|t| (now - t) as f32)
            .unwrap_or(0.0)
            .clamp(0.0, 0.1);
        self.last_time = Some(now);
        self.toasts.now = now;
        for (k, m) in self.output.tick() {
            self.toasts.push(k, &m);
        }
        self.feed_output(ctx);
        self.scene.tick(dt);
        if self.anim.clock.tick(dt) | self.comp.clock.tick(dt) {
            ctx.request_repaint();
        }
        if self.scene.clock.playing || self.anim.clock.playing || self.comp.clock.playing {
            ctx.request_repaint();
        }
        if let Some(r) = self.video.take_image() {
            match r.and_then(|(prompt, bytes)| {
                self.paint.add_image_layer(
                    &format!("Imagine: {}", prompt.chars().take(24).collect::<String>()),
                    &bytes,
                )
            }) {
                Ok(t) => {
                    self.mode = Mode::Paint;
                    self.toasts.push(ToastKind::Success, &t);
                }
                Err(e) => self
                    .toasts
                    .push(ToastKind::Error, &format!("Image generation failed: {}", e)),
            }
        }
        if self.video.image_busy() {
            ctx.request_repaint_after(std::time::Duration::from_millis(300));
        }
        if self.video.tick() {
            ctx.request_repaint_after(std::time::Duration::from_millis(500));
        }
        match self.video.take_stitch() {
            Some(Ok(m)) => self.toasts.push(ToastKind::Success, &m),
            Some(Err(e)) => self.toasts.push(ToastKind::Error, &e),
            None => {}
        }
        self.pump_assistant();
        self.handle_keys(ctx);
        self.project_frame(ctx);
        self.edge_resize(ctx);

        let p = self.pal;
        self.sync_scene_materials();
        self.tick_arena(dt);
        self.tick_lab(dt);
        self.header(ctx);
        self.status_bar(ctx);

        // No frame stroke on panels: egui >= 0.31 draws Frame strokes inside the
        // rect and counts them in the layout size, which made every panel 2 px
        // larger and exposed a gold outline. In 0.29 the stroke was hidden under
        // the panel separator line, so dropping it keeps the 0.29 look.
        let side = egui::Frame::NONE.fill(p.panel);
        let padded = side.inner_margin(egui::Margin::same(SP_MD as i8));
        let wctx = self.workspace_ctx();

        if self.mode.has_shelf() {
            egui::TopBottomPanel::bottom("shelf")
                .frame(
                    egui::Frame::NONE
                        .fill(p.panel)
                        .inner_margin(egui::Margin::symmetric(SP_MD as i8, SP_XS as i8)),
                )
                .show_animated(ctx, self.show_shelf, |ui| {
                    let active = (self.mode == Mode::Scene).then_some(self.scene.gizmo);
                    maya::shelf(ui, &p, &mut self.shelf_tab, active, &mut self.bus);
                });
        }
        let show_tl = self.show_timeline && self.mode.has_timeline();
        egui::TopBottomPanel::bottom("timeline")
            .resizable(false)
            .frame(padded)
            .show_animated(ctx, show_tl, |ui| match self.mode {
                Mode::Scene => self.scene.timeline(ui, &p, &mut self.bus),
                Mode::Animate => self.anim.timeline(ui, &p, &mut self.toasts, &mut self.bus),
                Mode::Composite => self.comp.timeline(ui, &p, &mut self.bus),
                _ => {}
            });

        egui::SidePanel::right("dock")
            .resizable(true)
            .default_width(330.0)
            .width_range(280.0..=520.0)
            .frame(padded)
            .show_animated(ctx, self.show_assistant, |ui| {
                ui.horizontal(|ui| {
                    if let Some(i) = theme::segmented(
                        ui,
                        &p,
                        "dock-tabs",
                        &["Seraphina", "Grok Imagine"],
                        self.dock_tab,
                        110.0,
                    ) {
                        self.dock_tab = i;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if theme::icon_button(ui, &p, ic::X, "Hide panel  (Ctrl+.)", false)
                            .clicked()
                        {
                            self.bus.push(AppCommand::TogglePanel {
                                panel: "assistant".into(),
                                visible: Some(false),
                            });
                        }
                    });
                });
                ui.add_space(SP_SM);
                if self.dock_tab == 0 {
                    self.assistant.ui(ui, &p, &wctx, now);
                } else {
                    self.video.ui(ui, &p, &mut self.bus);
                }
            });
        egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(300.0)
            .width_range(250.0..=440.0)
            .frame(side)
            .show_animated(ctx, self.show_inspector, |ui| {
                ui.spacing_mut().slider_width = (ui.available_width() - 150.0).max(60.0);
                egui::ScrollArea::vertical()
                    .id_salt("inspector-scroll")
                    .show(ui, |ui| match self.mode {
                        Mode::Paint => self.paint.inspector(ui, &p, &mut self.bus),
                        m => {
                            egui::Frame::NONE
                                .inner_margin(egui::Margin::same(SP_MD as i8))
                                .show(ui, |ui| match m {
                                    Mode::Stream => self.stream.inspector(ui, &p),
                                    Mode::Scene => self.scene.inspector(ui, &p, &mut self.bus),
                                    Mode::Animate => {
                                        self.anim.inspector(ui, &p, &mut self.toasts, &mut self.bus)
                                    }
                                    Mode::Composite => self.comp.inspector(ui, &p, &mut self.bus),
                                    Mode::Materials if self.lab.tab > 0 => {
                                        self.lab.inspector(ui, &p, &mut self.bus)
                                    }
                                    Mode::Materials => {
                                        let sel = self
                                            .scene
                                            .selected
                                            .and_then(|i| self.scene.objs.get(i))
                                            .filter(|o| o.mesh.is_some())
                                            .map(|o| o.name.clone());
                                        self.materials.inspector(
                                            ui,
                                            &p,
                                            &mut self.bus,
                                            sel.as_deref(),
                                        )
                                    }
                                    Mode::Paint => {}
                                });
                        }
                    });
            });
        egui::SidePanel::left("tools")
            .resizable(true)
            .default_width(220.0)
            .width_range(190.0..=320.0)
            .frame(side)
            .show_animated(ctx, self.show_tools, |ui| {
                let (badge, tip) = self.mode.style();
                egui::Frame::NONE
                    .fill(p.surface)
                    .inner_margin(egui::Margin::symmetric(SP_SM as i8, 6))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(self.mode.icon())
                                    .size(15.0)
                                    .color(p.accent),
                            );
                            ui.label(
                                egui::RichText::new(format!(
                                    "{} TOOLS",
                                    self.mode.label().to_uppercase()
                                ))
                                .size(12.0)
                                .strong()
                                .color(p.text),
                            );
                            theme::badge(ui, &p, badge).on_hover_text(tip);
                        });
                    });
                ui.spacing_mut().slider_width = (ui.available_width() - 130.0).max(60.0);
                egui::ScrollArea::vertical()
                    .id_salt("tools-scroll")
                    .show(ui, |ui| match self.mode {
                        Mode::Paint => self.paint.tools(ui, &p, &mut self.bus),
                        m => {
                            egui::Frame::NONE
                                .inner_margin(egui::Margin::same(SP_MD as i8))
                                .show(ui, |ui| match m {
                                    Mode::Stream => {
                                        self.stream.tools(ui, &p, &mut self.toasts);
                                        self.output.panel(
                                            ui,
                                            &p,
                                            &mut self.bus,
                                            &mut self.show_settings,
                                        );
                                    }
                                    Mode::Scene => self.scene.tools(ui, &p, &mut self.bus),
                                    Mode::Animate => self.anim.tools(ui, &p),
                                    Mode::Composite => self.comp.tools(ui, &p, &mut self.bus),
                                    Mode::Materials if self.lab.tab > 0 => {
                                        self.lab.tools(ui, &p, &mut self.bus)
                                    }
                                    Mode::Materials => self.materials.tools(ui, &p, &mut self.bus),
                                    Mode::Paint => {}
                                });
                        }
                    });
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(p.bg))
            .show(ctx, |ui| match self.mode {
                Mode::Stream => self.stream.center(ui, &p, &mut self.bus),
                Mode::Paint => {
                    egui::Frame::NONE
                        .fill(p.panel)
                        .inner_margin(egui::Margin::symmetric(SP_MD as i8, 5))
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.paint.options_bar(ui, &p, &mut self.bus);
                        });
                    egui::Frame::NONE
                        .fill(p.bg)
                        .inner_margin(egui::Margin {
                            left: SP_SM as i8,
                            right: SP_SM as i8,
                            top: 4,
                            bottom: 0,
                        })
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                egui::Frame::NONE
                                    .fill(p.surface)
                                    .stroke(Stroke::new(1.0_f32, p.line))
                                    .corner_radius(CornerRadius {
                                        nw: 4,
                                        ne: 4,
                                        sw: 0,
                                        se: 0,
                                    })
                                    .inner_margin(egui::Margin::symmetric(SP_MD as i8 - 1, 3))
                                    .show(ui, |ui| {
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{}.psd @ {:.1}% (RGB/8)",
                                                self.paint.doc_name,
                                                self.paint.zoom * 100.0
                                            ))
                                            .size(11.5)
                                            .color(p.text),
                                        );
                                    });
                            });
                        });
                    self.paint
                        .center(ui, &p, &mut self.bus, &mut self.toasts, &mut self.sel_ov);
                }
                Mode::Scene => self.scene.center(ui, &p, &mut self.bus),
                Mode::Animate => self.anim.center(ui, &p),
                Mode::Composite => self.comp.center(ui, &p),
                Mode::Materials => {
                    self.lab.tab_strip(ui, &p, &mut self.bus);
                    if self.lab.tab > 0 {
                        self.lab.center(ui, &p);
                    } else {
                        self.materials.center(ui, &p, &mut self.bus);
                    }
                }
            });
        self.windows(ctx);
        self.project_modals(ctx);
        self.crash_modal(ctx);
        self.consent_modal(ctx);
        self.toasts.show(ctx, &p);
        // everything the UI queued this frame runs now, through the bus
        let had = !self.bus.is_empty();
        if self.continuous {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.process_bus();
        if had || !self.assistant.requests.is_empty() || self.assistant.want_send {
            ctx.request_repaint();
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crash::GPU_INIT.store(false, std::sync::atomic::Ordering::SeqCst);
        if crash::CRASH_TEST.swap(false, std::sync::atomic::Ordering::SeqCst) {
            panic!("crash test requested with --crash-test");
        }
        single::set_context(ctx);
        if single::FOCUS_REQUESTED.swap(false, std::sync::atomic::Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            self.toasts
                .push(ToastKind::Info, "Seraphina Studio is already open here");
        }
        self.frame(ctx);
        crash::FIRST_FRAME.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.end_session();
    }
}

impl App {
    /// Settings ▸ Appearance ▸ UI scale (H3). Applied when the slider is
    /// let go, so the layout doesn't jump around under the pointer.
    fn ui_scale_ui(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        let id = egui::Id::new("ui-scale-pct");
        let saved = (self.app_settings.scale() * 100.0).round();
        let mut pct: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(saved);
        let mut apply = None;
        ui.horizontal(|ui| {
            ui.label(theme::muted(p, "UI scale"));
            let r = ui.add(
                egui::Slider::new(
                    &mut pct,
                    settings::MIN_SCALE * 100.0..=settings::MAX_SCALE * 100.0,
                )
                .step_by(5.0)
                .suffix("%"),
            );
            if r.drag_stopped() || (r.changed() && !r.dragged()) {
                apply = Some(pct);
            }
            if saved != 100.0 && ui.small_button("100%").clicked() {
                apply = Some(100.0);
            }
        });
        if self.ui_scale_capped {
            ui.label(theme::muted(
                p,
                &format!(
                    "Showing {:.0}% until the window is bigger.",
                    ui.ctx().zoom_factor() * 100.0
                ),
            ));
        }
        match apply {
            Some(v) => {
                ui.data_mut(|d| d.remove::<f32>(id));
                self.set_ui_scale(ui.ctx(), v / 100.0);
            }
            None => ui.data_mut(|d| d.insert_temp(id, pct)),
        }
    }

    /// Keep the chosen UI scale from shrinking the layout below the
    /// window's minimum size (1180x700 points): on a small window or
    /// screen the zoom is capped until there's room again.
    fn fit_ui_scale(&mut self, ctx: &egui::Context) {
        let want = self.app_settings.scale();
        let z = ctx.zoom_factor();
        let fit = if want <= 1.0 {
            want
        } else {
            // window size in DPI-scaled points, independent of our zoom
            let size = ctx.screen_rect().size() * z;
            if size.x < 100.0 || size.y < 100.0 {
                return; // minimised
            }
            let room = (size.x / MIN_W).min(size.y / MIN_H);
            ((room * 20.0).floor() / 20.0).clamp(1.0, want)
        };
        self.ui_scale_capped = fit < want - 0.001;
        if (fit - z).abs() > 0.001 {
            ctx.set_zoom_factor(fit);
        }
        if z != self.ui_zoom_seen {
            self.ui_zoom_seen = z;
            self.refit_in = 2;
        }
        if self.refit_in > 0 {
            self.refit_in -= 1;
            if self.refit_in == 0 {
                self.paint.fit();
            }
            ctx.request_repaint();
        }
    }

    pub fn set_ui_scale(&mut self, ctx: &egui::Context, scale: f32) {
        self.app_settings.ui_scale = scale;
        let s = self.app_settings.scale();
        self.app_settings.ui_scale = s;
        ctx.set_zoom_factor(s);
        // tests must not touch the real settings file
        if cfg!(test) {
            return;
        }
        if let Err(e) = self.app_settings.save() {
            self.toasts
                .push(ToastKind::Error, &format!("Couldn't save settings: {}", e));
        }
    }

    /// Settings ▸ Seraphina ▸ xAI API key (C7). The key is saved to
    /// Credential Manager on Windows; env vars still take precedence.
    /// Nothing here ever shows, logs or toasts the key itself.
    /// Settings ▸ xAI spending: the meter and the 24 h spend cap (C3).
    fn spending_ui(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        ui.label(theme::section(p, "xAI spending"));
        theme::kv(ui, p, "This session", self.meter.session.summary());
        theme::kv(ui, p, "This project", self.meter.project.summary());
        theme::kv(
            ui,
            p,
            "Last 24 h",
            format!("${:.4}", self.meter.last_24h_usd()),
        );
        let mut cap = self.meter.cap_usd;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new("Spend cap").color(p.muted));
            ui.add(
                egui::DragValue::new(&mut cap)
                    .range(0.0..=10_000.0)
                    .speed(0.1)
                    .prefix("$")
                    .suffix(" / 24 h")
                    .max_decimals(2),
            )
            .on_hover_text("Seraphina stops sending chat, image and video requests to xAI once this much has been spent in the last 24 hours. 0 = no cap.");
            if cap > 0.0 && ui.small_button("Off").clicked() {
                cap = 0.0;
            }
        });
        if (cap - self.meter.cap_usd).abs() > f64::EPSILON {
            self.meter.cap_usd = cap;
            self.app_settings.spend_cap_usd = cap;
            self.save_settings();
        }
        ui.add(
            egui::Label::new(theme::muted(
                p,
                "Costs are xAI's own figures where it reports them, otherwise estimates from the public price list (video is estimated). A job xAI already accepted is billed even if you press Stop.",
            ))
            .wrap(),
        );
    }

    fn xai_key_ui(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        if let Some(rx) = &self.xai_test_rx {
            if let Ok(r) = rx.try_recv() {
                self.xai_test_rx = None;
                match r {
                    Ok(n) => self.toasts.push(
                        ToastKind::Success,
                        &format!("xAI key works ({} models available)", n),
                    ),
                    Err(e) => self
                        .toasts
                        .push(ToastKind::Error, &format!("xAI key test failed: {}", e)),
                }
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_millis(200));
            }
        }
        let current = self.xai_cache.get_or_insert_with(secrets::xai_key).clone();
        let env = matches!(current, Some((_, secrets::KeySource::Env)));
        let status = match &current {
            Some((k, secrets::KeySource::Env)) => format!("{} from environment", output::mask(k)),
            Some((k, secrets::KeySource::Saved)) => format!("{} saved", output::mask(k)),
            None => "not set".into(),
        };
        theme::kv(ui, p, "xAI API key", status);
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.xai_edit)
                    .password(true)
                    .hint_text("paste a new xAI API key")
                    .desired_width(200.0),
            );
            let typed = !self.xai_edit.trim().is_empty();
            if ui
                .add_enabled(typed, theme::toggle_button(p, "Save", false))
                .clicked()
            {
                let r = secrets::Store::user().save(secrets::Secret::XaiKey, &self.xai_edit);
                self.xai_edit.clear();
                match r {
                    Ok(place) => {
                        self.reload_llm_providers();
                        let msg = if env {
                            format!("xAI key saved in {}. XAI_API_KEY / SERAPHINA_GROK_API_KEY is set, so that one stays in use.", place.label())
                        } else {
                            format!("xAI key saved in {}. Seraphina and Grok Imagine use it now.", place.label())
                        };
                        self.toasts.push_for(ToastKind::Success, &msg, 6.0);
                    }
                    Err(e) => self
                        .toasts
                        .push(ToastKind::Error, &format!("Couldn't save the xAI key: {}", e)),
                }
            }
            let testing = self.xai_test_rx.is_some();
            let key = if typed {
                Some(self.xai_edit.trim().to_string())
            } else {
                current.as_ref().map(|(k, _)| k.clone())
            };
            if ui
                .add_enabled(
                    key.is_some() && !testing,
                    theme::toggle_button(p, if testing { "Testing…" } else { "Test key" }, false),
                )
                .on_hover_text("Lists models at the xAI API with the typed key, or the one in use")
                .clicked()
            {
                if let Some(k) = key {
                    crash::register_secret(&k);
                    let base = std::env::var("SERAPHINA_BASE_URL")
                        .ok()
                        .filter(|b| !b.trim().is_empty())
                        .unwrap_or_else(|| llm::OpenAiCompat::DEFAULT_BASE.into());
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let _ = tx.send(secrets::test_xai_key(&base, &k));
                    });
                    self.xai_test_rx = Some(rx);
                }
            }
            if matches!(current, Some((_, secrets::KeySource::Saved)))
                && ui.add(theme::toggle_button(p, "Forget", false)).clicked()
            {
                secrets::Store::user().clear(secrets::Secret::XaiKey);
                self.reload_llm_providers();
                self.toasts.push(ToastKind::Info, "Saved xAI key removed");
            }
        });
    }

    /// Rebuild the language-model and Grok Imagine clients after the saved
    /// key changes (env vars still win).
    fn reload_llm_providers(&mut self) {
        self.xai_cache = None;
        if cfg!(test) {
            return;
        }
        self.assistant.provider = llm::OpenAiCompat::from_env()
            .map(|p| std::sync::Arc::new(p) as std::sync::Arc<dyn llm::LlmProvider>);
        self.video.reload_provider();
    }

    /// Settings ▸ Graphics: renderer choice (D2), applied on the next launch.
    fn graphics_settings_ui(&mut self, ui: &mut egui::Ui, p: &theme::Palette) {
        ui.label(theme::section(p, "Graphics"));
        theme::kv(ui, p, "Renderer in use", crash::renderer());
        let mut pick = self.app_settings.renderer;
        egui::ComboBox::from_id_salt("renderer-pref")
            .selected_text(pick.label())
            .show_ui(ui, |ui| {
                for r in settings::RendererPref::ALL {
                    ui.selectable_value(&mut pick, r, r.label());
                }
            });
        if pick != self.app_settings.renderer {
            self.app_settings.renderer = pick;
            // a fresh choice: let Automatic try the preferred renderer again
            self.app_settings.auto_fallback = None;
            match self.app_settings.save() {
                Ok(()) => self.toasts.push(
                    ToastKind::Info,
                    "Renderer saved. Restart Seraphina Studio to use it.",
                ),
                Err(e) => self
                    .toasts
                    .push(ToastKind::Error, &format!("Couldn't save settings: {}", e)),
            }
        }
        ui.add(egui::Label::new(theme::muted(p, "If the chosen renderer can't start, the other one is used automatically. SERAPHINA_RENDERER=wgpu|glow overrides this.")).wrap());
    }
}

/// Smallest window (points) the layout is designed for.
const MIN_W: f32 = 1180.0;
const MIN_H: f32 = 700.0;

/// Everything the app creator needs; cloned for each renderer attempt.
#[derive(Clone)]
struct StartCfg {
    start_mode: Option<String>,
    open_arg: Option<String>,
    script_files: bool,
    run_script: Option<String>,
    start_light: bool,
    native: bool,
}

fn app_creator(c: StartCfg) -> eframe::AppCreator<'static> {
    Box::new(move |cc| {
        let mut app = App::new(&cc.egui_ctx);
        app.native_frame = c.native;
        app.continuous = std::env::var("SERAPHINA_CONTINUOUS").is_ok();
        if let Some(m) = c.start_mode.as_deref().and_then(Mode::parse) {
            app.mode = m;
        }
        if c.start_light {
            app.set_dark(false);
        }
        app.project.script_files = c.script_files;
        if let Some(path) = c.open_arg {
            let is_project = std::path::Path::new(&path)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(project::EXT));
            app.bus.push(if is_project {
                // a path typed in a terminal is relative to the shell's
                // directory, not Documents/Seraphina
                let abs = std::path::absolute(&path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or(path);
                AppCommand::OpenProject { path: Some(abs) }
            } else {
                AppCommand::OpenFile { path }
            });
        }
        if let Some(path) = c.run_script {
            app.run_script(&path);
        }
        Ok(Box::new(app))
    })
}

fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into())
}

/// Start over in a fresh process with `next`: a panic can leave the event
/// loop unusable, so an in-process retry isn't safe after one.
fn relaunch_with(next: settings::RendererPref, why: &str) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let name = match next {
        settings::RendererPref::Glow => "glow",
        _ => "wgpu",
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(std::env::args_os().skip(1))
        .env("SERAPHINA_RENDERER", name)
        .env("SERAPHINA_RENDERER_ONLY", "1")
        .env(
            "SERAPHINA_RENDERER_FALLBACK",
            why.chars().take(300).collect::<String>(),
        );
    cmd.spawn().is_ok()
}

fn main() -> eframe::Result<()> {
    use std::sync::atomic::Ordering;
    crash::install_hook();
    // `SeraphinaStudio <file>` (also how "Open with" passes a file on Windows)
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--crash-test") {
        crash::CRASH_TEST.store(true, Ordering::SeqCst);
    }
    let cfg = StartCfg {
        start_mode: std::env::var("SERAPHINA_MODE").ok(),
        open_arg: args.get(1).filter(|a| !a.starts_with('-')).cloned(),
        // `--allow-file-commands`: let a `--run` script save and open projects
        script_files: args.iter().any(|a| a == "--allow-file-commands"),
        // `--run script.jsonl`: one AppCommand JSON per line, dispatched at startup (demos, screenshots)
        run_script: args
            .iter()
            .position(|a| a == "--run")
            .and_then(|i| args.get(i + 1))
            .cloned(),
        start_light: std::env::var("SERAPHINA_LIGHT").is_ok(),
        native: std::env::var("SERAPHINA_NATIVE_FRAME").is_ok(),
    };
    // A second plain launch brings the running window forward instead.
    if single::defer_to_running(cfg.open_arg.is_some() || cfg.run_script.is_some()) {
        return Ok(());
    }
    // D2: start with the preferred renderer; if it can't start before the
    // first frame, try the other one instead of dying
    let (mut prefs, _) = settings::AppSettings::load();
    let env_r = std::env::var("SERAPHINA_RENDERER").ok();
    let mut order = prefs.renderer_order(env_r.as_deref(), cfg!(windows));
    if std::env::var("SERAPHINA_RENDERER_ONLY").is_ok() {
        order.truncate(1);
    }
    if let Ok(why) = std::env::var("SERAPHINA_RENDERER_FALLBACK") {
        crash::set_fallback_note(&why);
    }
    let remember = |prefs: &mut settings::AppSettings, r: settings::RendererPref| {
        if prefs.renderer == settings::RendererPref::Auto && env_r.is_none() {
            prefs.auto_fallback = Some(r);
            let _ = prefs.save();
        }
    };
    let mut failures: Vec<String> = Vec::new();
    for (i, &r) in order.iter().enumerate() {
        let next = order.get(i + 1).copied();
        crash::set_renderer(r.label());
        crash::GPU_INIT.store(next.is_some(), Ordering::SeqCst);
        let options = eframe::NativeOptions {
            renderer: settings::to_eframe(r),
            viewport: egui::ViewportBuilder::default()
                .with_title("Seraphina Studio")
                .with_inner_size([1600.0, 940.0])
                .with_min_inner_size([MIN_W, MIN_H])
                .with_decorations(cfg.native),
            ..Default::default()
        };
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            eframe::run_native("Seraphina Studio", options, app_creator(cfg.clone()))
        }));
        crash::GPU_INIT.store(false, Ordering::SeqCst);
        let started = crash::FIRST_FRAME.load(Ordering::SeqCst);
        match (res, next) {
            (Ok(Ok(())), _) => return Ok(()),
            (Err(p), Some(next)) if !started => {
                // the panic hook already wrote a gpu-* report
                let why = format!("{} panicked: {}", r.label(), panic_text(&*p));
                remember(&mut prefs, next);
                if relaunch_with(next, &why) {
                    return Ok(());
                }
                std::panic::resume_unwind(p);
            }
            (Err(p), _) => std::panic::resume_unwind(p),
            (Ok(Err(e)), Some(next)) if !started => {
                let why = format!("{} failed: {}", r.label(), e);
                let _ = crash::write_report("gpu", &why, "");
                crash::set_fallback_note(&why);
                failures.push(why);
                remember(&mut prefs, next);
            }
            (Ok(Err(e)), _) => {
                failures.push(format!("{} failed: {}", r.label(), e));
                let path = crash::write_report(
                    "crash",
                    &format!("Startup failed: {}", e),
                    &failures.join("\n"),
                );
                crash::message_box(
                    "Seraphina Studio couldn't start",
                    &format!(
                        "The graphics system couldn't be started.\n\n{}\n\n{}\n\nUpdating the graphics driver usually fixes this.",
                        failures.join("\n"),
                        path.map(|p| format!("Details: {}", p.display()))
                            .unwrap_or_default()
                    ),
                );
                return Err(e);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{LlmReply, MockProvider, ToolCall};
    use egui::Pos2;
    use std::sync::Arc;

    /// The Windows zip ships out.obsr and out.h264 next to
    /// SeraphinaStudio.exe: Stream mode opens out.obsr at startup and
    /// out.h264 is the default path in its Open box. Every file the Open
    /// hint names must ship too. In-repo zip lists:
    /// packaging/windows/zip-files.txt (what the workflows stage) and
    /// packaging/windows/README.txt. Unzipped anywhere and started from
    /// another folder, each must resolve next to the exe.
    #[test]
    fn stream_samples_resolve_next_to_the_exe_in_the_zip_layout() {
        let hinted = stream::files_named_in(stream::OPEN_HINT);
        assert!(!hinted.is_empty(), "the Open hint names no files");
        let mut names = vec![stream::STARTUP_SAMPLE, stream::DEFAULT_OPEN];
        names.extend(
            hinted
                .iter()
                .filter(|n| !names.contains(n))
                .collect::<Vec<_>>(),
        );
        let readme = include_str!("../../../packaging/windows/README.txt");
        let zip = zip_files();
        for name in &names {
            assert!(
                zip.iter().any(|(to, from)| to == name && from == name),
                "{name} is used or named by Stream mode but packaging/windows/zip-files.txt does not ship it"
            );
            assert!(
                lists_file(readme, name),
                "{name} is used or named by Stream mode but packaging/windows/README.txt's FILES IN THIS FOLDER does not list it"
            );
        }
        let root = crate::test_dir::TestDir::new("zip_layout");
        let app = root.path().join("SeraphinaStudio");
        std::fs::create_dir_all(&app).unwrap();
        let exe = app.join("SeraphinaStudio.exe");
        for name in &names {
            assert!(
                std::fs::metadata(name).map(|m| m.len()).unwrap_or(0) > 0,
                "{name} must be a non-empty file in the repo root"
            );
            // A name that is not in the working directory, so only the
            // next-to-the-exe lookup can find it.
            let unique = format!("zip_layout_{}_{name}", std::process::id());
            std::fs::copy(name, app.join(&unique)).unwrap();
            assert!(!std::path::Path::new(&unique).exists());
            let found = resolve_asset_from(&unique, Some(exe.clone()));
            assert_eq!(found, app.join(&unique), "{name} not found next to the exe");
            assert!(std::fs::metadata(&found).unwrap().len() > 0);
        }
    }

    /// packaging/windows/zip-files.txt as (path in zip, from).
    fn zip_files() -> Vec<(&'static str, &'static str)> {
        include_str!("../../../packaging/windows/zip-files.txt")
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(|l| {
                let f: Vec<&str> = l.split_whitespace().collect();
                assert_eq!(f.len(), 2, "zip-files.txt: {l}");
                (f[0], f[1])
            })
            .collect()
    }

    /// The entries at the top of the zip an `actions/upload-artifact`
    /// step makes from its `path:` lines. The action roots the zip at the
    /// paths' deepest common folder (a lone folder is its own root), so
    /// files listed from several folders keep their folders. `staged` is
    /// the folder stage.ps1 fills and what it puts there.
    fn artifact_entries(paths: &[&str], staged: Option<(&str, &[&str])>) -> Vec<String> {
        let norm = |p: &str| p.trim_start_matches("./").trim_end_matches('/').to_string();
        let is_dir = |p: &str| p.ends_with('/') || staged.is_some_and(|(d, _)| norm(d) == norm(p));
        let dirs: Vec<Vec<String>> = paths
            .iter()
            .map(|p| {
                let mut c: Vec<String> = norm(p).split('/').map(String::from).collect();
                if !is_dir(p) {
                    c.pop();
                }
                c
            })
            .collect();
        let mut root = dirs.first().cloned().unwrap_or_default();
        for d in &dirs {
            let n = root.iter().zip(d).take_while(|(a, b)| a == b).count();
            root.truncate(n);
        }
        let mut out = Vec::new();
        for p in paths {
            let rel: Vec<String> = norm(p)
                .split('/')
                .skip(root.len())
                .map(String::from)
                .collect();
            let rel = rel.join("/");
            match staged {
                Some((d, inside)) if norm(d) == norm(p) => out.extend(inside.iter().map(|e| {
                    if rel.is_empty() {
                        e.to_string()
                    } else {
                        format!("{rel}/{e}")
                    }
                })),
                _ if is_dir(p) => out.push(format!("{rel}/")),
                _ => out.push(rel),
            }
        }
        out
    }

    /// What ci.yml's windows-ui job uploads: the `path:` lines of its
    /// upload-artifact step and the `-Dest` folder it runs stage.ps1 into.
    fn ci_artifact(ci: &str) -> (Vec<&str>, Option<&str>) {
        let job: Vec<&str> = ci
            .lines()
            .skip_while(|l| *l != "  windows-ui:")
            .skip(1)
            .take_while(|l| l.is_empty() || l.starts_with("   "))
            .collect();
        assert!(!job.is_empty(), "ci.yml has no windows-ui job");
        let dest = job.iter().position(|l| l.contains("stage.ps1")).map(|i| {
            let mut w = job[i..].iter().flat_map(|l| l.split_whitespace());
            w.find(|t| *t == "-Dest");
            w.next().expect("stage.ps1 -Dest needs a folder")
        });
        let up = job
            .iter()
            .position(|l| l.contains("uses: actions/upload-artifact"))
            .expect("windows-ui has no upload-artifact step");
        let at = up
            + job[up..]
                .iter()
                .position(|l| l.trim_start().starts_with("path:"))
                .expect("upload-artifact has no path");
        let first = job[at].trim_start().trim_start_matches("path:").trim();
        let paths = if first == "|" {
            let indent = job[at + 1].len() - job[at + 1].trim_start().len();
            job[at + 1..]
                .iter()
                .take_while(|l| l.len() - l.trim_start().len() >= indent && !l.trim().is_empty())
                .map(|l| l.trim())
                .collect()
        } else {
            vec![first]
        };
        (paths, dest)
    }

    #[test]
    fn artifact_entries_root_the_zip_at_the_common_folder() {
        assert_eq!(
            artifact_entries(
                &["App.exe", "packaging/windows/README.txt", "assets/"],
                None
            ),
            ["App.exe", "packaging/windows/README.txt", "assets/"]
        );
        assert_eq!(
            artifact_entries(&["x/a.txt", "x/b/c.txt"], None),
            ["a.txt", "b/c.txt"]
        );
        let inside: &[&str] = &["App.exe", "README.txt"];
        assert_eq!(
            artifact_entries(&["pkg/"], Some(("pkg", inside))),
            ["App.exe", "README.txt"]
        );
        assert_eq!(
            artifact_entries(&["pkg", "LICENSE"], Some(("pkg", inside))),
            ["pkg/App.exe", "pkg/README.txt", "LICENSE"]
        );
    }

    /// README.txt, LICENSE and SOURCE.txt sit next to the exe at the top
    /// of the windows-ui artifact zip, as packaging/windows/zip-files.txt
    /// says; README.txt's FILES IN THIS FOLDER lists exactly those entries.
    #[test]
    fn ci_artifact_puts_the_docs_next_to_the_exe() {
        let zip = zip_files();
        let inside: Vec<&str> = zip.iter().map(|(to, _)| *to).collect();
        let (paths, dest) = ci_artifact(include_str!("../../../.github/workflows/ci.yml"));
        let entries = artifact_entries(&paths, dest.map(|d| (d, &inside[..])));
        for need in ["SeraphinaStudio.exe", "README.txt", "LICENSE", "SOURCE.txt"] {
            assert!(
                inside.contains(&need),
                "packaging/windows/zip-files.txt does not ship {need}"
            );
        }
        for e in &inside {
            assert!(
                entries.iter().any(|x| x == e),
                "the windows-ui artifact has no {e} at its root; it has {entries:?}"
            );
        }
        let readme = include_str!("../../../packaging/windows/README.txt");
        for e in &inside {
            assert!(
                lists_file(readme, &e.replace('/', "\\")),
                "README.txt's FILES IN THIS FOLDER does not list {e}"
            );
        }
        let listed = readme
            .lines()
            .skip_while(|l| l.trim() != "FILES IN THIS FOLDER")
            .skip(2)
            .take_while(|l| !l.trim().is_empty())
            .filter_map(|l| l.split_whitespace().next())
            .count();
        assert_eq!(listed, inside.len(), "README.txt lists files the zip lacks");
    }

    /// Runs packaging/windows/stage.ps1, as ci.yml's windows-ui job does, and checks
    /// the folder that becomes the zip root.
    #[test]
    #[cfg_attr(
        windows,
        ignore = "Wine has no pwsh; the windows-ui CI job runs stage.ps1 itself"
    )]
    fn stage_script_puts_every_listed_file_at_the_top() {
        let pwsh = std::env::var("SERAPHINA_PWSH").unwrap_or_else(|_| "pwsh".into());
        let dir = crate::test_dir::TestDir::new("stage");
        let dest = dir.path().join("pkg");
        let exe = std::env::current_exe().unwrap();
        let out = std::process::Command::new(&pwsh)
            .args([
                "-NoProfile",
                "-File",
                "packaging/windows/stage.ps1",
                "-Dest",
            ])
            .arg(&dest)
            .arg("-Exe")
            .arg(&exe)
            .args([
                "-Ref",
                "test",
                "-Sha",
                "0123abc",
                "-Repo",
                "SynerGro-AI/seraphina-studio",
            ])
            .output()
            .unwrap_or_else(|e| {
                panic!("can't run {pwsh} ({e}): install PowerShell 7 or set SERAPHINA_PWSH")
            });
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let mut top: Vec<String> = std::fs::read_dir(&dest)
            .unwrap()
            .map(|e| {
                let e = e.unwrap();
                let n = e.file_name().into_string().unwrap();
                if e.path().is_dir() {
                    format!("{n}/")
                } else {
                    n
                }
            })
            .collect();
        top.sort();
        let mut want: Vec<String> = zip_files().iter().map(|(to, _)| to.to_string()).collect();
        want.sort();
        assert_eq!(top, want);
        assert_eq!(
            std::fs::read(dest.join("SeraphinaStudio.exe")).unwrap(),
            std::fs::read(&exe).unwrap()
        );
        assert_eq!(
            std::fs::read(dest.join("README.txt")).unwrap(),
            std::fs::read("packaging/windows/README.txt").unwrap()
        );
        let source = std::fs::read_to_string(dest.join("SOURCE.txt")).unwrap();
        assert!(
            source.contains("tree/0123abc") && source.contains("cd seraphina-studio"),
            "{source}"
        );
        assert!(!source.contains("{{"), "{source}");
        for (to, _) in zip_files().iter().filter(|(to, _)| to.ends_with('/')) {
            assert!(
                std::fs::read_dir(dest.join(to)).unwrap().count() > 0,
                "{to} is empty"
            );
        }
    }

    /// Whether README.txt's FILES IN THIS FOLDER section has an entry
    /// for `name` (the first word of one of its indented lines). Prose
    /// elsewhere that merely mentions the file doesn't count.
    fn lists_file(readme: &str, name: &str) -> bool {
        readme
            .lines()
            .skip_while(|l| l.trim() != "FILES IN THIS FOLDER")
            .skip(2) // the title and its underline
            .take_while(|l| !l.trim().is_empty())
            .filter(|l| l.starts_with("  "))
            .any(|l| l.split_whitespace().next() == Some(name))
    }

    #[test]
    fn lists_file_takes_only_entries_of_the_files_section() {
        let readme = "Keep out.obsr next to the exe.\n\nFILES IN THIS FOLDER\n--------------------\n  App.exe     the app\n  out.h264    a sample\n\nENV\n  out.avbn    elsewhere\n";
        assert!(lists_file(readme, "out.h264"));
        assert!(lists_file(readme, "App.exe"));
        assert!(!lists_file(readme, "out.obsr"), "prose is not an entry");
        assert!(!lists_file(readme, "out.avbn"), "another section");
        assert!(!lists_file(readme, "out.h26"), "whole names only");
        assert!(!lists_file("  out.h264  no section\n", "out.h264"));
    }

    #[test]
    fn files_named_in_finds_file_names_only() {
        assert_eq!(
            stream::files_named_in("Try out.h264, out.avbn or out.obsr from the repo root."),
            ["out.h264", "out.avbn", "out.obsr"]
        );
        assert_eq!(
            stream::files_named_in(stream::OPEN_HINT),
            ["out.h264", "out.obsr"]
        );
    }

    fn input(t: f64) -> egui::RawInput {
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1600.0, 940.0),
            )),
            time: Some(t),
            ..Default::default()
        }
    }

    fn run(app: &mut App, ctx: &egui::Context, frames: usize, t0: f64) {
        for i in 0..frames {
            let _ = ctx.run(input(t0 + i as f64 * 0.05), |ctx| app.frame(ctx));
        }
    }

    /// Press at `from`, drag through `steps` points to `to`, release.
    fn drag(app: &mut App, ctx: &egui::Context, from: Pos2, to: Pos2, t0: f64) -> f64 {
        use egui::{Event, Modifiers, PointerButton};
        let mut t = t0;
        let mut frame = |events: Vec<Event>, t: &mut f64| {
            let mut i = input(*t);
            i.events = events;
            let _ = ctx.run(i, |ctx| app.frame(ctx));
            *t += 0.05;
        };
        let press = |pos, pressed| Event::PointerButton {
            pos,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        };
        frame(vec![Event::PointerMoved(from)], &mut t);
        frame(vec![press(from, true)], &mut t);
        let steps = 8;
        for k in 1..=steps {
            let p = from + (to - from) * (k as f32 / steps as f32);
            frame(vec![Event::PointerMoved(p)], &mut t);
        }
        frame(vec![press(to, false)], &mut t);
        frame(vec![], &mut t);
        t
    }

    #[test]
    fn an_animate_stroke_inks_only_the_current_frame_and_undoes_in_one_step() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.mode = Mode::Animate;
        run(&mut app, &ctx, 3, 0.0);
        assert!(
            app.dispatch(&AppCommand::Undo, Origin::User).is_err(),
            "undo with no history must say so"
        );
        app.dispatch(&AppCommand::TimelineScrub { time: 0.5 }, Origin::User)
            .unwrap();
        let cur = app.anim.current();
        let before = app.anim.frames.clone();
        drag(
            &mut app,
            &ctx,
            Pos2::new(700.0, 450.0),
            Pos2::new(900.0, 470.0),
            1.0,
        );
        assert_eq!(app.anim.current(), cur);
        for (i, (a, b)) in before.iter().zip(&app.anim.frames).enumerate() {
            if i != cur {
                assert!(a.data == b.data, "frame {} changed", i);
            }
        }
        let ink = |f: &obs_rust_bitstream::ui_canvas::Raster| -> Vec<[u8; 4]> {
            f.data
                .as_chunks::<4>()
                .0
                .iter()
                .filter(|p| p[3] > 0)
                .copied()
                .collect()
        };
        let inked = ink(&app.anim.frames[cur]);
        assert!(inked.len() > 200, "stroke covers {} px", inked.len());
        // the brush colour, at full strength in the stroke's core
        assert!(inked.contains(&[28, 28, 30, 255]));
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(
            app.anim.frames[cur].data == before[cur].data,
            "one undo removes the stroke"
        );
        app.dispatch(&AppCommand::Redo, Origin::User).unwrap();
        assert_eq!(ink(&app.anim.frames[cur]), inked, "redo is exact");
        assert!(app.dispatch(&AppCommand::Redo, Origin::User).is_err());
    }

    #[test]
    fn a_paint_brush_stroke_changes_only_the_selected_layer_and_undoes_exactly() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.mode = Mode::Paint;
        run(&mut app, &ctx, 3, 0.0);
        app.dispatch(
            &AppCommand::SelectLayer {
                layer: "Paint".into(),
            },
            Origin::User,
        )
        .unwrap();
        app.paint.tool = paint::Tool::Brush;
        let sel = app.paint.doc.selected;
        // layers may be trimmed to their content; compare them in document space
        let pixels = |app: &App| -> Vec<Vec<u8>> {
            let (w, h) = (app.paint.doc.width, app.paint.doc.height);
            app.paint
                .doc
                .layers
                .iter()
                .map(|l| {
                    let mut l = l.clone();
                    l.bake_to(w, h);
                    l.raster.data.clone()
                })
                .collect()
        };
        let before = pixels(&app);
        drag(
            &mut app,
            &ctx,
            Pos2::new(700.0, 450.0),
            Pos2::new(900.0, 470.0),
            1.0,
        );
        let after = pixels(&app);
        for (i, (a, b)) in before.iter().zip(&after).enumerate() {
            if i != sel {
                assert!(a == b, "layer {} changed", app.paint.doc.layers[i].name);
            }
        }
        let changed = before[sel]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(after[sel].as_chunks::<4>().0)
            .filter(|(a, b)| a != b)
            .count();
        assert!(changed > 200, "stroke changed {} px", changed);
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(pixels(&app) == before, "undo is exact");
        app.dispatch(&AppCommand::Redo, Origin::User).unwrap();
        assert!(pixels(&app) == after, "redo is exact");
    }

    #[test]
    fn a_fresh_launch_is_not_marked_unsaved() {
        let ctx = egui::Context::default();
        let mut app = crate::workspace::tests::test_app(&ctx);
        let s0 = (app.paint.edits, app.anim.edits, app.project.edits);
        let mut t = 0.0;
        for m in [
            Mode::Paint,
            Mode::Composite,
            Mode::Scene,
            Mode::Animate,
            Mode::Stream,
            Mode::Materials,
            Mode::Paint,
        ] {
            app.mode = m;
            run(&mut app, &ctx, 12, t);
            t += 1.0;
            assert!(
                !app.check_dirty(),
                "{:?}: {:?} -> {:?}",
                m,
                s0,
                (app.paint.edits, app.anim.edits, app.project.edits)
            );
        }
        // and the same after saving and reopening
        let tmp = crate::test_dir::TestDir::new("fresh-clean");
        let path = tmp.path().join("fresh.seraphina");
        app.save_project_to(&path).unwrap();
        app.open_project_from(&path).unwrap();
        for m in [Mode::Scene, Mode::Materials, Mode::Paint] {
            app.mode = m;
            run(&mut app, &ctx, 12, t);
            t += 1.0;
            assert!(!app.check_dirty(), "{:?} after reopening", m);
        }
        crate::test_dir::fail_late();
    }

    #[test]
    fn a_damaged_learning_bank_is_announced_at_startup() {
        let tmp = crate::test_dir::TestDir::new("startup-corrupt");
        let dir = tmp.path().to_path_buf();
        let path = dir.join("mercy_bank.json");
        std::fs::write(&path, b"\x00\x00garbage").unwrap();
        let ctx = egui::Context::default();
        let a = assistant_panel::AssistantPanel::with_provider(None, prefs::Prefs::load(path));
        let app = App::with(&ctx, a, videogen::VideoGenPanel::unconfigured());
        assert!(app
            .toasts
            .items
            .iter()
            .any(|t| matches!(t.kind, ToastKind::Warn) && t.text.contains("learning bank")));
        crate::test_dir::fail_late();
    }

    fn app_with(ctx: &egui::Context, provider: Option<Arc<dyn llm::LlmProvider>>) -> App {
        let a = assistant_panel::AssistantPanel::with_provider(provider, prefs::Prefs::ephemeral());
        let mut app = App::with(ctx, a, videogen::VideoGenPanel::unconfigured());
        app.app_settings.ai_consent = true; // tests that need the prompt clear it
                                            // never touch the user's saved output settings or stream key
                                            // a recordings folder of this App's own, removed when it drops
        let rec = test_dir::TestDir::new("app_rec");
        app.output = output::Output::new(
            output::OutputConfig {
                record_dir: rec.path().to_string_lossy().into_owned(),
                width: 320,
                height: 180,
                ..Default::default()
            },
            String::new(),
        );
        app.test_dirs.push(rec);
        let docs = crate::test_dir::TestDir::new("docs");
        app.project.docs_dir = docs.path().to_path_buf();
        app.test_dirs.push(docs);
        app
    }

    /// app_with's recordings folder goes when the App drops, also when
    /// the test panics part-way, and each App has its own.
    #[test]
    fn app_with_removes_its_recordings_folder_even_when_the_test_panics() {
        let ctx = egui::Context::default();
        let mut rec = std::path::PathBuf::new();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let app = app_with(&ctx, None);
            rec = std::path::PathBuf::from(&app.output.cfg.record_dir);
            assert!(rec.is_dir(), "{} not made", rec.display());
            std::fs::write(rec.join("take.mp4"), b"x").unwrap();
            let other = app_with(&ctx, None);
            assert_ne!(other.output.cfg.record_dir, app.output.cfg.record_dir);
            panic!("test failed part-way");
        }));
        assert!(r.is_err());
        assert!(!rec.exists(), "{} left behind", rec.display());
    }

    fn call(name: &str, args: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{}", name),
            name: name.into(),
            arguments: args.into(),
        }
    }

    fn wait_idle(app: &mut App, ctx: &egui::Context, secs: u64) {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        while std::time::Instant::now() < end {
            run(app, ctx, 1, 0.0);
            if !app.assistant.is_busy() && !app.assistant.want_send {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("assistant never finished");
    }

    fn action_lines(app: &App) -> Vec<(u64, ActionLine)> {
        app.assistant
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                assistant_panel::ItemKind::Actions { batch, lines, .. } => Some(
                    lines
                        .iter()
                        .map(|l| (*batch, l.clone()))
                        .collect::<Vec<_>>(),
                ),
                _ => None,
            })
            .flatten()
            .collect()
    }

    #[test]
    fn ctrl_plus_zooms_the_canvas_not_the_ui() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        assert!(!ctx.options(|o| o.zoom_with_keyboard));
        run(&mut app, &ctx, 2, 0.0);
        app.mode = Mode::Paint;
        let ui0 = ctx.zoom_factor();
        let z0 = app.paint.zoom;
        let key = |k: egui::Key| egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        let mut t = 1.0;
        for k in [
            egui::Key::Equals,
            egui::Key::Plus,
            egui::Key::Minus,
            egui::Key::Num0,
            egui::Key::Equals,
        ] {
            let mut i = input(t);
            i.modifiers = egui::Modifiers::COMMAND;
            i.events.push(key(k));
            let _ = ctx.run(i, |ctx| app.frame(ctx));
            run(&mut app, &ctx, 1, t + 0.05);
            t += 0.2;
            assert_eq!(ctx.zoom_factor(), ui0, "{:?} must not zoom the whole UI", k);
        }
        let _ = z0;
        // the canvas itself still responds to Ctrl+=
        let before = app.paint.zoom;
        let mut i = input(t);
        i.modifiers = egui::Modifiers::COMMAND;
        i.events.push(key(egui::Key::Equals));
        let _ = ctx.run(i, |ctx| app.frame(ctx));
        assert!(
            app.paint.zoom > before,
            "canvas zoom {} -> {}",
            before,
            app.paint.zoom
        );
        assert_eq!(ctx.zoom_factor(), ui0);
    }

    #[test]
    fn ai_undo_snapshots_are_capped() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        run(&mut app, &ctx, 1, 0.0);
        let layers0 = app.paint.doc.layers.len();
        let mut batches = Vec::new();
        for i in 0..100 {
            app.run_tool_calls(vec![call(
                "add_layer",
                &format!(r#"{{"name":"ai {}"}}"#, i),
            )]);
            batches.push(action_lines(&app).last().unwrap().0);
            assert!(
                app.snapshots.len() <= MAX_AI_SNAPSHOTS,
                "memory must stay flat: {} snapshots",
                app.snapshots.len()
            );
        }
        assert_eq!(app.snapshots.len(), MAX_AI_SNAPSHOTS);
        assert_eq!(app.paint.doc.layers.len(), layers0 + 100);
        // the oldest batch is gone: undo reports it and changes nothing
        assert!(!app.undo_batch(batches[0]));
        assert_eq!(app.paint.doc.layers.len(), layers0 + 100);
        // the newest is still undoable
        assert!(app.undo_batch(*batches.last().unwrap()));
        assert_eq!(app.paint.doc.layers.len(), layers0 + 99);
        // and so is the 10th newest
        assert!(app.undo_batch(batches[100 - MAX_AI_SNAPSHOTS]));
        assert_eq!(app.paint.doc.layers.len(), layers0 + 100 - MAX_AI_SNAPSHOTS);
    }

    #[test]
    fn seraphina_makes_the_cube_copper_and_builds_water_with_undo() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        run(&mut app, &ctx, 2, 0.0);
        app.scene.shading = scene::Shading::Shaded;
        // grey before
        let grey = app.scene.surface_of("cube1");
        assert_eq!(grey, obs_rust_bitstream::mesh3d::Surface::grey());
        // "make the cube copper" → material_assign through the Triad gate
        app.run_tool_calls(vec![call(
            "material_assign",
            r#"{"object":"cube1","material":"copper"}"#,
        )]);
        let lines = action_lines(&app);
        assert!(
            lines.iter().any(
                |(_, l)| l.status == LineStatus::Done && l.text.contains("cube1 is now Copper")
            ),
            "{:?}",
            lines.iter().map(|l| &l.1.text).collect::<Vec<_>>()
        );
        assert_eq!(app.scene.assign.material_of("cube1"), Ok("Copper"));
        assert!(
            app.materials.lib.registry.get("Copper").is_ok(),
            "saved into MaterialRegistry"
        );
        let cu = app.scene.surface_of("cube1");
        assert_eq!(cu.metalness, 1.0);
        // the rendered cube is copper-coloured (red > blue) where it was grey
        app.scene.selected = None;
        let f = app.scene.render(
            320,
            200,
            Color32::BLACK,
            Color32::from_gray(40),
            app.pal.accent,
        );
        let (mut r, mut b, mut n) = (0u64, 0u64, 0u64);
        for (i, id) in f.ids.iter().enumerate() {
            if *id >> 16 == 1 {
                r += f.rgba[i * 4] as u64;
                b += f.rgba[i * 4 + 2] as u64;
                n += 1;
            }
        }
        // copper F0 in sRGB is ≈ (250, 209, 194): red/blue ≈ 1.29 (grey is bluish, < 1)
        assert!(
            n > 500 && r > b * 115 / 100,
            "copper cube: n {} r {} b {}",
            n,
            r,
            b
        );
        // undo in 3D removes the assignment
        app.mode = Mode::Scene;
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(app.scene.assign.material_of("cube1").is_err());
        app.dispatch(&AppCommand::Redo, Origin::User).unwrap();
        assert_eq!(app.scene.assign.material_of("cube1"), Ok("Copper"));
        // non-mesh targets and junk materials are rejected
        assert!(app
            .dispatch(
                &AppCommand::MaterialAssign {
                    object: Some("key_light".into()),
                    material: "gold".into()
                },
                Origin::User
            )
            .is_err());
        assert!(app
            .dispatch(
                &AppCommand::MaterialAssign {
                    object: Some("cube1".into()),
                    material: "unobtainium".into()
                },
                Origin::User
            )
            .is_err());
        // "build H2O"
        app.run_tool_calls(vec![call("materials_build", r#"{"formula":"H2O"}"#)]);
        assert_eq!(app.mode, Mode::Materials);
        let w = app.materials.built.clone().unwrap();
        assert_eq!(w.formula, "H2O");
        assert!(w.analysis.charge_balanced && (w.pbr.ior - 1.333).abs() < 1e-3);
        app.dispatch(&AppCommand::MaterialsSave { name: None }, Origin::User)
            .unwrap();
        assert!(app.materials.lib.profiles.contains_key("Water"));
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(
            !app.materials.lib.profiles.contains_key("Water"),
            "undo the save"
        );
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(app.materials.built.is_none(), "undo the build");
        // the whole AI batch can be undone too
        let batch = lines[0].0;
        assert!(app.undo_batch(batch));
        assert!(app.scene.assign.material_of("cube1").is_err());
        // Materials mode renders with the inspector and builder populated
        app.dispatch(
            &AppCommand::MaterialsPreset {
                preset: "gold".into(),
            },
            Origin::User,
        )
        .unwrap();
        run(&mut app, &ctx, 3, 10.0);
    }

    /// UI frame time with a heavy arena playing, before (M1: all-pairs
    /// contacts stepped inside the frame) and after (A8: sweep-and-prune on
    /// the worker). `cargo test --release --bin ui -- --ignored
    /// measure_ui_frame_time --nocapture` (SERAPHINA_BENCH_BODIES, default 1500).
    #[test]
    #[ignore = "benchmark: run with --release and --nocapture"]
    fn measure_ui_frame_time_with_a_heavy_arena() {
        use obs_rust_bitstream::arena::{stress_pile, Collide, Environment};
        let n: usize = std::env::var("SERAPHINA_BENCH_BODIES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1500);
        let frames = 120;
        let stats = |mut v: Vec<f64>| {
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            (
                v.iter().sum::<f64>() / v.len() as f64,
                v[v.len() / 2],
                v[v.len() * 95 / 100],
            )
        };
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.mode = Mode::Scene;
        let dt = 1.0 / 60.0;
        let mut t = 0.0;
        for _ in 0..5 {
            let _ = ctx.run(input(t), |ctx| app.frame(ctx));
            t += dt;
        }
        // before: the frame also steps the arena itself, all pairs
        app.scene.arena = Some(stress_pile(Environment::earth(), n, 11));
        app.scene.arena_playing = false;
        let mut before = Vec::new();
        for _ in 0..frames {
            let s = std::time::Instant::now();
            let a = app.scene.arena.as_mut().unwrap();
            for _ in 0..4 {
                a.step_with(Collide::Brute);
            }
            let _ = ctx.run(input(t), |ctx| app.frame(ctx));
            before.push(s.elapsed().as_secs_f64() * 1e3);
            t += dt;
        }
        // after: the worker steps it; the frame shows the newest snapshot
        app.scene.arena = Some(stress_pile(Environment::earth(), n, 11));
        let t_sim0 = app.scene.arena.as_ref().unwrap().time;
        app.scene.arena_playing = true;
        let wall = std::time::Instant::now();
        let mut after = Vec::new();
        for _ in 0..frames {
            let s = std::time::Instant::now();
            let _ = ctx.run(input(t), |ctx| app.frame(ctx));
            after.push(s.elapsed().as_secs_f64() * 1e3);
            // pace like a 60 Hz display
            let used = s.elapsed().as_secs_f64();
            if used < dt {
                std::thread::sleep(std::time::Duration::from_secs_f64(dt - used));
            }
            t += dt;
        }
        let wall = wall.elapsed().as_secs_f64();
        app.scene.arena_playing = false;
        app.arena_sync();
        let sim = app.scene.arena.as_ref().unwrap().time - t_sim0;
        let (b, a) = (stats(before), stats(after));
        println!(
            "{n} bodies, {frames} frames: before mean {:.2} ms / median {:.2} / p95 {:.2}; after mean {:.2} ms / median {:.2} / p95 {:.2}; simulation ran {:.2} s in {:.2} s wall ({:.0}% of real time)",
            b.0, b.1, b.2, a.0, a.1, a.2, sim, wall, 100.0 * sim / wall
        );
    }

    #[test]
    fn a_document_wider_than_the_gpu_texture_limit_draws_from_tiles() {
        use obs_rust_bitstream::paint_engine::PaintLayer;
        use obs_rust_bitstream::ui_canvas::Raster;
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.mode = Mode::Paint;
        run(&mut app, &ctx, 2, 0.0);
        assert_eq!(app.paint.gpu_tile_count(), 1);
        // the test context reports egui's default 2048 px texture limit
        let (w, h) = (9000u32, 700u32);
        let mut r = Raster::new(w, h);
        for (i, px) in r.data.chunks_mut(4).enumerate() {
            px.copy_from_slice(&[(i % 251) as u8, 90, 160, 255]);
        }
        app.paint.set_doc(crate::paint::PaintDoc {
            layers: vec![PaintLayer::new(1, "Wide", r)],
            groups: Vec::new(),
            width: w,
            height: h,
            selection: None,
            selected: 0,
            next_id: 2,
        });
        let out = ctx.run(input(1.0), |ctx| app.frame(ctx));
        let tiles = crate::tiled_tex::plan(w, h, 2048);
        assert_eq!(app.paint.gpu_tile_count(), tiles.len());
        assert_eq!(tiles.len(), 5);
        // nothing uploaded is over the limit
        for (_, d) in &out.textures_delta.set {
            assert!(d.image.width() <= 2048 && d.image.height() <= 2048);
        }
        // a stroke updates the tiles it crosses and the frame still draws
        app.dispatch(
            &AppCommand::BrushStroke {
                points: vec![[3900.0, 300.0], [4300.0, 340.0]],
            },
            Origin::User,
        )
        .unwrap();
        let out = ctx.run(input(1.1), |ctx| app.frame(ctx));
        assert!(out
            .textures_delta
            .set
            .iter()
            .all(|(_, d)| d.image.width() <= 2048 && d.image.height() <= 2048));
    }

    #[test]
    fn opening_an_image_over_the_megapixel_cap_is_refused_with_a_message() {
        fn crc32(b: &[u8]) -> u32 {
            let mut c = !0u32;
            for &x in b {
                c ^= x as u32;
                for _ in 0..8 {
                    c = if c & 1 != 0 {
                        0xEDB8_8320 ^ (c >> 1)
                    } else {
                        c >> 1
                    };
                }
            }
            !c
        }
        let tmp = crate::test_dir::TestDir::new("mp");
        let dir = tmp.path();
        // a PNG header claiming 20000×20000 (400 MP) and no pixel data
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let mut ihdr = b"IHDR".to_vec();
        ihdr.extend(20000u32.to_be_bytes());
        ihdr.extend(20000u32.to_be_bytes());
        ihdr.extend([8, 6, 0, 0, 0]);
        png.extend(13u32.to_be_bytes());
        png.extend(&ihdr);
        png.extend(crc32(&ihdr).to_be_bytes());
        for c in [&b"IDAT"[..], &b"IEND"[..]] {
            png.extend(0u32.to_be_bytes());
            png.extend(c);
            png.extend(crc32(c).to_be_bytes());
        }
        let big_png = dir.join("huge.png");
        std::fs::write(&big_png, &png).unwrap();
        // a PSD header claiming 30000×30000
        let mut psd = b"8BPS".to_vec();
        psd.extend([0, 1, 0, 0, 0, 0, 0, 0, 0, 3]);
        psd.extend(30000u32.to_be_bytes());
        psd.extend(30000u32.to_be_bytes());
        psd.extend([0, 8, 0, 3]);
        let big_psd = dir.join("huge.psd");
        std::fs::write(&big_psd, &psd).unwrap();

        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        let doc = (
            app.paint.doc.width,
            app.paint.doc.height,
            app.paint.doc.layers.len(),
        );
        for (f, mp) in [(&big_png, "400 megapixels"), (&big_psd, "900 megapixels")] {
            let e = app
                .dispatch(
                    &AppCommand::OpenFile {
                        path: f.display().to_string(),
                    },
                    Origin::User,
                )
                .unwrap_err();
            assert!(e.contains(mp) && e.contains("MP limit"), "{}", e);
            assert!(e.contains("SERAPHINA_MAX_MEGAPIXELS"), "{}", e);
        }
        assert_eq!(
            doc,
            (
                app.paint.doc.width,
                app.paint.doc.height,
                app.paint.doc.layers.len()
            ),
            "nothing changed"
        );
        assert!(crate::paint::check_open_size(7680, 4320).is_ok());
        assert!(
            crate::paint::check_open_size(16384, 8192).is_err()
                == (16384u64 * 8192 > crate::paint::max_megapixels() * 1_000_000)
        );
        crate::test_dir::fail_late();
    }

    #[test]
    fn background_arena_steps_match_inline_steps_and_keep_the_ui_free() {
        let ctx = egui::Context::default();
        let setup = |ctx: &egui::Context| {
            let mut app = app_with(ctx, None);
            run(&mut app, ctx, 2, 0.0);
            app.dispatch(
                &AppCommand::SceneTransform {
                    object: Some("cube1".into()),
                    translate: Some([0.0, 2.5, 0.0]),
                    rotate_y: None,
                    scale: None,
                },
                Origin::User,
            )
            .unwrap();
            app
        };
        // 1000 steps on the worker…
        let mut bg = setup(&ctx);
        let r = bg
            .dispatch(&AppCommand::ArenaStep { steps: Some(1000) }, Origin::User)
            .unwrap();
        assert!(r.contains("background"), "{}", r);
        assert!(bg.arena_run.is_some());
        // the UI keeps drawing frames while it runs, and shows progress
        run(&mut bg, &ctx, 3, 16.0);
        bg.arena_sync();
        assert!(bg.arena_run.is_none() && bg.scene.arena_progress.is_none());
        // …equal 240+240+240+240+40 steps inline, bit for bit
        let mut inl = setup(&ctx);
        for n in [240, 240, 240, 240, 40] {
            inl.dispatch(&AppCommand::ArenaStep { steps: Some(n) }, Origin::User)
                .unwrap();
        }
        assert!(inl.arena_run.is_none());
        assert_eq!(
            format!("{:?}", bg.scene.arena.as_ref().unwrap().bodies),
            format!("{:?}", inl.scene.arena.as_ref().unwrap().bodies)
        );
        assert_eq!(bg.scene.objs[0].pos, inl.scene.objs[0].pos);
        // reset during a background run cancels it
        bg.dispatch(
            &AppCommand::ArenaStep {
                steps: Some(240 * 600),
            },
            Origin::User,
        )
        .unwrap();
        let t = std::time::Instant::now();
        bg.dispatch(&AppCommand::ArenaReset, Origin::User).unwrap();
        assert!(t.elapsed().as_secs_f32() < 5.0);
        assert!(bg.arena_run.is_none() && bg.scene.arena.is_none());
    }

    #[test]
    fn arena_drops_the_copper_cube_on_the_moon_and_melts_ice() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        run(&mut app, &ctx, 2, 0.0);
        // Seraphina: "drop the copper cube from 3 m on the Moon"
        app.run_tool_calls(vec![
            call(
                "material_assign",
                r#"{"object":"cube1","material":"copper"}"#,
            ),
            call(
                "scene_transform",
                r#"{"object":"cube1","translate":[0,2.5,0]}"#,
            ),
            call("arena_env", r#"{"preset":"moon"}"#),
            call("arena_step", r#"{"steps":1}"#),
        ]);
        let lines = action_lines(&app);
        assert!(
            lines.iter().all(|(_, l)| l.status == LineStatus::Done),
            "{:?}",
            lines.iter().map(|l| &l.1.text).collect::<Vec<_>>()
        );
        let a = app.scene.arena.as_ref().unwrap();
        let b = a.body("cube1").unwrap();
        assert_eq!(b.material, "Copper");
        assert!((b.mass - 8960.0).abs() < 2.0, "1 m³ of copper: {}", b.mass);
        assert!((a.env.gravity - 1.62).abs() < 1e-6);
        // fall 2.5 m on the Moon: t = sqrt(2h/g) = 1.757 s
        let t_land = (2.0 * 2.5f32 / 1.62).sqrt();
        let steps = (t_land / obs_rust_bitstream::arena::DT) as u32 - 4;
        app.dispatch(&AppCommand::ArenaStep { steps: Some(steps) }, Origin::User)
            .unwrap();
        app.arena_sync();
        assert!(
            app.scene.objs[0].pos[1] > 0.5,
            "still falling just before t_land"
        );
        // it bounces (copper e = 0.55); bounces take long in lunar gravity
        app.dispatch(&AppCommand::ArenaStep { steps: Some(240) }, Origin::User)
            .unwrap();
        assert!(
            app.scene.arena.as_ref().unwrap().body("cube1").unwrap().vel[1] != 0.0
                || app.scene.objs[0].pos[1] > 0.51
        );
        app.dispatch(
            &AppCommand::ArenaStep {
                steps: Some(240 * 15),
            },
            Origin::User,
        )
        .unwrap();
        app.arena_sync();
        let y = app.scene.objs[0].pos[1];
        assert!((y - 0.5).abs() < 0.02, "landed on the ground: {}", y);
        // play advances with real frame time
        app.dispatch(
            &AppCommand::ArenaPlay {
                playing: Some(true),
            },
            Origin::User,
        )
        .unwrap();
        let t0 = app.scene.arena.as_ref().unwrap().time;
        run(&mut app, &ctx, 10, 20.0);
        app.arena_sync();
        assert!(app.scene.arena.as_ref().unwrap().time > t0 + 0.2);
        // reset puts it back where it was before the simulation
        app.dispatch(&AppCommand::ArenaReset, Origin::User).unwrap();
        assert!(app.scene.arena.is_none());
        assert!(
            (app.scene.objs[0].pos[1] - 3.0).abs() < 1e-4,
            "{}",
            app.scene.objs[0].pos[1]
        );
        // undo of a simulation start goes back to the pre-sim scene
        app.dispatch(&AppCommand::ArenaStep { steps: Some(240) }, Origin::User)
            .unwrap();
        assert!(app.scene.objs[0].pos[1] < 3.0);
        app.mode = Mode::Scene;
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(app.scene.arena.is_none() && (app.scene.objs[0].pos[1] - 3.0).abs() < 1e-4);
        // ice melts with a heater: a 10 cm ice cube on Earth
        app.dispatch(
            &AppCommand::ArenaEnv {
                preset: Some("earth".into()),
                gravity: None,
                air_density: None,
                temperature_c: None,
                pressure_kpa: None,
                wind: None,
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(
            &AppCommand::SceneAdd {
                kind: "cube".into(),
            },
            Origin::User,
        )
        .unwrap();
        let ice = app.scene.objs[app.scene.selected.unwrap()].name.clone();
        app.dispatch(
            &AppCommand::SceneTransform {
                object: Some(ice.clone()),
                translate: Some([3.0, 0.0, 0.0]),
                rotate_y: None,
                scale: Some(0.1),
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(
            &AppCommand::MaterialAssign {
                object: Some(ice.clone()),
                material: "ice".into(),
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(
            &AppCommand::ArenaBody {
                object: Some(ice.clone()),
                temperature_c: Some(-10.0),
                heater_w: Some(5000.0),
                fixed: None,
                velocity: None,
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(
            &AppCommand::ArenaStep {
                steps: Some(240 * 120),
            },
            Origin::User,
        )
        .unwrap();
        app.arena_sync();
        let b = app
            .scene
            .arena
            .as_ref()
            .unwrap()
            .body(&ice)
            .unwrap()
            .clone();
        assert_eq!(
            b.phase,
            obs_rust_bitstream::material_env::Phase::Liquid,
            "{:?} {} K",
            b.phase,
            b.temp
        );
        let o = app.scene.objs.iter().find(|o| o.name == ice).unwrap();
        assert!(
            o.squash[1] < 0.5 && o.squash[0] > 1.5,
            "puddle: {:?}",
            o.squash
        );
        assert!(app
            .scene
            .arena
            .as_ref()
            .unwrap()
            .events
            .log
            .iter()
            .any(|(_, e)| e.contains("solid → liquid")));
        assert!(app.app_context().contains("Physics arena"));
        // errors
        assert!(app
            .dispatch(
                &AppCommand::ArenaEnv {
                    preset: Some("jupiter".into()),
                    gravity: None,
                    air_density: None,
                    temperature_c: None,
                    pressure_kpa: None,
                    wind: None
                },
                Origin::User
            )
            .is_err());
        assert!(app
            .dispatch(
                &AppCommand::ArenaBody {
                    object: Some("key_light".into()),
                    temperature_c: None,
                    heater_w: Some(1.0),
                    fixed: None,
                    velocity: None
                },
                Origin::User
            )
            .is_err());
    }

    #[test]
    fn demo_scripts_run_cleanly() {
        let ctx = egui::Context::default();
        for f in ["materials_scene", "arena_moon", "arena_melt"] {
            let mut app = app_with(&ctx, None);
            let path = format!("{}/docs/scripts/{}.jsonl", env!("CARGO_MANIFEST_DIR"), f);
            let lines = std::fs::read_to_string(&path)
                .unwrap()
                .lines()
                .filter(|l| l.trim().starts_with('{'))
                .count();
            assert_eq!(app.run_script(&path), lines, "{}: every line dispatches", f);
            run(&mut app, &ctx, 2, 0.0);
        }
        let mut app = app_with(&ctx, None);
        assert_eq!(app.run_script("/nonexistent.jsonl"), 0);
    }

    #[test]
    fn physics_owns_keyframed_objects_until_reset() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        // cube1 carries keyframes in the default scene
        assert!(!app.scene.objs[0].track_y.is_empty());
        let y0 = app.scene.objs[0].pos[1];
        app.dispatch(
            &AppCommand::SceneTransform {
                object: Some("cube1".into()),
                translate: Some([0.0, 2.6, 0.0]),
                rotate_y: None,
                scale: None,
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(
            &AppCommand::ArenaEnv {
                preset: Some("moon".into()),
                gravity: None,
                air_density: None,
                temperature_c: None,
                pressure_kpa: None,
                wind: None,
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(&AppCommand::ArenaStep { steps: Some(300) }, Origin::User)
            .unwrap();
        run(&mut app, &ctx, 3, 0.0);
        // 1.25 s on the Moon: Δy = ½·1.62·1.25² = 1.266 m
        let y = app.scene.objs[0].pos[1];
        assert!((y - (y0 + 2.6 - 1.266)).abs() < 0.01, "y {}", y);
        app.dispatch(&AppCommand::ArenaReset, Origin::User).unwrap();
        run(&mut app, &ctx, 2, 0.0);
        assert!(app.scene.arena.is_none());
    }

    #[test]
    fn every_mode_renders_dark_and_light() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        for dark in [true, false] {
            app.set_dark(dark);
            for m in Mode::ALL {
                app.mode = m;
                for tab in 0..2 {
                    app.dock_tab = tab;
                    run(&mut app, &ctx, 2, 0.0);
                }
            }
        }
    }

    #[test]
    fn panels_and_windows_can_toggle() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.show_shortcuts = true;
        app.show_settings = true;
        app.show_account = true;
        run(&mut app, &ctx, 2, 0.0);
        for panel in ["tools", "inspector", "timeline", "shelf", "assistant"] {
            assert!(app
                .dispatch(
                    &AppCommand::TogglePanel {
                        panel: panel.into(),
                        visible: Some(false)
                    },
                    Origin::User
                )
                .is_ok());
        }
        assert!(
            !app.show_tools
                && !app.show_inspector
                && !app.show_timeline
                && !app.show_shelf
                && !app.show_assistant
        );
        for m in Mode::ALL {
            app.mode = m;
            run(&mut app, &ctx, 2, 0.0);
        }
        assert!(app
            .dispatch(
                &AppCommand::TogglePanel {
                    panel: "nope".into(),
                    visible: None
                },
                Origin::User
            )
            .is_err());
    }

    #[test]
    fn ui_scale_command_zooms_and_clamps() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        for (pct, want) in [(150.0, 1.5), (500.0, 2.0), (10.0, 0.8), (100.0, 1.0)] {
            app.dispatch(&AppCommand::SetUiScale { percent: pct }, Origin::User)
                .unwrap();
            // egui applies a new zoom at the start of the next pass
            let _ = ctx.run(egui::RawInput::default(), |_| {});
            assert_eq!(ctx.zoom_factor(), want, "{pct}");
            assert_eq!(app.app_settings.ui_scale, want);
        }
        assert!(app
            .dispatch(&AppCommand::SetUiScale { percent: f32::NAN }, Origin::User)
            .is_err());
    }

    #[test]
    fn mode_and_theme_go_through_the_bus() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.bus.push(AppCommand::SwitchMode { mode: "3d".into() });
        app.bus.push(AppCommand::SetTheme { dark: false });
        app.process_bus();
        assert_eq!(app.mode, Mode::Scene);
        assert!(!app.dark);
        assert!(app
            .dispatch(
                &AppCommand::SwitchMode {
                    mode: "photoshop".into()
                },
                Origin::User
            )
            .is_ok());
        assert_eq!(app.mode, Mode::Paint);
        assert!(app
            .dispatch(&AppCommand::SwitchMode { mode: "xyz".into() }, Origin::User)
            .is_err());
        assert!(app.toasts.items.iter().any(|t| t.kind == ToastKind::Error));
    }

    #[test]
    fn user_tool_choice_updates_learning() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        let n = app.assistant.prefs.bank.updates;
        app.dispatch(
            &AppCommand::SelectTool {
                tool: "lasso".into(),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.paint.tool, paint::Tool::Lasso);
        assert_eq!(app.assistant.prefs.bank.updates, n + 1);
        app.dispatch(
            &AppCommand::SelectTool {
                tool: "brush".into(),
            },
            Origin::Seraphina,
        )
        .unwrap();
        assert_eq!(app.assistant.prefs.bank.updates, n + 1);
    }

    #[test]
    fn stream_commands_and_status() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.stream.load_sample(&mut app.toasts);
        app.dispatch(
            &AppCommand::RunReader {
                width: Some(8),
                reads: Some(3),
                wrap: Some(false),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.stream.insp.cursor(), (3, 0));
        // same counter mode: no reset, reads continue
        app.dispatch(
            &AppCommand::RunReader {
                width: None,
                reads: Some(2),
                wrap: Some(false),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.stream.insp.cursor(), (5, 0));
        // switching to the u8 counter restarts; 256 byte reads wrap to 0
        app.dispatch(
            &AppCommand::RunReader {
                width: None,
                reads: Some(256),
                wrap: Some(true),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.stream.insp.cursor().0, 0);
        assert!(app
            .dispatch(
                &AppCommand::RunReader {
                    width: Some(7),
                    reads: Some(1),
                    wrap: None
                },
                Origin::User
            )
            .is_err());
        app.dispatch(&AppCommand::ResetReader, Origin::User)
            .unwrap();
        assert_eq!(app.stream.insp.cursor(), (0, 0));
        // live needs an RTMP URL: a clear error, nothing started
        let e = app
            .dispatch(&AppCommand::SetStreaming { live: true }, Origin::User)
            .unwrap_err();
        assert!(e.contains("RTMP URL"), "{}", e);
        assert!(app
            .dispatch(
                &AppCommand::SetStreamTarget {
                    url: "http://x".into()
                },
                Origin::User
            )
            .is_err());
        assert!(!app.output.is_live());
        // Windows runners and Wine have no ffmpeg.exe: there the bus must
        // refuse clearly. Everywhere else ffmpeg is required and the real
        // recording below must run; a missing ffmpeg fails the test.
        if cfg!(windows) && output::find_ffmpeg().is_err() {
            assert!(app
                .dispatch(&AppCommand::SetRecording { recording: true }, Origin::User)
                .unwrap_err()
                .contains("ffmpeg was not found"));
            return;
        }
        output::find_ffmpeg()
            .expect("stream_commands_and_status needs ffmpeg (on PATH or SERAPHINA_FFMPEG)");
        // real recording of the Stream-mode bit view through the bus
        app.mode = Mode::Stream;
        let msg = app
            .dispatch(&AppCommand::SetRecording { recording: true }, Origin::User)
            .unwrap();
        assert!(msg.contains("recording 320×180"), "{}", msg);
        assert!(app.output.is_recording());
        let t0 = std::time::Instant::now();
        while t0.elapsed() < std::time::Duration::from_millis(1500) {
            run(&mut app, &ctx, 1, 0.0);
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        assert!(app.output.status().recording.unwrap() >= 1.4);
        assert!(
            app.dispatch(
                &AppCommand::SetOutputSource {
                    source: "screen".into(),
                    device: None,
                },
                Origin::User
            )
            .is_err(),
            "can't switch source mid-recording"
        );
        let msg = app
            .dispatch(&AppCommand::SetRecording { recording: false }, Origin::User)
            .unwrap();
        assert!(msg.contains("saved"), "{}", msg);
        let path = app.output.last_record.clone().unwrap();
        assert!(std::fs::metadata(&path).unwrap().len() > 1000);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn set_output_source_selects_a_camera_by_name() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        let src = |source: &str, device: Option<&str>| AppCommand::SetOutputSource {
            source: source.into(),
            device: device.map(Into::into),
        };
        let msg = app.dispatch(&src("camera", None), Origin::User).unwrap();
        assert_eq!(msg, "output source: Camera (first camera found)");
        let msg = app
            .dispatch(&src("webcam", Some(" C920 ")), Origin::User)
            .unwrap();
        assert_eq!(msg, "output source: Camera (C920)");
        assert_eq!(app.output.cfg.source, output::Source::Camera);
        assert_eq!(app.output.cfg.camera_device, "C920");
        // switching away keeps the chosen camera for next time
        app.dispatch(&src("canvas", None), Origin::User).unwrap();
        assert_eq!(app.output.cfg.camera_device, "C920");
        assert!(app
            .dispatch(&src("screen", Some("C920")), Origin::User)
            .unwrap_err()
            .contains("only the camera source takes a device"));
        assert!(app
            .dispatch(&src("radio", None), Origin::User)
            .unwrap_err()
            .contains("(canvas, screen, camera)"));
        // the AI tool spec offers the camera and its device
        let spec = commands::tool_specs()
            .into_iter()
            .find(|t| t.name == "set_output_source")
            .unwrap();
        let p = spec.params.to_string();
        assert!(p.contains("\"camera\"") && p.contains("\"device\""), "{p}");
    }

    #[test]
    fn open_missing_file_is_error_not_panic() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.stream.path = "definitely/missing.bin".into();
        app.stream.open(&mut app.toasts);
        assert!(app.toasts.items.iter().any(|t| t.kind == ToastKind::Error));
        assert!(app
            .dispatch(
                &AppCommand::OpenFile {
                    path: "definitely/missing.png".into()
                },
                Origin::User
            )
            .is_err());
    }

    /// Rewrites assets/mountain_spire.psd from the built-in sample document.
    #[test]
    #[ignore = "tool, not a test: rewrites assets/mountain_spire.psd"]
    fn regenerate_sample_psd() {
        let mut t = Toasts {
            now: 0.0,
            items: Vec::new(),
        };
        let p = paint::PaintState::new(&mut t);
        p.to_psd()
            .save(std::path::Path::new("assets/mountain_spire.psd"))
            .unwrap();
    }

    #[test]
    fn dodge_and_burn_do_not_build_up_while_paused() {
        for tool in [paint::Tool::Dodge, paint::Tool::Burn] {
            let mut t = Toasts {
                now: 0.0,
                items: Vec::new(),
            };
            let run = |frames: usize, t: &mut Toasts| {
                let mut p = paint::PaintState::new(t);
                p.doc.selected = p
                    .doc
                    .layers
                    .iter()
                    .position(|l| l.name == "Mountain Spire")
                    .unwrap();
                p.tool = tool;
                p.begin_stroke().unwrap();
                for _ in 0..frames {
                    p.stroke_to((300.0, 300.0));
                }
                // then a real movement still lays dabs at spacing
                p.stroke_to((340.0, 300.0));
                p.end_stroke();
                p.doc.layers[p.doc.selected].raster.data.clone()
            };
            let once = run(1, &mut t);
            let held = run(40, &mut t);
            assert!(once == held, "{:?} built up while the pointer paused", tool);
            let orig = paint::PaintState::new(&mut t);
            let i = orig
                .doc
                .layers
                .iter()
                .position(|l| l.name == "Mountain Spire")
                .unwrap();
            assert_ne!(
                orig.doc.layers[i].raster.data, once,
                "{:?} changed nothing",
                tool
            );
        }
    }

    /// The chat panel routes CLI intents to the real `seraphina` CLI.
    #[test]
    #[ignore = "needs the seraphina CLI on PATH (pip install seraphina-agi)"]
    fn chat_routes_memory_intents_to_the_real_cli() {
        let mut cli = llm::SeraphinaCli::detect().expect("seraphina CLI not found on PATH");
        let home = crate::test_dir::TestDir::new("chat_cli");
        cli.home = Some(home.path().to_path_buf());
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.assistant.set_cli(Some(cli));
        let say = |app: &mut App, text: &str| -> String {
            app.assistant.input = text.into();
            app.assistant.want_send = true;
            for _ in 0..400 {
                app.pump_assistant();
                if !app.assistant.is_busy() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            app.assistant
                .items
                .last()
                .map(|i| i.text.clone())
                .unwrap_or_default()
        };
        assert!(say(&mut app, "remember Mist Overlay looks best at 55%").contains("remembered"));
        assert!(say(&mut app, "recall mist").contains("Mist Overlay looks best at 55%"));
        assert!(say(&mut app, "triad tidy the layer stack").contains("consensus"));
        assert!(say(&mut app, "version").contains("seraphina"));
        assert!(say(&mut app, "help").contains("recall [query]"));
        crate::test_dir::fail_late();
    }

    #[test]
    fn bundled_psd_decodes_real_pixels() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        let msg = app
            .dispatch(
                &AppCommand::OpenFile {
                    path: "assets/mountain_spire.psd".into(),
                },
                Origin::User,
            )
            .unwrap();
        assert!(msg.contains("layers"), "{}", msg);
        let doc = &app.paint.doc;
        assert!(doc.width > 100 && doc.height > 100);
        assert!(doc.layers.len() >= 6, "{} layers", doc.layers.len());
        let find = |n: &str| {
            doc.layers
                .iter()
                .find(|l| l.name == n)
                .unwrap_or_else(|| panic!("missing layer {}", n))
        };
        // the photo layer is real, varied pixel data
        let base = find("Mountain Spire");
        let colours: std::collections::HashSet<[u8; 3]> = base
            .raster
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .step_by(7)
            .map(|p| [p[0], p[1], p[2]])
            .collect();
        assert!(colours.len() > 2000, "{} colours", colours.len());
        assert!(base
            .raster
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .all(|p| p[3] == 255));
        // properties survive
        let mist = find("Mist Overlay");
        assert_eq!(mist.blend, BlendMode::Screen);
        assert!((mist.opacity - 0.45).abs() < 0.01);
        assert!(
            mist.raster
                .data
                .as_chunks::<4>()
                .0
                .iter()
                .take(1000)
                .all(|p| p[3] == 0),
            "mask baked into alpha at the top"
        );
        assert_eq!(find("Rock Foreground").blend, BlendMode::Multiply);
        let env = doc
            .groups
            .iter()
            .find(|g| g.name == "Environment")
            .expect("group");
        assert_eq!(
            doc.layers
                .iter()
                .filter(|l| l.group == Some(env.id))
                .count(),
            2
        );
        // composite is not blank and matches the file's merged image
        let flat = app.paint.flattened();
        let file = obs_rust_bitstream::psd_pixels::PsdPixels::load(&resolve_asset(
            "assets/mountain_spire.psd",
        ))
        .unwrap();
        let merged = file.merged.unwrap();
        let diff: f64 = flat
            .data
            .iter()
            .zip(&merged)
            .map(|(a, b)| (*a as f64 - *b as f64).abs())
            .sum::<f64>()
            / flat.data.len() as f64;
        assert!(diff < 1.5, "mean diff {}", diff);
        let mean: f64 = flat.data.iter().map(|v| *v as f64).sum::<f64>() / flat.data.len() as f64;
        assert!(mean > 40.0, "composite isn't blank ({})", mean);
        // hiding a layer changes the composite
        let i = doc
            .layers
            .iter()
            .position(|l| l.name == "Mountain Spire")
            .unwrap();
        app.paint.doc.layers[i].visible = false;
        assert_ne!(app.paint.flattened().data, flat.data);
    }

    #[test]
    fn psd_save_roundtrips_through_open() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        let tmp = crate::test_dir::TestDir::new("psd");
        let path = tmp.path().join("rt.psd");
        let before = app.paint.flattened();
        app.paint.to_psd().save(&path).unwrap();
        app.dispatch(
            &AppCommand::OpenFile {
                path: path.to_string_lossy().into(),
            },
            Origin::User,
        )
        .unwrap();
        let after = app.paint.flattened();
        let diff: f64 = before
            .data
            .iter()
            .zip(&after.data)
            .map(|(a, b)| (*a as f64 - *b as f64).abs())
            .sum::<f64>()
            / before.data.len() as f64;
        assert!(diff < 1.5, "mean diff {}", diff);
        crate::test_dir::fail_late();
    }

    #[test]
    fn paint_layer_commands_and_undo() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        let n = app.paint.doc.layers.len();
        app.dispatch(
            &AppCommand::AddLayer {
                name: Some("Test".into()),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.paint.doc.layers.len(), n + 1);
        app.dispatch(
            &AppCommand::SetLayerOpacity {
                layer: Some("Mist Overlay".into()),
                opacity: 55.0,
            },
            Origin::User,
        )
        .unwrap();
        let i = app.paint.find_layer(Some("Mist Overlay")).unwrap();
        assert!((app.paint.doc.layers[i].opacity - 0.55).abs() < 1e-4);
        app.dispatch(
            &AppCommand::SetLayerVisibility {
                layer: Some("Mist Overlay".into()),
                visible: false,
            },
            Origin::User,
        )
        .unwrap();
        assert!(!app.paint.doc.layers[i].visible);
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(app.paint.doc.layers[i].visible);
        app.dispatch(
            &AppCommand::SelectLayer {
                layer: "Paint".into(),
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(
            &AppCommand::BrushStroke {
                points: vec![[100.0, 100.0], [300.0, 140.0]],
            },
            Origin::User,
        )
        .unwrap();
        let pi = app.paint.find_layer(Some("Paint")).unwrap();
        assert!(app.paint.doc.layers[pi]
            .raster
            .data
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] > 0));
        app.mode = Mode::Paint;
        run(&mut app, &ctx, 2, 0.0);
    }

    #[test]
    fn general_undo_for_3d_and_composite() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        let n0 = app.scene.objs.len();
        app.dispatch(
            &AppCommand::SceneAdd {
                kind: "cylinder".into(),
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(
            &AppCommand::SceneEdgeLoop {
                object: Some("cube1".into()),
                edge: None,
                t: None,
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.scene.objs[0].mesh.as_ref().unwrap().faces.len(), 10);
        // a drag = many transforms in quick succession -> one undo step
        for k in 0..20 {
            app.toasts.now = 10.0 + k as f64 * 0.016;
            app.dispatch(
                &AppCommand::SceneTransform {
                    object: Some("cube1".into()),
                    translate: Some([0.1, 0.0, 0.0]),
                    rotate_y: None,
                    scale: None,
                },
                Origin::User,
            )
            .unwrap();
        }
        assert!((app.scene.objs[0].pos[0] - 2.0).abs() < 1e-4);
        assert_eq!(app.scene_hist.depth(), (3, 0));
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(app.scene.objs[0].pos[0].abs() < 1e-4, "whole drag undone");
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert_eq!(
            app.scene.objs[0].mesh.as_ref().unwrap().faces.len(),
            6,
            "edge loop undone"
        );
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert_eq!(app.scene.objs.len(), n0, "add undone");
        assert!(app.dispatch(&AppCommand::Undo, Origin::User).is_err());
        app.dispatch(&AppCommand::Redo, Origin::User).unwrap();
        app.dispatch(&AppCommand::Redo, Origin::User).unwrap();
        assert_eq!(app.scene.objs.len(), n0 + 1);
        assert_eq!(app.scene.objs[0].mesh.as_ref().unwrap().faces.len(), 10);
        // failed edits leave no empty undo step
        let d = app.scene_hist.depth().0;
        assert!(app
            .dispatch(
                &AppCommand::SceneBoolean {
                    op: "xor".into(),
                    object: None,
                    with: None
                },
                Origin::User
            )
            .is_err());
        assert_eq!(app.scene_hist.depth().0, d);

        // Composite: opacity slider + blend change
        app.dispatch(
            &AppCommand::SwitchMode {
                mode: "composite".into(),
            },
            Origin::User,
        )
        .unwrap();
        let name = app.comp.layers[0].layer.name.clone();
        let (o0, b0) = (
            app.comp.layers[0].layer.opacity,
            app.comp.layers[0].layer.blend,
        );
        app.toasts.now = 20.0;
        app.dispatch(
            &AppCommand::SetLayerOpacity {
                layer: Some(name.clone()),
                opacity: 30.0,
            },
            Origin::User,
        )
        .unwrap();
        app.toasts.now = 20.1;
        app.dispatch(
            &AppCommand::SetLayerOpacity {
                layer: Some(name.clone()),
                opacity: 20.0,
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(
            &AppCommand::SetLayerBlend {
                layer: Some(name.clone()),
                mode: "multiply".into(),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.comp.layers[0].layer.blend, BlendMode::Multiply);
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert_eq!(app.comp.layers[0].layer.blend, b0);
        app.dispatch(&AppCommand::Undo, Origin::User).unwrap();
        assert!(
            (app.comp.layers[0].layer.opacity - o0).abs() < 1e-4,
            "slider drag undone in one step"
        );
        app.dispatch(&AppCommand::Redo, Origin::User).unwrap();
        assert!((app.comp.layers[0].layer.opacity - 0.2).abs() < 1e-3);
    }

    #[test]
    fn scene_commands() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.dispatch(
            &AppCommand::SceneAdd {
                kind: "cube".into(),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.mode, Mode::Scene);
        let i = app.scene.selected.unwrap();
        let f0 = app.scene.objs[i].mesh.as_ref().unwrap().faces.len();
        app.dispatch(
            &AppCommand::SceneExtrude {
                distance: Some(0.5),
            },
            Origin::User,
        )
        .unwrap();
        app.dispatch(&AppCommand::SceneBevel { amount: Some(0.2) }, Origin::User)
            .unwrap();
        let m = app.scene.objs[i].mesh.as_ref().unwrap();
        assert_eq!(m.faces.len(), f0 + 8);
        assert_eq!(m.verts.len(), 16);
        app.dispatch(
            &AppCommand::SceneGizmo {
                mode: "rotate".into(),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.current_tool(), "Rotate Tool");
        app.dispatch(
            &AppCommand::SceneTransform {
                object: None,
                translate: Some([1.0, 0.0, 0.0]),
                rotate_y: Some(45.0),
                scale: Some(2.0),
            },
            Origin::User,
        )
        .unwrap();
        assert_eq!(app.scene.objs[i].scale, 2.0);
        app.dispatch(&AppCommand::SetKeyframe, Origin::User)
            .unwrap();
        assert_eq!(app.scene.objs[i].track_y.keys().len(), 1);
        app.dispatch(&AppCommand::SceneDelete { object: None }, Origin::User)
            .unwrap();
        run(&mut app, &ctx, 2, 0.0);
    }

    #[test]
    fn scene_keys_drive_position() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.scene.clock.time = 1.5;
        app.scene.tick(0.0);
        assert!((app.scene.objs[0].pos[1] - 1.6).abs() < 1e-3);
    }

    #[test]
    fn timeline_commands_per_mode() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.mode = Mode::Paint;
        assert!(app
            .dispatch(&AppCommand::TimelineScrub { time: 1.0 }, Origin::User)
            .is_err());
        app.mode = Mode::Composite;
        app.dispatch(&AppCommand::TimelineScrub { time: 4.0 }, Origin::User)
            .unwrap();
        assert_eq!(app.comp.clock.time, 4.0);
        app.dispatch(
            &AppCommand::SetLayerOpacity {
                layer: Some("Mist Overlay".into()),
                opacity: 30.0,
            },
            Origin::User,
        )
        .unwrap();
        let i = app.comp.find(Some("Mist Overlay")).unwrap();
        assert!((app.comp.opacity_at(i, 4.0) - 0.3).abs() < 1e-4);
        app.dispatch(
            &AppCommand::TimelinePlay {
                playing: Some(true),
            },
            Origin::User,
        )
        .unwrap();
        assert!(app.comp.clock.playing);
        app.mode = Mode::Animate;
        let n = app.anim.frames.len();
        app.dispatch(&AppCommand::SetKeyframe, Origin::User)
            .unwrap();
        assert_eq!(app.anim.frames.len(), n + 1);
        app.dispatch(&AppCommand::DeleteKeyframe, Origin::User)
            .unwrap();
        assert_eq!(app.anim.frames.len(), n);
        app.dispatch(&AppCommand::TimelineScrub { time: 0.5 }, Origin::User)
            .unwrap();
        assert_eq!(app.anim.current(), 6);
        run(&mut app, &ctx, 3, 0.0);
    }

    #[test]
    fn composite_animates_opacity() {
        let ctx = egui::Context::default();
        let app = app_with(&ctx, None);
        let last = app.comp.layers.len() - 1;
        assert!(app.comp.opacity_at(last, 0.0) < app.comp.opacity_at(last, 4.0));
        assert_ne!(app.comp.frame(0.0).data, app.comp.frame(4.0).data);
    }

    #[test]
    fn offline_assistant_is_labelled() {
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, None);
        app.assistant.input = "how do I use this?".into();
        app.assistant.want_send = true;
        run(&mut app, &ctx, 1, 0.0);
        let last = app.assistant.items.last().unwrap();
        assert!(last
            .text
            .contains("Offline: no language model is configured"));
        assert_eq!(app.assistant.backend_label(), "Offline help");
    }

    #[test]
    fn ai_tool_calls_run_through_bus_and_undo_in_one_click() {
        let ctx = egui::Context::default();
        let replies = vec![
            LlmReply {
                content: "Done: the mist is softer and the sky is hidden.".into(),
                tool_calls: vec![],
                usage: None,
            },
            LlmReply {
                content: String::new(),
                tool_calls: vec![
                    call(
                        "set_layer_opacity",
                        r#"{"layer":"Mist Overlay","opacity":55}"#,
                    ),
                    call(
                        "set_layer_visibility",
                        r#"{"layer":"Sky Gradient","visible":false}"#,
                    ),
                    call("export_png", "{}"),
                    call("no_such_tool", "{}"),
                ],
                usage: None,
            },
        ];
        let mock = Arc::new(MockProvider::new(replies));
        let mut app = app_with(&ctx, Some(mock.clone()));
        let mist = app.paint.find_layer(Some("Mist Overlay")).unwrap();
        let sky = app.paint.find_layer(Some("Sky Gradient")).unwrap();
        let before = app.paint.doc.layers[mist].opacity;
        app.assistant.input = "soften the mist and hide the sky".into();
        app.assistant.want_send = true;
        wait_idle(&mut app, &ctx, 10);
        assert!((app.paint.doc.layers[mist].opacity - 0.55).abs() < 1e-4);
        assert!(!app.paint.doc.layers[sky].visible);
        let lines = action_lines(&app);
        let batch = lines[0].0;
        assert!(
            lines.iter().any(|(_, l)| l.status == LineStatus::Done
                && l.text.contains("Seraphina: set Mist Overlay opacity 55%")),
            "{:?}",
            lines
        );
        assert!(lines.iter().any(|(_, l)| l.status == LineStatus::NeedsUser));
        assert!(lines.iter().any(|(_, l)| l.status == LineStatus::Rejected));
        {
            let seen = mock.seen.lock().unwrap();
            assert_eq!(seen.len(), 2);
            assert!(seen[1].iter().any(|m| m.role == "tool"
                && m.content.starts_with("<<<DATA tool result ")
                && m.content.contains("\nok: ")));
            assert!(seen[1]
                .iter()
                .any(|m| m.role == "tool" && m.content.contains("\nblocked: ")));
            assert!(seen[0][0].content.contains("Current app state"));
        }
        assert!(app
            .assistant
            .items
            .iter()
            .any(|i| i.text.contains("the mist is softer")));
        let updates = app.assistant.prefs.bank.updates;
        assert!(app.undo_batch(batch));
        assert!((app.paint.doc.layers[mist].opacity - before).abs() < 1e-4);
        assert!(app.paint.doc.layers[sky].visible);
        // two undone actions × (per-command + overall acceptance) EMA keys
        assert_eq!(app.assistant.prefs.bank.updates, updates + 4);
        assert!(app.assistant.prefs.summary().contains("set_layer_opacity"));
    }

    #[test]
    fn approved_command_runs_from_card() {
        let ctx = egui::Context::default();
        let replies = vec![
            LlmReply {
                content: "Press Run to go live.".into(),
                tool_calls: vec![],
                usage: None,
            },
            LlmReply {
                content: String::new(),
                tool_calls: vec![call("set_streaming", r#"{"live":true}"#)],
                usage: None,
            },
        ];
        let mut app = app_with(&ctx, Some(Arc::new(MockProvider::new(replies))));
        app.assistant.input = "go live".into();
        app.assistant.want_send = true;
        wait_idle(&mut app, &ctx, 10);
        assert!(
            !app.output.is_live(),
            "mercy gate must hold irreversible actions"
        );
        assert!(!app.log.iter().any(|(_, n, _)| n == "set_streaming"));
        let lines = action_lines(&app);
        let k = lines
            .iter()
            .position(|(_, l)| l.status == LineStatus::NeedsUser)
            .unwrap();
        let (batch, l) = lines[k].clone();
        app.assistant.requests.push(PanelRequest::Run {
            cmd: l.cmd.unwrap(),
            batch,
            line: k,
        });
        run(&mut app, &ctx, 1, 0.0);
        // the approved command reached the real output (no RTMP URL configured in tests)
        let (_, n, r) = app.log.last().unwrap();
        assert_eq!(n, "set_streaming");
        assert!(r.as_ref().unwrap_err().contains("RTMP URL"), "{:?}", r);
    }

    #[test]
    fn stop_cancels_a_slow_round_and_frees_the_input() {
        let ctx = egui::Context::default();
        let replies = vec![LlmReply {
            content: String::new(),
            tool_calls: vec![call(
                "set_layer_opacity",
                r#"{"layer":"Mist Overlay","opacity":10}"#,
            )],
            usage: None,
        }];
        let mock = Arc::new(MockProvider::new(replies));
        *mock.delay.lock().unwrap() = Some(std::time::Duration::from_secs(30));
        let mut app = app_with(&ctx, Some(mock.clone()));
        let mist = app.paint.find_layer(Some("Mist Overlay")).unwrap();
        let before = app.paint.doc.layers[mist].opacity;
        app.assistant.input = "fade the mist".into();
        app.assistant.want_send = true;
        run(&mut app, &ctx, 2, 0.0);
        assert!(app.assistant.is_busy());
        let t0 = std::time::Instant::now();
        assert!(app.assistant.stop());
        assert!(!app.assistant.is_busy());
        assert!(!app.assistant.stop(), "nothing left to stop");
        // the worker notices quickly and nothing it returns is applied
        std::thread::sleep(std::time::Duration::from_millis(300));
        run(&mut app, &ctx, 3, 0.0);
        assert!(t0.elapsed() < std::time::Duration::from_secs(5));
        assert!((app.paint.doc.layers[mist].opacity - before).abs() < 1e-4);
        assert!(
            mock.seen.lock().unwrap().is_empty(),
            "the mock never answered"
        );
        assert!(app
            .assistant
            .items
            .iter()
            .any(|i| i.text.starts_with("Stopped. Nothing more will be sent")));
        // the next message goes through normally
        *mock.delay.lock().unwrap() = None;
        mock.replies.lock().unwrap().push(LlmReply {
            content: "Okay.".into(),
            tool_calls: vec![],
            usage: None,
        });
        app.assistant.input = "never mind".into();
        app.assistant.want_send = true;
        wait_idle(&mut app, &ctx, 10);
        assert!(app.assistant.items.iter().any(|i| i.text.contains("Okay.")));
    }

    fn system_prompt(m: &MockProvider, round: usize) -> String {
        m.seen.lock().unwrap()[round][0].content.clone()
    }

    #[test]
    fn first_send_asks_for_consent_and_respects_the_choice() {
        let ctx = egui::Context::default();
        let reply = |t: &str| LlmReply {
            content: t.into(),
            ..Default::default()
        };
        let mock = billed_mock(vec![reply("b"), reply("a")]);
        let mut app = app_with(&ctx, Some(mock.clone()));
        app.app_settings = settings::AppSettings::default();
        app.assistant.input = "hello".into();
        app.assistant.want_send = true;
        run(&mut app, &ctx, 3, 0.0);
        assert!(app.consent_open && mock.seen.lock().unwrap().is_empty());
        assert_eq!(app.assistant.input, "hello", "the message is kept");
        app.consent_choice(Consent::No);
        run(&mut app, &ctx, 3, 0.0);
        assert!(!app.consent_open && mock.seen.lock().unwrap().is_empty());
        assert!(app
            .assistant
            .items
            .iter()
            .any(|i| i.text.starts_with("Nothing was sent")));
        // asked again; this time only the messages go
        app.assistant.want_send = true;
        run(&mut app, &ctx, 2, 0.0);
        assert!(app.consent_open);
        app.consent_choice(Consent::MessagesOnly);
        wait_idle(&mut app, &ctx, 10);
        let sys = system_prompt(&mock, 0);
        assert!(
            !sys.contains("Active mode") && sys.contains("not shared"),
            "{sys}"
        );
        assert!(app.app_settings.ai_consent && !app.app_settings.share_app_state);
        // turning app state back on (Settings ▸ Privacy) sends it, no prompt
        app.app_settings.share_app_state = true;
        app.assistant.input = "again".into();
        app.assistant.want_send = true;
        wait_idle(&mut app, &ctx, 10);
        assert!(!app.consent_open);
        assert!(system_prompt(&mock, 1).contains("Active mode"));
    }

    #[test]
    fn local_models_need_no_consent() {
        let ctx = egui::Context::default();
        let mock = Arc::new(MockProvider::new(vec![LlmReply {
            content: "hi".into(),
            ..Default::default()
        }]));
        let mut app = app_with(&ctx, Some(mock.clone()));
        app.app_settings = settings::AppSettings::default();
        app.assistant.input = "hello".into();
        app.assistant.want_send = true;
        wait_idle(&mut app, &ctx, 10);
        assert!(!app.consent_open && mock.seen.lock().unwrap().len() == 1);
    }

    fn billed_mock(replies: Vec<LlmReply>) -> Arc<MockProvider> {
        let mut m = MockProvider::new(replies);
        m.billed = Some("grok-4.7".into());
        Arc::new(m)
    }

    fn dollars(usd: f64) -> usage::Usage {
        usage::Usage {
            ticks: (usd * usage::TICKS_PER_USD) as u64,
            ..Default::default()
        }
    }

    #[test]
    fn rounds_are_metered_with_xai_costs_or_estimates() {
        let ctx = egui::Context::default();
        let replies = vec![
            LlmReply {
                content: "Second.".into(),
                tool_calls: vec![],
                usage: None,
            },
            LlmReply {
                content: "First.".into(),
                tool_calls: vec![],
                usage: Some(usage::Usage {
                    prompt_tokens: 900,
                    completion_tokens: 10,
                    ticks: 20_000_000,
                    estimated: false,
                }),
            },
        ];
        let mut app = app_with(&ctx, Some(billed_mock(replies)));
        for msg in ["one", "two"] {
            app.assistant.input = msg.into();
            app.assistant.want_send = true;
            wait_idle(&mut app, &ctx, 10);
        }
        run(&mut app, &ctx, 2, 0.0);
        let s = &app.meter.session;
        assert_eq!((s.calls, s.estimated_calls), (2, 1), "{s:?}");
        assert!(s.usd_ticks > 20_000_000 && s.prompt_tokens > 900);
        assert_eq!(&app.meter.project, s);
        // saved with the project, cleared by New
        let c = app.to_container(false, None);
        assert_eq!(c.manifest["usage"]["calls"], 2);
        app.new_project();
        assert_eq!(app.meter.project, usage::Totals::default());
        assert_eq!(app.meter.session.calls, 2, "the session keeps counting");
        // an unbilled provider is not metered
        let mut app = app_with(&ctx, Some(Arc::new(MockProvider::new(vec![]))));
        app.assistant.input = "free".into();
        app.assistant.want_send = true;
        wait_idle(&mut app, &ctx, 10);
        assert_eq!(app.meter.session.calls, 0);
    }

    #[test]
    fn spend_cap_blocks_chat_and_imagine_with_a_toast() {
        let ctx = egui::Context::default();
        let mock = billed_mock(vec![LlmReply {
            content: "should not be sent".into(),
            ..Default::default()
        }]);
        let mut app = app_with(&ctx, Some(mock.clone()));
        app.meter.cap_usd = 0.50;
        app.meter.record(&dollars(0.60));
        app.assistant.input = "make it pop".into();
        app.assistant.want_send = true;
        wait_idle(&mut app, &ctx, 5);
        assert!(mock.seen.lock().unwrap().is_empty(), "nothing went to xAI");
        assert!(app
            .toasts
            .items
            .iter()
            .any(|t| matches!(t.kind, ToastKind::Warn)
                && t.text.starts_with("Spend cap reached ($0.60 of $0.50")));
        assert!(app
            .assistant
            .items
            .iter()
            .any(|i| i.text.contains("Settings")));

        struct Free;
        impl videogen::VideoGenProvider for Free {
            fn name(&self) -> String {
                "free".into()
            }
            fn start_generate(
                &self,
                _: &str,
                _: u32,
                _: &std::sync::atomic::AtomicBool,
            ) -> Result<String, String> {
                Err("must not be called".into())
            }
            fn start_extend(
                &self,
                _: &str,
                _: &str,
                _: &std::sync::atomic::AtomicBool,
            ) -> Result<String, String> {
                Err("must not be called".into())
            }
            fn poll(
                &self,
                _: &str,
                _: &std::sync::atomic::AtomicBool,
            ) -> Result<videogen::JobStatus, String> {
                Ok(videogen::JobStatus::Pending)
            }
        }
        app.video.provider = Some(Arc::new(Free));
        let e = app
            .dispatch(
                &AppCommand::VideoGenerate {
                    prompt: "a cat".into(),
                    duration: Some(1),
                },
                Origin::User,
            )
            .unwrap_err();
        assert!(e.starts_with("Spend cap reached"), "{e}");
        assert!(app.video.clips.is_empty());
        // raising the cap lets work through again
        app.meter.cap_usd = 5.0;
        assert!(app
            .dispatch(
                &AppCommand::VideoGenerate {
                    prompt: "a cat".into(),
                    duration: Some(1),
                },
                Origin::User
            )
            .is_ok());
    }

    /// Live, billed: real xAI tool calling through the bus. Run with
    /// `cargo test --bin ui -- --ignored live_xai_tool_calling_smoke`.
    #[test]
    #[ignore = "live, billed: needs XAI_API_KEY or SERAPHINA_GROK_API_KEY"]
    fn live_xai_tool_calling_smoke() {
        assert!(
            ["XAI_API_KEY", "SERAPHINA_GROK_API_KEY"]
                .iter()
                .any(|k| std::env::var_os(k).is_some()),
            "live_xai_tool_calling_smoke needs XAI_API_KEY or SERAPHINA_GROK_API_KEY"
        );
        let p = llm::OpenAiCompat::from_env().expect("provider from env");
        let ctx = egui::Context::default();
        let mut app = app_with(&ctx, Some(Arc::new(p)));
        app.assistant.input =
            "Set Mist Overlay opacity to 55% and switch to Composite mode.".into();
        app.assistant.want_send = true;
        wait_idle(&mut app, &ctx, 180);
        let lines = action_lines(&app);
        for (_, l) in &lines {
            eprintln!("LIVE action: {:?} {}", l.status, l.text);
        }
        for it in &app.assistant.items {
            eprintln!("LIVE chat: {}", it.text.replace('\n', " "));
        }
        let mist = app.paint.find_layer(Some("Mist Overlay")).unwrap();
        assert!(
            (app.paint.doc.layers[mist].opacity - 0.55).abs() < 0.011,
            "opacity {}",
            app.paint.doc.layers[mist].opacity
        );
        assert_eq!(app.mode, Mode::Composite);
        assert!(app
            .log
            .iter()
            .any(|(o, n, r)| *o == Origin::Seraphina && n == "set_layer_opacity" && r.is_ok()));
    }

    #[test]
    fn toasts_expire() {
        let mut t = Toasts::default();
        t.push(ToastKind::Info, "hi");
        t.now = 10.0;
        let ctx = egui::Context::default();
        let _ = ctx.run(input(10.0), |ctx| t.show(ctx, &Palette::dark()));
        assert!(t.items.is_empty());
    }
}

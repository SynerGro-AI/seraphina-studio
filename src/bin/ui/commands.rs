//! Central typed command bus. Every app action is an `AppCommand`.
//! Buttons push commands; Seraphina's tool calls are parsed into the
//! same enum and executed through `App::dispatch`, so the user and the
//! AI share one path (and one undo story).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Who issued a command.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    User,
    Seraphina,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum AppCommand {
    // ---- shell
    SwitchMode {
        mode: String,
    },
    SetTheme {
        dark: bool,
    },
    /// Whole-UI scale, 80..=200 percent (saved).
    SetUiScale {
        percent: f32,
    },
    TogglePanel {
        panel: String,
        visible: Option<bool>,
    },
    // ---- paint: tools & brush
    SelectTool {
        tool: String,
    },
    SetBrush {
        size: Option<f32>,
        hardness: Option<f32>,
        flow: Option<f32>,
        color: Option<String>,
        exposure: Option<f32>,
    },
    BrushStroke {
        points: Vec<[f32; 2]>,
    },
    Gradient {
        x0: f32,
        y0: f32,
        x1: f32,
        y1: f32,
    },
    SetCloneSource {
        x: f32,
        y: f32,
    },
    // ---- paint: layers
    AddLayer {
        name: Option<String>,
    },
    AddGroup {
        name: Option<String>,
    },
    DuplicateLayer {
        layer: Option<String>,
    },
    RemoveLayer {
        layer: Option<String>,
    },
    MoveLayer {
        layer: Option<String>,
        direction: String,
    },
    SelectLayer {
        layer: String,
    },
    RenameLayer {
        layer: Option<String>,
        name: String,
    },
    SetLayerOpacity {
        layer: Option<String>,
        opacity: f32,
    },
    SetLayerFill {
        layer: Option<String>,
        fill: f32,
    },
    SetLayerBlend {
        layer: Option<String>,
        mode: String,
    },
    SetLayerVisibility {
        layer: Option<String>,
        visible: bool,
    },
    SetLayerLocked {
        layer: Option<String>,
        locked: bool,
    },
    OffsetLayer {
        layer: Option<String>,
        dx: i32,
        dy: i32,
    },
    SetGroupVisibility {
        group: String,
        visible: bool,
    },
    // ---- paint: masks & selection
    AddMask {
        layer: Option<String>,
    },
    SetMask {
        layer: Option<String>,
        density: Option<f32>,
        feather: Option<f32>,
        invert: Option<bool>,
        enabled: Option<bool>,
    },
    RemoveMask {
        layer: Option<String>,
    },
    SelectRect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    SelectPolygon {
        points: Vec<[f32; 2]>,
    },
    MagicWand {
        x: f32,
        y: f32,
        tolerance: Option<u8>,
    },
    SelectAll,
    Deselect,
    InvertSelection,
    // ---- paint: image ops
    ApplyFilter {
        filter: String,
        strength: Option<f32>,
    },
    Crop {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
    AddText {
        text: String,
        x: f32,
        y: f32,
        size: Option<f32>,
        color: Option<String>,
    },
    PenAddPoint {
        x: f32,
        y: f32,
    },
    PenStroke,
    PenToSelection,
    PenClear,
    Undo,
    Redo,
    Zoom {
        percent: Option<f32>,
        fit: Option<bool>,
    },
    Pan {
        dx: f32,
        dy: f32,
    },
    OpenFile {
        path: String,
    },
    /// Layered PSD. No path: the user picks one in a Save dialog. A
    /// relative path is resolved against Documents/Seraphina.
    SavePsd {
        #[serde(default)]
        path: Option<String>,
    },
    /// Flattened PNG; `path` as for `SavePsd`.
    ExportPng {
        #[serde(default)]
        path: Option<String>,
    },
    /// Save the whole workspace as a `.seraphina` project. No path (or
    /// `save_as`) asks the user where.
    SaveProject {
        #[serde(default)]
        path: Option<String>,
        #[serde(default)]
        save_as: bool,
    },
    /// Open a `.seraphina` project (asks first if there are unsaved changes).
    OpenProject {
        #[serde(default)]
        path: Option<String>,
    },
    /// Start a new, empty project (asks first if there are unsaved changes).
    NewProject,
    // ---- 3D
    SceneAdd {
        kind: String,
    },
    SceneSelect {
        object: String,
    },
    SceneGizmo {
        mode: String,
    },
    SceneTransform {
        object: Option<String>,
        translate: Option<[f32; 3]>,
        rotate_y: Option<f32>,
        scale: Option<f32>,
    },
    SceneExtrude {
        distance: Option<f32>,
    },
    SceneBevel {
        amount: Option<f32>,
    },
    SceneDelete {
        object: Option<String>,
    },
    SceneEdgeLoop {
        object: Option<String>,
        edge: Option<[usize; 2]>,
        t: Option<f32>,
    },
    SceneMultiCut {
        object: Option<String>,
        face: Option<usize>,
        from_edge: Option<usize>,
        to_edge: Option<usize>,
        from_t: Option<f32>,
        to_t: Option<f32>,
    },
    SceneBridge {
        object: Option<String>,
        with: Option<String>,
    },
    SceneBoolean {
        op: String,
        object: Option<String>,
        with: Option<String>,
    },
    SceneQuadDraw {
        points: Vec<[f32; 3]>,
    },
    SceneShading {
        mode: String,
    },
    // ---- materials lab
    MaterialsSelect {
        element: String,
    },
    MaterialsSearch {
        query: String,
    },
    MaterialsAdd {
        element: String,
        count: Option<u32>,
    },
    MaterialsRemove {
        element: String,
    },
    MaterialsClear,
    MaterialsBuild {
        formula: Option<String>,
    },
    MaterialsBuildAtom {
        protons: u32,
        neutrons: u32,
        electrons: u32,
    },
    MaterialsPreset {
        preset: String,
    },
    MaterialsSave {
        name: Option<String>,
    },
    MaterialAssign {
        object: Option<String>,
        material: String,
    },
    // ---- physics arena
    ArenaPlay {
        playing: Option<bool>,
    },
    ArenaStep {
        steps: Option<u32>,
    },
    ArenaReset,
    ArenaEnv {
        preset: Option<String>,
        gravity: Option<f32>,
        air_density: Option<f32>,
        temperature_c: Option<f32>,
        pressure_kpa: Option<f32>,
        wind: Option<[f32; 3]>,
    },
    ArenaBody {
        object: Option<String>,
        temperature_c: Option<f32>,
        heater_w: Option<f32>,
        fixed: Option<bool>,
        velocity: Option<[f32; 3]>,
    },
    // ---- lab: reactions, circuits, computer, relativity (simulations; not document edits)
    LabTab {
        tab: String,
    },
    ChemReact {
        equation: Option<String>,
        temperature_c: Option<f64>,
        pressure_kpa: Option<f64>,
        seconds: Option<f64>,
        air: Option<bool>,
    },
    CircuitRun {
        preset: Option<String>,
        voltage: Option<f64>,
        temperature_c: Option<f64>,
        material: Option<String>,
        length_mm: Option<f64>,
        diameter_mm: Option<f64>,
        doping_cm3: Option<f64>,
    },
    CpuRun {
        program: Option<String>,
        source: Option<String>,
        temperature_c: Option<f64>,
        clock_mhz: Option<f64>,
        die_object: Option<String>,
    },
    CpuHeatSweep {
        program: Option<String>,
        clock_mhz: Option<f64>,
    },
    RelSpeed {
        beta: Option<f64>,
        speed_kms: Option<f64>,
    },
    NbodyPreset {
        preset: Option<String>,
        gr: Option<bool>,
        gr_exaggeration: Option<f64>,
    },
    NbodyPlay {
        playing: Option<bool>,
        days_per_second: Option<f64>,
    },
    NbodyMeasurePrecession,
    // ---- timeline
    TimelinePlay {
        playing: Option<bool>,
    },
    TimelineScrub {
        time: f32,
    },
    SetKeyframe,
    DeleteKeyframe,
    // ---- stream
    RunReader {
        width: Option<u32>,
        reads: Option<usize>,
        wrap: Option<bool>,
    },
    ResetReader,
    SetStreaming {
        live: bool,
    },
    SetRecording {
        recording: bool,
    },
    RetryStream,
    SetOutputSource {
        source: String,
    },
    SetStreamTarget {
        url: String,
    },
    /// UI only: never exposed as a tool, never logged with its value.
    SetStreamKey {
        key: String,
    },
    // ---- video generation
    VideoGenerate {
        prompt: String,
        duration: Option<u32>,
    },
    GenerateImage {
        prompt: String,
    },
    VideoExtend {
        clip: usize,
        prompt: String,
    },
    VideoStitch {
        clips: Option<Vec<usize>>,
    },
}

impl AppCommand {
    pub fn name(&self) -> String {
        serde_json::to_value(self)
            .ok()
            .and_then(|v| v.get("cmd").and_then(|c| c.as_str()).map(|s| s.to_string()))
            .unwrap_or_else(|| "unknown".into())
    }

    /// Whether this command changes document state (and so belongs in
    /// an AI action's undo snapshot). View-only commands return false.
    pub fn mutates(&self) -> bool {
        !matches!(
            self,
            AppCommand::Zoom { .. }
                | AppCommand::Pan { .. }
                | AppCommand::MaterialsSelect { .. }
                | AppCommand::MaterialsSearch { .. }
                | AppCommand::LabTab { .. }
                | AppCommand::ChemReact { .. }
                | AppCommand::CircuitRun { .. }
                | AppCommand::CpuRun { .. }
                | AppCommand::CpuHeatSweep { .. }
                | AppCommand::RelSpeed { .. }
                | AppCommand::NbodyPreset { .. }
                | AppCommand::NbodyPlay { .. }
                | AppCommand::NbodyMeasurePrecession
                | AppCommand::SetTheme { .. }
                | AppCommand::SetUiScale { .. }
                | AppCommand::TogglePanel { .. }
                | AppCommand::SavePsd { .. }
                | AppCommand::ExportPng { .. }
                | AppCommand::OpenFile { .. }
                | AppCommand::SaveProject { .. }
                | AppCommand::OpenProject { .. }
                | AppCommand::NewProject
        )
    }

    /// Commands Seraphina may not run without the user (file writes and
    /// paid/networked generation). The Triad mercy wheel blocks these.
    pub fn needs_user(&self) -> bool {
        matches!(
            self,
            AppCommand::OpenFile { .. }
                | AppCommand::SavePsd { .. }
                | AppCommand::ExportPng { .. }
                | AppCommand::SaveProject { .. }
                | AppCommand::OpenProject { .. }
                | AppCommand::NewProject
                | AppCommand::VideoGenerate { .. }
                | AppCommand::GenerateImage { .. }
                | AppCommand::VideoExtend { .. }
                | AppCommand::VideoStitch { .. }
                | AppCommand::SetStreaming { live: true }
                | AppCommand::SetRecording { recording: true }
                | AppCommand::SetStreamTarget { .. }
                | AppCommand::SetStreamKey { .. }
                | AppCommand::RetryStream
        )
    }

    /// Project save/open, which `--run` scripts may use when started with
    /// `--allow-file-commands`.
    pub fn is_project_file(&self) -> bool {
        matches!(
            self,
            AppCommand::SaveProject { path: Some(_), .. }
                | AppCommand::OpenProject { path: Some(_) }
                | AppCommand::SavePsd { path: Some(_) }
                | AppCommand::ExportPng { path: Some(_) }
        )
    }
}

/// Parse an OpenAI-style tool call (`name`, JSON `arguments`).
pub fn parse_tool_call(name: &str, arguments: &str) -> Result<AppCommand, String> {
    let args: Value = if arguments.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(arguments).map_err(|e| format!("arguments are not JSON: {}", e))?
    };
    let mut obj = match args {
        Value::Object(m) => m,
        Value::Null => serde_json::Map::new(),
        _ => return Err("arguments must be a JSON object".into()),
    };
    if !tool_specs().iter().any(|t| t.name == name) {
        return Err(format!("unknown tool '{}'", name));
    }
    obj.insert("cmd".into(), Value::String(name.to_string()));
    serde_json::from_value(Value::Object(obj)).map_err(|e| format!("{}: {}", name, e))
}

pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub params: Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": props, "required": required })
}

const LAYER: &str =
    "Layer name (case-insensitive) or 1-based index from the top. Omit for the selected layer.";

/// JSON-schema tool definitions for every command.
pub fn tool_specs() -> Vec<ToolSpec> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    let n = |d: &str| json!({ "type": "number", "description": d });
    let b = |d: &str| json!({ "type": "boolean", "description": d });
    let pts = json!({ "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2 }, "description": "Image-space [x, y] points" });
    vec![
        ToolSpec { name: "switch_mode", description: "Switch workspace mode.", params: obj(json!({ "mode": { "type": "string", "enum": ["paint", "composite", "3d", "animate", "stream", "materials"] } }), &["mode"]) },
        ToolSpec { name: "set_theme", description: "Dark or light appearance.", params: obj(json!({ "dark": b("true for dark") }), &["dark"]) },
        ToolSpec { name: "set_ui_scale", description: "Make the whole interface bigger or smaller (saved for next time).", params: obj(json!({ "percent": n("80 to 200; 100 is normal") }), &["percent"]) },
        ToolSpec { name: "toggle_panel", description: "Show or hide a panel.", params: obj(json!({ "panel": { "type": "string", "enum": ["tools", "inspector", "timeline", "assistant", "videogen", "shelf"] }, "visible": b("omit to toggle") }), &["panel"]) },
        ToolSpec { name: "select_tool", description: "Pick a Paint tool.", params: obj(json!({ "tool": { "type": "string", "enum": ["move", "marquee", "lasso", "quick_select", "crop", "healing", "clone", "brush", "eraser", "gradient", "dodge", "burn", "pen", "type", "hand", "zoom", "eyedropper"] } }), &["tool"]) },
        ToolSpec { name: "set_brush", description: "Brush parameters. Any subset.", params: obj(json!({ "size": n("diameter px 1-500"), "hardness": n("0-100 %"), "flow": n("1-100 %"), "color": s("#RRGGBB"), "exposure": n("dodge/burn exposure 1-100 %") }), &[]) },
        ToolSpec { name: "brush_stroke", description: "Stroke the current paint tool (brush, eraser, dodge, burn, clone, healing) through points on the selected layer.", params: obj(json!({ "points": pts.clone() }), &["points"]) },
        ToolSpec { name: "gradient", description: "Linear gradient from the brush colour to transparent on the selected layer, inside the selection.", params: obj(json!({ "x0": n("start x"), "y0": n("start y"), "x1": n("end x"), "y1": n("end y") }), &["x0", "y0", "x1", "y1"]) },
        ToolSpec { name: "set_clone_source", description: "Clone stamp source point.", params: obj(json!({ "x": n("x"), "y": n("y") }), &["x", "y"]) },
        ToolSpec { name: "add_layer", description: "New empty raster layer above the selected one.", params: obj(json!({ "name": s("layer name") }), &[]) },
        ToolSpec { name: "add_group", description: "New group (folder) containing the selected layer.", params: obj(json!({ "name": s("group name") }), &[]) },
        ToolSpec { name: "duplicate_layer", description: "Duplicate a layer.", params: obj(json!({ "layer": s(LAYER) }), &[]) },
        ToolSpec { name: "remove_layer", description: "Delete a layer.", params: obj(json!({ "layer": s(LAYER) }), &[]) },
        ToolSpec { name: "move_layer", description: "Reorder a layer.", params: obj(json!({ "layer": s(LAYER), "direction": { "type": "string", "enum": ["up", "down", "top", "bottom"] } }), &["direction"]) },
        ToolSpec { name: "select_layer", description: "Make a layer the active layer.", params: obj(json!({ "layer": s(LAYER) }), &["layer"]) },
        ToolSpec { name: "rename_layer", description: "Rename a layer.", params: obj(json!({ "layer": s(LAYER), "name": s("new name") }), &["name"]) },
        ToolSpec { name: "set_layer_opacity", description: "Layer opacity in percent.", params: obj(json!({ "layer": s(LAYER), "opacity": n("0-100") }), &["opacity"]) },
        ToolSpec { name: "set_layer_fill", description: "Layer fill in percent.", params: obj(json!({ "layer": s(LAYER), "fill": n("0-100") }), &["fill"]) },
        ToolSpec { name: "set_layer_blend", description: "Layer blend mode.", params: obj(json!({ "layer": s(LAYER), "mode": { "type": "string", "enum": ["normal", "multiply", "screen", "overlay", "darken", "lighten", "soft light", "difference"] } }), &["mode"]) },
        ToolSpec { name: "set_layer_visibility", description: "Show or hide a layer.", params: obj(json!({ "layer": s(LAYER), "visible": b("visible") }), &["visible"]) },
        ToolSpec { name: "set_layer_locked", description: "Lock or unlock a layer's pixels.", params: obj(json!({ "layer": s(LAYER), "locked": b("locked") }), &["locked"]) },
        ToolSpec { name: "offset_layer", description: "Move a layer's pixels by dx, dy.", params: obj(json!({ "layer": s(LAYER), "dx": n("pixels"), "dy": n("pixels") }), &["dx", "dy"]) },
        ToolSpec { name: "set_group_visibility", description: "Show or hide a layer group.", params: obj(json!({ "group": s("group name"), "visible": b("visible") }), &["group", "visible"]) },
        ToolSpec { name: "add_mask", description: "Add a layer mask from the current selection (or reveal-all).", params: obj(json!({ "layer": s(LAYER) }), &[]) },
        ToolSpec { name: "set_mask", description: "Edit the layer mask.", params: obj(json!({ "layer": s(LAYER), "density": n("0-100 %"), "feather": n("0-100 px"), "invert": b("invert"), "enabled": b("enabled") }), &[]) },
        ToolSpec { name: "remove_mask", description: "Delete the layer mask.", params: obj(json!({ "layer": s(LAYER) }), &[]) },
        ToolSpec { name: "select_rect", description: "Rectangular marquee selection in image pixels.", params: obj(json!({ "x": n("x"), "y": n("y"), "w": n("width"), "h": n("height") }), &["x", "y", "w", "h"]) },
        ToolSpec { name: "select_polygon", description: "Lasso/polygon selection.", params: obj(json!({ "points": pts.clone() }), &["points"]) },
        ToolSpec { name: "magic_wand", description: "Quick-select similar colours from a seed pixel.", params: obj(json!({ "x": n("x"), "y": n("y"), "tolerance": n("0-255, default 32") }), &["x", "y"]) },
        ToolSpec { name: "select_all", description: "Select the whole canvas.", params: obj(json!({}), &[]) },
        ToolSpec { name: "deselect", description: "Clear the selection.", params: obj(json!({}), &[]) },
        ToolSpec { name: "invert_selection", description: "Invert the selection.", params: obj(json!({}), &[]) },
        ToolSpec { name: "apply_filter", description: "Apply an adjustment to the selected layer (inside the selection).", params: obj(json!({ "filter": { "type": "string", "enum": ["levels", "curves", "grayscale", "invert", "blur"] }, "strength": n("levels gain 0.5-2 or blur radius px") }), &["filter"]) },
        ToolSpec { name: "crop", description: "Crop the document.", params: obj(json!({ "x": n("x"), "y": n("y"), "w": n("width"), "h": n("height") }), &["x", "y", "w", "h"]) },
        ToolSpec { name: "add_text", description: "Create a text layer.", params: obj(json!({ "text": s("text"), "x": n("x"), "y": n("baseline-top y"), "size": n("px, default 48"), "color": s("#RRGGBB") }), &["text", "x", "y"]) },
        ToolSpec { name: "pen_add_point", description: "Append a point to the pen path.", params: obj(json!({ "x": n("x"), "y": n("y") }), &["x", "y"]) },
        ToolSpec { name: "pen_stroke", description: "Stroke the pen path with the brush on the selected layer.", params: obj(json!({}), &[]) },
        ToolSpec { name: "pen_to_selection", description: "Turn the pen path into a selection.", params: obj(json!({}), &[]) },
        ToolSpec { name: "pen_clear", description: "Clear the pen path.", params: obj(json!({}), &[]) },
        ToolSpec { name: "undo", description: "Undo the last Paint edit.", params: obj(json!({}), &[]) },
        ToolSpec { name: "redo", description: "Redo.", params: obj(json!({}), &[]) },
        ToolSpec { name: "zoom", description: "Set Paint zoom.", params: obj(json!({ "percent": n("5-1600"), "fit": b("fit to window") }), &[]) },
        ToolSpec { name: "pan", description: "Pan the Paint view in screen points.", params: obj(json!({ "dx": n("dx"), "dy": n("dy") }), &["dx", "dy"]) },
        ToolSpec { name: "open_file", description: "Open an image as a new layer, or read a PSD header (user approval required).", params: obj(json!({ "path": s("file path") }), &["path"]) },
        ToolSpec { name: "save_psd", description: "Save the document as a layered PSD with pixels (user approval required). Without a path the user picks one in a Save dialog (suggesting <name>_edit.psd in Documents/Seraphina); relative paths go under Documents/Seraphina.", params: obj(json!({"path": {"type": "string", "description": "optional file path"}}), &[]) },
        ToolSpec { name: "save_project", description: "Save every mode (Paint, Animate, Composite, 3D, Materials, Stream settings, optionally this chat) as a .seraphina project (user approval required). Omit path to save to the current project or ask the user.", params: obj(json!({ "path": s("file path; .seraphina is added"), "save_as": b("always ask where to save") }), &[]) },
        ToolSpec { name: "open_project", description: "Open a .seraphina project, replacing the workspace; the user is asked about unsaved changes (user approval required).", params: obj(json!({ "path": s("file path; omit to ask the user") }), &[]) },
        ToolSpec { name: "new_project", description: "Start a new project; the user is asked about unsaved changes (user approval required).", params: obj(json!({}), &[]) },
        ToolSpec { name: "export_png", description: "Export a flattened PNG (user approval required). Without a path the user picks one in a Save dialog; relative paths go under Documents/Seraphina.", params: obj(json!({"path": {"type": "string", "description": "optional file path"}}), &[]) },
        ToolSpec { name: "scene_add", description: "Add a 3D object.", params: obj(json!({ "kind": { "type": "string", "enum": ["cube", "plane", "cylinder", "joint", "light", "camera"] } }), &["kind"]) },
        ToolSpec { name: "scene_select", description: "Select a 3D object by name.", params: obj(json!({ "object": s("object name") }), &["object"]) },
        ToolSpec { name: "scene_gizmo", description: "3D viewport drag mode.", params: obj(json!({ "mode": { "type": "string", "enum": ["select", "move", "rotate", "scale", "edge_loop", "multi_cut", "quad_draw"] } }), &["mode"]) },
        ToolSpec { name: "scene_transform", description: "Translate (relative), rotate Y (absolute degrees) or scale (absolute) a 3D object.", params: obj(json!({ "object": s("object name; omit for selected"), "translate": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 }, "rotate_y": n("degrees"), "scale": n("uniform scale") }), &[]) },
        ToolSpec { name: "scene_extrude", description: "Extrude the selected mesh's top face.", params: obj(json!({ "distance": n("units, default 0.5") }), &[]) },
        ToolSpec { name: "scene_bevel", description: "Bevel (chamfer) the selected mesh's top face.", params: obj(json!({ "amount": n("0.05-0.45, default 0.15") }), &[]) },
        ToolSpec { name: "scene_delete", description: "Delete a 3D object.", params: obj(json!({ "object": s("object name; omit for selected") }), &[]) },
        ToolSpec { name: "scene_edge_loop", description: "Insert an edge loop through the quad ring crossing an edge (default: the most vertical quad edge).", params: obj(json!({ "object": s("object; omit for selected"), "edge": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 2, "description": "vertex indices [a, b]" }, "t": n("position along the edge 0-1, default 0.5") }), &[]) },
        ToolSpec { name: "scene_multi_cut", description: "Split one face with a cut between two of its edges (default: top face, edges 0 and 2 at their midpoints).", params: obj(json!({ "object": s("object; omit for selected"), "face": n("face index"), "from_edge": n("edge index within the face"), "to_edge": n("edge index within the face"), "from_t": n("0-1"), "to_t": n("0-1") }), &[]) },
        ToolSpec { name: "scene_bridge", description: "Merge another mesh into this one and bridge their two facing faces with quads.", params: obj(json!({ "object": s("object; omit for selected"), "with": s("other mesh; omit for the previously selected one") }), &[]) },
        ToolSpec { name: "scene_boolean", description: "Boolean of two meshes (the result replaces `object`; `with` is removed).", params: obj(json!({ "op": { "type": "string", "enum": ["union", "difference", "intersection"] }, "object": s("object; omit for selected"), "with": s("other mesh; omit for the previously selected one") }), &["op"]) },
        ToolSpec { name: "scene_quad_draw", description: "Add a quad (4 world-space points) to the quad-draw mesh, welding shared corners.", params: obj(json!({ "points": { "type": "array", "items": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3 }, "minItems": 4, "maxItems": 4 } }), &["points"]) },
        ToolSpec { name: "scene_shading", description: "Viewport display mode.", params: obj(json!({ "mode": { "type": "string", "enum": ["shaded", "wireframe", "shaded_wire"] } }), &["mode"]) },
        ToolSpec { name: "materials_select", description: "Show an element's detail card in the Materials Lab.", params: obj(json!({ "element": s("symbol, name or atomic number") }), &["element"]) },
        ToolSpec { name: "materials_search", description: "Filter the periodic table.", params: obj(json!({ "query": s("symbol, name fragment or atomic number; empty clears") }), &["query"]) },
        ToolSpec { name: "materials_add", description: "Add atoms of an element to the composition builder.", params: obj(json!({ "element": s("symbol, name or atomic number"), "count": n("atoms, default 1") }), &["element"]) },
        ToolSpec { name: "materials_remove", description: "Remove one atom of an element from the composition builder.", params: obj(json!({ "element": s("symbol, name or atomic number") }), &["element"]) },
        ToolSpec { name: "materials_clear", description: "Clear the composition builder.", params: obj(json!({}), &[]) },
        ToolSpec { name: "materials_build", description: "Build a material from a chemical formula (e.g. H2O, Fe2O3, Ca(OH)2, CuZn) or the builder's composition: checks nuclear stability and charge balance, derives PBR and physical properties.", params: obj(json!({ "formula": s("formula; omit to use the builder composition") }), &[]) },
        ToolSpec { name: "materials_build_atom", description: "Build an element from protons, neutrons and electrons (rejects ions and unstable nuclei).", params: obj(json!({ "protons": n("1-118"), "neutrons": n("neutrons"), "electrons": n("electrons (= protons for neutral)") }), &["protons", "neutrons", "electrons"]) },
        ToolSpec { name: "materials_preset", description: "Load a measured material preset.", params: obj(json!({ "preset": { "type": "string", "enum": ["gold", "copper", "iron", "aluminum", "silicon", "glass", "water", "graphite", "diamond"] } }), &["preset"]) },
        ToolSpec { name: "materials_save", description: "Save the built material to the library (MaterialRegistry).", params: obj(json!({ "name": s("name; default the material's name") }), &[]) },
        ToolSpec { name: "material_assign", description: "Assign a material to a 3D mesh object (e.g. 'make the cube copper'). The material may be a library name, a preset (gold, copper, iron, aluminum, silicon, glass, water, graphite, diamond) or a formula; it is saved to the library if new.", params: obj(json!({ "object": s("object name; omit for selected"), "material": s("library name, preset or formula") }), &["material"]) },
        ToolSpec { name: "arena_play", description: "Play or pause the physics arena: every visible 3D mesh becomes a rigid body (mass = material density × volume) under the arena environment (gravity, air drag, buoyancy, wind, heat exchange, phase changes).", params: obj(json!({ "playing": b("omit to toggle") }), &[]) },
        ToolSpec { name: "arena_step", description: "Advance the physics arena by fixed 1/240 s steps (paused). Runs over 240 steps go to a background thread; the next arena/scene command waits for them.", params: obj(json!({ "steps": n("number of 1/240 s steps, default 4") }), &[]) },
        ToolSpec { name: "arena_reset", description: "Stop the physics arena and restore the scene to how it was before the simulation.", params: obj(json!({}), &[]) },
        ToolSpec { name: "arena_env", description: "Set the arena environment: a preset and/or individual values.", params: obj(json!({ "preset": { "type": "string", "enum": ["earth", "moon", "mars", "vacuum", "space", "custom"] }, "gravity": n("m/s²"), "air_density": n("kg/m³"), "temperature_c": n("ambient °C"), "pressure_kpa": n("kPa"), "wind": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "m/s [x, y, z]" } }), &[]) },
        ToolSpec { name: "arena_body", description: "Physics settings of a 3D mesh: starting temperature, heater power (e.g. melt it), pinned, initial velocity.", params: obj(json!({ "object": s("object; omit for selected"), "temperature_c": n("°C"), "heater_w": n("watts (negative = cooling)"), "fixed": b("pinned in place"), "velocity": { "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": "m/s [x, y, z]" } }), &[]) },
        ToolSpec { name: "lab_tab", description: "Show a Materials Lab tab: elements (periodic table), reactions, circuits, computer, relativity.", params: obj(json!({ "tab": { "type": "string", "enum": ["elements", "reactions", "circuits", "computer", "relativity"] } }), &["tab"]) },
        ToolSpec { name: "chem_react", description: "Run a chemical reaction: the equation is balanced exactly, thermochemistry (ΔH, ΔS, ΔG, K) comes from CODATA/NIST-JANAF/CRC data, and an adiabatic constant-pressure reactor integrates the published rate law. Reports conversion, final and adiabatic flame temperature. Omitted fields keep their current values.", params: obj(json!({ "equation": s("e.g. 'CH4 + O2 -> CO2 + H2O' (balancing is automatic; <=> for equilibria; (g)/(l)/(s) tags optional)"), "temperature_c": n("start temperature °C"), "pressure_kpa": n("kPa"), "seconds": n("duration to simulate, s"), "air": b("oxygen comes as air (adds 3.76 N2 per O2)") }), &[]) },
        ToolSpec { name: "circuit_run", description: "Simulate a circuit by modified nodal analysis: divider (DC), rc / rlc (step transients), diode (I–V vs temperature), inverter (CMOS transfer curve), fuse (a real wire: R = ρL/A from the element, Joule heating, melts open), silicon (doped-Si resistivity).", params: obj(json!({ "preset": { "type": "string", "enum": ["divider", "rc", "rlc", "diode", "inverter", "fuse", "silicon"] }, "voltage": n("source volts"), "temperature_c": n("°C"), "material": s("wire element symbol, e.g. Cu, Al, W"), "length_mm": n("wire length mm"), "diameter_mm": n("wire diameter mm"), "doping_cm3": n("donor doping cm⁻³ for silicon") }), &[]) },
        ToolSpec { name: "cpu_run", description: "Run a program on the simulated 4-bit computer built from transistor-characterised CMOS gates. The die temperature and clock change gate delays; too hot or too fast and it computes wrong answers. die_object binds the die temperature to a 3D arena body (heat it with arena_body heater_w); empty string unbinds.", params: obj(json!({ "program": { "type": "string", "enum": ["multiply", "fibonacci", "countdown"] }, "source": s("custom assembly (LDI LDA STA ADD ADDI SUB SUBI JMP JZ JC OUT HLT, labels 'name:')"), "temperature_c": n("die °C"), "clock_mhz": n("MHz"), "die_object": s("3D object whose arena temperature is the die temperature") }), &[]) },
        ToolSpec { name: "cpu_heat_sweep", description: "Heat the die from 27 °C until the program fails, and report why (timing, logic levels or intrinsic silicon).", params: obj(json!({ "program": { "type": "string", "enum": ["multiply", "fibonacci", "countdown"] }, "clock_mhz": n("MHz") }), &[]) },
        ToolSpec { name: "rel_speed", description: "Special-relativity readouts (γ, time dilation, length contraction, energy, Doppler) at a speed.", params: obj(json!({ "beta": n("v/c, 0 to <1"), "speed_kms": n("speed in km/s (instead of beta)") }), &[]) },
        ToolSpec { name: "nbody_preset", description: "Load a gravity scenario (Newtonian N-body; optional 1PN Schwarzschild GR correction). gr_exaggeration > 1 shrinks c as a visual aid so the precession is visible.", params: obj(json!({ "preset": { "type": "string", "enum": ["mercury", "inner", "earth_moon", "figure8", "binary"] }, "gr": b("GR correction on"), "gr_exaggeration": n("1 = real; up to 1e8") }), &[]) },
        ToolSpec { name: "nbody_play", description: "Play or pause the N-body simulation.", params: obj(json!({ "playing": b("omit to toggle"), "days_per_second": n("simulated days per real second") }), &[]) },
        ToolSpec { name: "nbody_measure_precession", description: "Simulate Mercury for 5 orbits with and without GR at real strength and report the perihelion advance in arcseconds per century.", params: obj(json!({}), &[]) },
        ToolSpec { name: "timeline_play", description: "Play or pause the current mode's timeline.", params: obj(json!({ "playing": b("omit to toggle") }), &[]) },
        ToolSpec { name: "timeline_scrub", description: "Move the playhead (seconds).", params: obj(json!({ "time": n("seconds") }), &["time"]) },
        ToolSpec { name: "set_keyframe", description: "Key the selected 3D object's Y / composite layer opacity / animate frame at the playhead.", params: obj(json!({}), &[]) },
        ToolSpec { name: "delete_keyframe", description: "Delete the key at the playhead.", params: obj(json!({}), &[]) },
        ToolSpec { name: "run_reader", description: "Run the bitstream reader over the loaded stream.", params: obj(json!({ "width": n("bits per read 1-16"), "reads": n("number of reads, default 32"), "wrap": b("true = u8 wrapping counter, false = safe usize") }), &[]) },
        ToolSpec { name: "reset_reader", description: "Rewind the bitstream reader.", params: obj(json!({}), &[]) },
        ToolSpec { name: "set_streaming", description: "Go live to the RTMP target from Settings via ffmpeg, or end the stream. Starting needs the user.", params: obj(json!({ "live": b("live") }), &["live"]) },
        ToolSpec { name: "set_recording", description: "Start/stop recording the canvas (or screen) to MP4 via ffmpeg. Starting needs the user.", params: obj(json!({ "recording": b("recording") }), &["recording"]) },
        ToolSpec { name: "retry_stream", description: "Reconnect the live stream now (after a dropped connection).", params: obj(json!({}), &[]) },
        ToolSpec { name: "set_output_source", description: "What recording/streaming captures.", params: obj(json!({ "source": { "type": "string", "enum": ["canvas", "screen"] } }), &["source"]) },
        ToolSpec { name: "set_stream_target", description: "RTMP server URL (the stream key is set by the user in Settings).", params: obj(json!({ "url": s("rtmp://host/app") }), &["url"]) },
        ToolSpec { name: "generate_image", description: "Generate an image with Grok Imagine and add it to Paint as a new layer (user approval required; billed per image).", params: obj(json!({ "prompt": s("prompt") }), &["prompt"]) },
        ToolSpec { name: "video_generate", description: "Queue a Grok Imagine video generation (user approval required).", params: obj(json!({ "prompt": s("prompt"), "duration": n("seconds 1-15") }), &["prompt"]) },
        ToolSpec { name: "video_extend", description: "Extend a generated clip (user approval required).", params: obj(json!({ "clip": n("clip index"), "prompt": s("prompt") }), &["clip", "prompt"]) },
        ToolSpec { name: "video_stitch", description: "Write a concat list of clips (user approval required).", params: obj(json!({ "clips": { "type": "array", "items": { "type": "number" } } }), &[]) },
    ]
}

/// OpenAI/xAI `tools` array.
pub fn tools_json() -> Value {
    Value::Array(tool_specs().into_iter().map(|t| json!({ "type": "function", "function": { "name": t.name, "description": t.description, "parameters": t.params } })).collect())
}

/// Parse "#RRGGBB" / "RRGGBB".
pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().trim_start_matches('#');
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The number of AI tools. README.md and docs/AI_TOOLS.md must agree.
    const TOOL_COUNT: usize = 101;

    /// docs/AI_TOOLS.md, generated from tool_specs().
    fn ai_tools_markdown() -> String {
        let specs = tool_specs();
        let mut md = format!(
            "# AI tools\n\nGenerated from `tool_specs()` in `src/bin/ui/commands.rs`; \
             `cargo test --features ui --bin ui ai_tools_doc` checks it. \
             Regenerate with `cargo test --features ui --bin ui -- --ignored \
             regenerate_ai_tools_doc`.\n\n{} tools.\n\n| Tool | What it does |\n|------|--------------|\n",
            specs.len()
        );
        for t in &specs {
            md.push_str(&format!(
                "| `{}` | {} |\n",
                t.name,
                t.description.replace('|', "\\|")
            ));
        }
        md
    }

    #[test]
    fn ai_tools_doc_and_readme_match_the_code() {
        let specs = tool_specs();
        let mut names: Vec<_> = specs.iter().map(|t| t.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), specs.len(), "duplicate tool names");
        assert_eq!(
            include_str!("../../../docs/AI_TOOLS.md"),
            ai_tools_markdown(),
            "docs/AI_TOOLS.md is stale: run the regenerate_ai_tools_doc test"
        );
        let claim = format!("The {} commands are exposed", specs.len());
        assert!(
            include_str!("../../../README.md").contains(&claim),
            "README.md must say \"{claim}\""
        );
    }

    #[test]
    #[ignore = "tool, not a test: rewrites docs/AI_TOOLS.md"]
    fn regenerate_ai_tools_doc() {
        std::fs::write("docs/AI_TOOLS.md", ai_tools_markdown()).expect("write docs/AI_TOOLS.md");
    }

    fn sample_value(schema: &Value) -> Value {
        match schema.get("type").and_then(|t| t.as_str()) {
            Some("string") => schema
                .get("enum")
                .and_then(|e| e.get(0))
                .cloned()
                .unwrap_or(json!("x")),
            Some("number") => json!(1),
            Some("boolean") => json!(true),
            Some("array") => {
                let item = schema.get("items").cloned().unwrap_or(json!({}));
                let n = schema
                    .get("minItems")
                    .and_then(|m| m.as_u64())
                    .unwrap_or(1)
                    .max(1);
                Value::Array((0..n).map(|_| sample_value(&item)).collect())
            }
            _ => json!({}),
        }
    }

    #[test]
    fn every_tool_spec_parses_into_a_command() {
        let specs = tool_specs();
        assert_eq!(specs.len(), TOOL_COUNT);
        for t in &specs {
            let props = t
                .params
                .get("properties")
                .and_then(|p| p.as_object())
                .cloned()
                .unwrap_or_default();
            let required: Vec<String> = t
                .params
                .get("required")
                .and_then(|r| r.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let mut args = serde_json::Map::new();
            for r in &required {
                args.insert(r.clone(), sample_value(&props[r.as_str()]));
            }
            let cmd = parse_tool_call(t.name, &Value::Object(args).to_string())
                .unwrap_or_else(|e| panic!("{} failed: {}", t.name, e));
            assert_eq!(cmd.name(), t.name);
            // all optional args at once
            let mut all = serde_json::Map::new();
            for (k, v) in &props {
                all.insert(k.clone(), sample_value(v));
            }
            parse_tool_call(t.name, &Value::Object(all).to_string())
                .unwrap_or_else(|e| panic!("{} (all args) failed: {}", t.name, e));
        }
    }

    #[test]
    fn parse_examples_and_errors() {
        assert_eq!(
            parse_tool_call(
                "set_layer_opacity",
                r#"{"layer":"Mist Overlay","opacity":55}"#
            ),
            Ok(AppCommand::SetLayerOpacity {
                layer: Some("Mist Overlay".into()),
                opacity: 55.0
            })
        );
        assert_eq!(parse_tool_call("undo", ""), Ok(AppCommand::Undo));
        assert!(parse_tool_call("set_layer_opacity", r#"{"layer":"x"}"#).is_err());
        assert!(parse_tool_call("format_disk", "{}").is_err());
        assert!(parse_tool_call("undo", "[1,2]").is_err());
        assert!(parse_tool_call("undo", "{not json").is_err());
        assert!(
            parse_tool_call("set_stream_key", r#"{"key":"x"}"#).is_err(),
            "UI-only commands are not tools"
        );
        assert_eq!(
            parse_tool_call(
                "material_assign",
                r#"{"object":"cube1","material":"copper"}"#
            ),
            Ok(AppCommand::MaterialAssign {
                object: Some("cube1".into()),
                material: "copper".into()
            })
        );
        assert_eq!(
            parse_tool_call("materials_build", r#"{"formula":"H2O"}"#),
            Ok(AppCommand::MaterialsBuild {
                formula: Some("H2O".into())
            })
        );
        assert_eq!(
            parse_tool_call("scene_boolean", r#"{"op":"difference"}"#),
            Ok(AppCommand::SceneBoolean {
                op: "difference".into(),
                object: None,
                with: None
            })
        );
    }

    #[test]
    fn policy_flags() {
        assert!(AppCommand::ExportPng { path: None }.needs_user());
        assert!(AppCommand::VideoGenerate {
            prompt: "x".into(),
            duration: None
        }
        .needs_user());
        assert!(!AppCommand::SetStreaming { live: false }.needs_user());
        assert!(!AppCommand::Zoom {
            percent: Some(50.0),
            fit: None
        }
        .mutates());
        assert!(AppCommand::Undo.mutates());
        assert_eq!(parse_hex("#ff8000"), Some([255, 128, 0]));
        assert_eq!(parse_hex("nope"), None);
        assert_eq!(
            tools_json().as_array().map(|a| a.len()).unwrap_or(0),
            TOOL_COUNT
        );
    }
}

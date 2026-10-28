//! Paint mode: Photoshop-style layered raster editor on `paint_engine`.
//!
//! Discrete actions go through `PaintState::apply(AppCommand)`, the same
//! entry point Seraphina's tool calls use. Continuous pointer gestures
//! (strokes, drags) call the same engine functions directly and record
//! one undo step per gesture.

use crate::commands::{parse_hex, AppCommand};
use crate::text_raster;
use crate::theme::{self, Palette, SP_SM};
use crate::{resolve_asset, ToastKind, Toasts};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Stroke, Vec2};
use egui_phosphor::regular as ic;
use obs_rust_bitstream::paint_engine::{
    blur_mask, clone_dab, composite_into, dab_rect, gradient_fill, heal_dab, layer_thumbnail,
    paint_dab, spaced_dab_points, thumbnail, tone_dab, union_rect, LayerGroup, LayerMask,
    PaintLayer, Selection, TextSpec,
};
use obs_rust_bitstream::psd_pixels::{LayerKind, PsdLayerPixels, PsdPixels};
use obs_rust_bitstream::tile_undo::{budget_from_env, DeltaHistory};
use obs_rust_bitstream::ui_canvas::{
    save_target, zoom_step, BlendMode, Brush, Filter, PixelRect, Raster, ViewTransform,
};
use obs_rust_bitstream::Psd;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Move,
    Marquee,
    Lasso,
    QuickSelect,
    Crop,
    Healing,
    Clone,
    Brush,
    Eraser,
    Gradient,
    Dodge,
    Burn,
    Pen,
    Type,
    Hand,
    Zoom,
    Eyedropper,
}

impl Tool {
    pub const ALL: [Tool; 17] = [
        Tool::Move,
        Tool::Marquee,
        Tool::Lasso,
        Tool::QuickSelect,
        Tool::Crop,
        Tool::Healing,
        Tool::Clone,
        Tool::Brush,
        Tool::Eraser,
        Tool::Gradient,
        Tool::Dodge,
        Tool::Burn,
        Tool::Pen,
        Tool::Type,
        Tool::Hand,
        Tool::Zoom,
        Tool::Eyedropper,
    ];

    pub const SECTIONS: [(&'static str, &'static [Tool]); 8] = [
        ("Move", &[Tool::Move]),
        (
            "Selection",
            &[Tool::Marquee, Tool::Lasso, Tool::QuickSelect],
        ),
        ("Crop", &[Tool::Crop]),
        ("Retouch", &[Tool::Healing, Tool::Clone]),
        (
            "Paint",
            &[
                Tool::Brush,
                Tool::Eraser,
                Tool::Gradient,
                Tool::Dodge,
                Tool::Burn,
            ],
        ),
        ("Draw", &[Tool::Pen]),
        ("Type", &[Tool::Type]),
        ("Navigate", &[Tool::Hand, Tool::Zoom, Tool::Eyedropper]),
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Tool::Move => "Move Tool",
            Tool::Marquee => "Rectangular Marquee",
            Tool::Lasso => "Lasso",
            Tool::QuickSelect => "Quick Select",
            Tool::Crop => "Crop Tool",
            Tool::Healing => "Healing Brush",
            Tool::Clone => "Clone Stamp",
            Tool::Brush => "Brush Tool",
            Tool::Eraser => "Eraser Tool",
            Tool::Gradient => "Gradient Tool",
            Tool::Dodge => "Dodge Tool",
            Tool::Burn => "Burn Tool",
            Tool::Pen => "Pen Tool",
            Tool::Type => "Type Tool",
            Tool::Hand => "Hand Tool",
            Tool::Zoom => "Zoom Tool",
            Tool::Eyedropper => "Eyedropper",
        }
    }

    pub fn id(&self) -> &'static str {
        match self {
            Tool::Move => "move",
            Tool::Marquee => "marquee",
            Tool::Lasso => "lasso",
            Tool::QuickSelect => "quick_select",
            Tool::Crop => "crop",
            Tool::Healing => "healing",
            Tool::Clone => "clone",
            Tool::Brush => "brush",
            Tool::Eraser => "eraser",
            Tool::Gradient => "gradient",
            Tool::Dodge => "dodge",
            Tool::Burn => "burn",
            Tool::Pen => "pen",
            Tool::Type => "type",
            Tool::Hand => "hand",
            Tool::Zoom => "zoom",
            Tool::Eyedropper => "eyedropper",
        }
    }

    pub fn from_id(s: &str) -> Option<Tool> {
        let k = s.trim().to_lowercase().replace([' ', '-'], "_");
        Tool::ALL.iter().copied().find(|t| {
            t.id() == k
                || t.label().to_lowercase().replace(' ', "_") == k
                || t.label()
                    .to_lowercase()
                    .trim_end_matches(" tool")
                    .replace(' ', "_")
                    == k
        })
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Tool::Move => ic::ARROWS_OUT_CARDINAL,
            Tool::Marquee => ic::SELECTION,
            Tool::Lasso => ic::LASSO,
            Tool::QuickSelect => ic::MAGIC_WAND,
            Tool::Crop => ic::CROP,
            Tool::Healing => ic::BANDAIDS,
            Tool::Clone => ic::STAMP,
            Tool::Brush => ic::PAINT_BRUSH,
            Tool::Eraser => ic::ERASER,
            Tool::Gradient => ic::GRADIENT,
            Tool::Dodge => ic::SUN,
            Tool::Burn => ic::FIRE,
            Tool::Pen => ic::PEN_NIB,
            Tool::Type => ic::TEXT_T,
            Tool::Hand => ic::HAND,
            Tool::Zoom => ic::MAGNIFYING_GLASS,
            Tool::Eyedropper => ic::EYEDROPPER,
        }
    }

    pub fn key(&self) -> egui::Key {
        use egui::Key::*;
        match self {
            Tool::Move => V,
            Tool::Marquee => M,
            Tool::Lasso => L,
            Tool::QuickSelect => W,
            Tool::Crop => C,
            Tool::Healing => J,
            Tool::Clone => S,
            Tool::Brush => B,
            Tool::Eraser => E,
            Tool::Gradient => G,
            Tool::Dodge | Tool::Burn => O,
            Tool::Pen => P,
            Tool::Type => T,
            Tool::Hand => H,
            Tool::Zoom => Z,
            Tool::Eyedropper => I,
        }
    }

    pub fn key_label(&self) -> &'static str {
        match self {
            Tool::Burn => "Shift+O",
            _ => self.key().name(),
        }
    }

    pub fn hint(&self) -> &'static str {
        match self {
            Tool::Move => "Drag to move the active layer's pixels.",
            Tool::Marquee => "Drag a rectangle. Shift adds to the selection; click to deselect.",
            Tool::Lasso => "Drag a freehand outline; release to close it. Click to deselect.",
            Tool::QuickSelect => "Click to select similar colours (tolerance in the options bar). Shift-click adds.",
            Tool::Crop => "Drag a crop box, then Enter or Apply. Esc cancels.",
            Tool::Healing => "Paint over blemishes: texture comes from beside the brush, tone from around it.",
            Tool::Clone => "Alt-click to set the source, then paint to copy from it.",
            Tool::Brush => "Drag to paint on the active layer. Selections clip the brush.",
            Tool::Eraser => "Drag to erase the active layer to transparency.",
            Tool::Gradient => "Drag to lay a gradient from the foreground colour to transparent.",
            Tool::Dodge => "Drag to lighten. Exposure is in the options bar.",
            Tool::Burn => "Drag to darken. Exposure is in the options bar.",
            Tool::Pen => "Click to add path points. Enter strokes the path, Ctrl+Enter makes a selection, Esc clears.",
            Tool::Type => "Click on the canvas to place a text layer, type, then Enter.",
            Tool::Hand => "Drag to pan. Middle-drag pans with any tool.",
            Tool::Zoom => "Click to zoom in, Alt-click to zoom out.",
            Tool::Eyedropper => "Click to pick the foreground colour from the image.",
        }
    }

    pub fn is_stroke(&self) -> bool {
        matches!(
            self,
            Tool::Brush | Tool::Eraser | Tool::Dodge | Tool::Burn | Tool::Clone | Tool::Healing
        )
    }
}

pub fn blend_key(b: BlendMode) -> [u8; 4] {
    *match b {
        BlendMode::Normal => b"norm",
        BlendMode::Multiply => b"mul ",
        BlendMode::Screen => b"scrn",
        BlendMode::Overlay => b"over",
        BlendMode::Darken => b"dark",
        BlendMode::Lighten => b"lite",
        BlendMode::SoftLight => b"sLit",
        BlendMode::Difference => b"diff",
    }
}

pub fn blend_from_key(k: &[u8; 4]) -> Option<BlendMode> {
    Some(match k {
        b"norm" | b"pass" => BlendMode::Normal,
        b"mul " => BlendMode::Multiply,
        b"scrn" => BlendMode::Screen,
        b"over" => BlendMode::Overlay,
        b"dark" => BlendMode::Darken,
        b"lite" => BlendMode::Lighten,
        b"sLit" => BlendMode::SoftLight,
        b"diff" => BlendMode::Difference,
        _ => return None,
    })
}

// ---- document ---------------------------------------------------------------

/// Everything undo restores.
#[derive(Clone)]
pub struct PaintDoc {
    /// Bottom to top.
    pub layers: Vec<PaintLayer>,
    pub groups: Vec<LayerGroup>,
    pub width: u32,
    pub height: u32,
    pub selection: Option<Selection>,
    pub selected: usize,
    pub next_id: u32,
}

impl PaintDoc {
    fn alloc_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }
}

/// Undo step limit; the byte budget (`SERAPHINA_UNDO_MB`, default 512 MB)
/// usually binds first on big documents.
pub const UNDO_STEPS: usize = 200;

pub struct TextDraft {
    pub x: f32,
    pub y: f32,
    pub text: String,
    pub screen: Pos2,
    pub focus: bool,
}

pub struct PaintState {
    pub doc: PaintDoc,
    history: DeltaHistory<PaintDoc>,
    /// Layer rasters (by `Arc` address) already trimmed to their content.
    settled: HashSet<usize>,
    coalesce: Option<String>,
    /// Bumped by every document edit, undo and redo (dirty tracking).
    pub edits: u64,
    pub tool: Tool,
    pub brush: Brush,
    /// Dodge/burn exposure 0..=1.
    pub exposure: f32,
    pub tolerance: u8,
    pub text_size: f32,
    pub zoom: f32,
    pub pan: Vec2,
    fit_pending: bool,
    pub crop: Option<PixelRect>,
    drag_from: Option<(f32, f32)>,
    drag_to: Option<(f32, f32)>,
    last_dab: Option<(f32, f32)>,
    lasso: Vec<(f32, f32)>,
    pub pen: Vec<(f32, f32)>,
    pub clone_offset: Option<(f32, f32)>,
    clone_anchor: Option<(f32, f32)>,
    stroke_src: Option<Arc<Raster>>,
    stroke_rect: Option<PixelRect>,
    move_origin: Option<(i32, i32)>,
    stroking: bool,
    flat: Raster,
    tex: Option<crate::tiled_tex::TiledTexture>,
    dirty: Option<PixelRect>,
    thumb_gen: u64,
    thumbs: HashMap<u32, (u64, egui::TextureHandle)>,
    pub cursor_px: Option<(u32, u32)>,
    pub path: String,
    /// Summary of the last PSD opened or saved.
    pub psd: Option<String>,
    pub doc_name: String,
    pub text_draft: Option<TextDraft>,
    pub sections_open: [bool; 8],
    pub layers_open: bool,
    pub masks_open: bool,
    pub props_open: bool,
    pub rename: Option<(usize, String)>,
    pub source_ppi: Option<f32>,
}

/// Largest image Paint opens, in megapixels (A9). 128 MP is 512 MB per
/// full RGBA layer (a 16384×8192 or 11585² canvas); bigger files are
/// refused with a message instead of exhausting memory. Override with
/// `SERAPHINA_MAX_MEGAPIXELS`.
pub const DEFAULT_MAX_MEGAPIXELS: u64 = 128;

pub fn max_megapixels() -> u64 {
    std::env::var("SERAPHINA_MAX_MEGAPIXELS")
        .ok()
        .and_then(|v| v.trim().parse().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_MAX_MEGAPIXELS)
}

/// Refuse images over the megapixel cap, before decoding their pixels.
pub fn check_open_size(w: u32, h: u32) -> Result<(), String> {
    let px = w as u64 * h as u64;
    let cap = max_megapixels();
    if px > cap * 1_000_000 {
        return Err(format!(
            "{}×{} is {:.0} megapixels, over the {} MP limit (each layer would need {:.1} GB); downscale it first, or raise SERAPHINA_MAX_MEGAPIXELS",
            w,
            h,
            px as f64 / 1e6,
            cap,
            px as f64 * 4.0 / 1e9
        ));
    }
    Ok(())
}

/// Width and height from a PSD header (bytes 14..22, big-endian).
fn psd_dimensions(path: &std::path::Path) -> Option<(u32, u32)> {
    use std::io::Read;
    let mut b = [0u8; 22];
    std::fs::File::open(path).ok()?.read_exact(&mut b).ok()?;
    (&b[..4] == b"8BPS").then(|| {
        (
            u32::from_be_bytes([b[18], b[19], b[20], b[21]]),
            u32::from_be_bytes([b[14], b[15], b[16], b[17]]),
        )
    })
}

fn load_raster(path: &str) -> Result<Raster, String> {
    let file = resolve_asset(path);
    let (w, h) = image::ImageReader::open(&file)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .into_dimensions()
        .map_err(|e| e.to_string())?;
    check_open_size(w, h)?;
    let img = image::open(file).map_err(|e| e.to_string())?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    Raster::from_rgba(w, h, rgba.into_raw()).ok_or_else(|| "bad pixel buffer".to_string())
}

fn placeholder(w: u32, h: u32) -> Raster {
    let mut r = Raster::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let t = y as f32 / h as f32;
            r.data[i] = (30.0 + 50.0 * t) as u8;
            r.data[i + 1] = (34.0 + 40.0 * t) as u8;
            r.data[i + 2] = (52.0 + 30.0 * (1.0 - t)) as u8;
            r.data[i + 3] = 255;
        }
    }
    r
}

/// Deterministic value noise at quarter resolution, bilinear upsampled.
fn cloud_layer(w: u32, h: u32) -> Raster {
    let (sw, sh) = ((w / 4).max(2), (h / 4).max(2));
    let hash = |x: i32, y: i32, s: i32| -> f32 {
        let mut n = (x.wrapping_mul(374761393))
            .wrapping_add(y.wrapping_mul(668265263))
            .wrapping_add(s.wrapping_mul(1442695041)) as u32;
        n = (n ^ (n >> 13)).wrapping_mul(1274126177);
        ((n ^ (n >> 16)) & 0xffff) as f32 / 65535.0
    };
    let noise = |x: f32, y: f32, s: i32| {
        let (x0, y0) = (x.floor() as i32, y.floor() as i32);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
        let a = hash(x0, y0, s) + (hash(x0 + 1, y0, s) - hash(x0, y0, s)) * sx;
        let b = hash(x0, y0 + 1, s) + (hash(x0 + 1, y0 + 1, s) - hash(x0, y0 + 1, s)) * sx;
        a + (b - a) * sy
    };
    let mut small = vec![0f32; (sw * sh) as usize];
    for y in 0..sh {
        for x in 0..sw {
            let (fx, fy) = (x as f32 / sw as f32, y as f32 / sh as f32);
            let v = noise(fx * 6.0, fy * 4.0, 1) * 0.55
                + noise(fx * 13.0, fy * 9.0, 2) * 0.3
                + noise(fx * 27.0, fy * 19.0, 3) * 0.15;
            let fade = (1.0 - fy / 0.55).clamp(0.0, 1.0);
            small[(y * sw + x) as usize] = ((v - 0.45) * 2.4).clamp(0.0, 1.0) * fade;
        }
    }
    let mut r = Raster::new(w, h);
    for y in 0..h {
        let fy = (y as f32 / 4.0).min((sh - 1) as f32);
        let y0 = fy.floor() as u32;
        let y1 = (y0 + 1).min(sh - 1);
        let ty = fy - y0 as f32;
        for x in 0..w {
            let fx = (x as f32 / 4.0).min((sw - 1) as f32);
            let x0 = fx.floor() as u32;
            let x1 = (x0 + 1).min(sw - 1);
            let tx = fx - x0 as f32;
            let g = |xx: u32, yy: u32| small[(yy * sw + xx) as usize];
            let v = (g(x0, y0) * (1.0 - tx) + g(x1, y0) * tx) * (1.0 - ty)
                + (g(x0, y1) * (1.0 - tx) + g(x1, y1) * tx) * ty;
            let i = ((y * w + x) * 4) as usize;
            r.data[i..i + 4].copy_from_slice(&[255, 236, 214, (v * 255.0) as u8]);
        }
    }
    r
}

fn sky_layer(w: u32, h: u32) -> Raster {
    let mut r = Raster::new(w, h);
    gradient_fill(
        &mut r,
        (w as f32 * 0.5, 0.0),
        (w as f32 * 0.5, h as f32 * 0.5),
        [255, 150, 60, 255],
        [255, 150, 60, 0],
        None,
    );
    r
}

fn rock_layer(w: u32, h: u32) -> Raster {
    let mut r = Raster::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let fx = x as f32 / w as f32;
            let fy = y as f32 / h as f32;
            let d = ((fx - 0.5).abs() * 1.6).powf(1.6) + ((fy - 0.35).max(0.0) * 1.4).powf(2.2);
            let a = ((d - 0.25) * 1.6).clamp(0.0, 1.0);
            let i = ((y * w + x) * 4) as usize;
            r.data[i..i + 4].copy_from_slice(&[38, 28, 22, (a * 255.0) as u8]);
        }
    }
    r
}

fn lights_layer(w: u32, h: u32) -> Raster {
    let mut r = Raster::new(w, h);
    let mut seed: u32 = 0x5eed;
    let mut rnd = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / 16777216.0
    };
    for i in 0..160 {
        let t = i as f32 / 160.0;
        let x = w as f32 * (0.42 + 0.12 * (t * 9.0).sin() * (1.0 - t * 0.5) + (rnd() - 0.5) * 0.05);
        let y = h as f32 * (0.98 - 0.42 * t + (rnd() - 0.5) * 0.02);
        let size = (3.0 + rnd() * 6.0) * (1.2 - t * 0.6);
        let b = Brush {
            size: size * 3.0,
            hardness: 0.0,
            flow: 0.55,
            spacing: 0.25,
            color: [255, 196, 96],
        };
        paint_dab(&mut r, &b, x, y, false, None);
        let core = Brush {
            size: size * 0.8,
            hardness: 0.5,
            flow: 0.9,
            spacing: 0.25,
            color: [255, 236, 180],
        };
        paint_dab(&mut r, &core, x, y, false, None);
    }
    r
}

impl PaintState {
    pub fn new(toasts: &mut Toasts) -> Self {
        let mut doc = PaintDoc {
            layers: Vec::new(),
            groups: Vec::new(),
            width: 0,
            height: 0,
            selection: None,
            selected: 0,
            next_id: 0,
        };
        let base = match load_raster("assets/mountain_spire.jpg") {
            Ok(r) => r,
            Err(_) => {
                toasts.push(
                    ToastKind::Warn,
                    "assets/ not found next to the app; showing a placeholder image.",
                );
                placeholder(1168, 784)
            }
        };
        let (w, h) = (base.width, base.height);
        doc.width = w;
        doc.height = h;
        let id = doc.alloc_id();
        doc.layers.push(PaintLayer::new(id, "Mountain Spire", base));
        let gid = doc.alloc_id();
        doc.groups.push(LayerGroup {
            id: gid,
            name: "Environment".into(),
            visible: true,
            collapsed: false,
        });
        let id = doc.alloc_id();
        let mut sky = PaintLayer::new(id, "Sky Gradient", sky_layer(w, h));
        sky.blend = BlendMode::Overlay;
        sky.opacity = 0.6;
        sky.group = Some(gid);
        doc.layers.push(sky);
        let id = doc.alloc_id();
        let mut clouds = PaintLayer::new(id, "Clouds", cloud_layer(w, h));
        clouds.blend = BlendMode::Screen;
        clouds.opacity = 0.35;
        clouds.group = Some(gid);
        doc.layers.push(clouds);
        let id = doc.alloc_id();
        let mut rock = PaintLayer::new(id, "Rock Foreground", rock_layer(w, h));
        rock.blend = BlendMode::Multiply;
        rock.opacity = 0.85;
        doc.layers.push(rock);
        if let Ok(r) = load_raster("assets/mist_overlay.jpg") {
            let id = doc.alloc_id();
            let r = if (r.width, r.height) == (w, h) {
                r
            } else {
                r.resized(w, h)
            };
            let mut mist = PaintLayer::new(id, "Mist Overlay", r);
            mist.blend = BlendMode::Screen;
            mist.opacity = 0.45;
            let mut m = LayerMask::from_selection(&Selection::from_rect(
                w,
                h,
                PixelRect {
                    x: 0,
                    y: h * 2 / 5,
                    w,
                    h: h - h * 2 / 5,
                },
            ));
            m.feather = 40.0;
            mist.mask = Some(m);
            doc.layers.push(mist);
        }
        let id = doc.alloc_id();
        let mut lights = PaintLayer::new(id, "Valley Lights", lights_layer(w, h));
        lights.blend = BlendMode::Screen;
        doc.layers.push(lights);
        let id = doc.alloc_id();
        doc.layers
            .push(PaintLayer::new(id, "Paint", Raster::new(w, h)));
        doc.selected = doc.layers.len() - 1;
        for l in &mut doc.layers {
            if let Some(m) = &mut l.mask {
                m.prepare();
            }
        }
        let psd = None;
        let mut s = Self {
            doc,
            history: DeltaHistory::new(UNDO_STEPS, budget_from_env()),
            settled: HashSet::new(),
            coalesce: None,
            edits: 0,
            tool: Tool::Brush,
            brush: Brush::default(),
            exposure: 0.5,
            tolerance: 32,
            text_size: 48.0,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit_pending: true,
            crop: None,
            drag_from: None,
            drag_to: None,
            last_dab: None,
            lasso: Vec::new(),
            pen: Vec::new(),
            clone_offset: None,
            clone_anchor: None,
            stroke_src: None,
            stroke_rect: None,
            move_origin: None,
            stroking: false,
            flat: Raster::new(w, h),
            tex: None,
            dirty: None,
            thumb_gen: 1,
            thumbs: HashMap::new(),
            cursor_px: None,
            path: "assets/mountain_spire.jpg".to_string(),
            psd,
            doc_name: "mountain_spire".to_string(),
            text_draft: None,
            sections_open: [true; 8],
            layers_open: true,
            masks_open: true,
            props_open: true,
            rename: None,
            source_ppi: None,
        };
        s.mark_all();
        s
    }

    pub fn width(&self) -> u32 {
        self.doc.width
    }

    pub fn height(&self) -> u32 {
        self.doc.height
    }

    fn mark_all(&mut self) {
        self.dirty = Some(PixelRect {
            x: 0,
            y: 0,
            w: self.doc.width,
            h: self.doc.height,
        });
        self.thumb_gen += 1;
    }

    fn mark(&mut self, r: Option<PixelRect>) {
        self.dirty = union_rect(self.dirty, r);
    }

    /// Trim layers edited since the last step to their content, so a layer
    /// holding a small drawing on a big canvas costs only the drawing.
    pub(crate) fn settle(&mut self) {
        if self.stroking {
            return;
        }
        let mut keep = HashSet::with_capacity(self.doc.layers.len());
        for l in &mut self.doc.layers {
            let k = Arc::as_ptr(&l.raster) as usize;
            if !self.settled.contains(&k) {
                l.trim();
            }
            keep.insert(Arc::as_ptr(&l.raster) as usize);
        }
        self.settled = keep;
    }

    fn checkpoint(&mut self) {
        self.edits += 1;
        self.settle();
        self.history.push(&self.doc);
        self.coalesce = None;
    }

    /// One undo step for a burst of the same edit (slider drags).
    fn checkpoint_coalesced(&mut self, key: String) {
        self.edits += 1;
        if self.coalesce.as_deref() != Some(key.as_str()) {
            self.settle();
            self.history.push(&self.doc);
            self.coalesce = Some(key);
        }
    }

    pub fn end_coalesce(&mut self) {
        self.coalesce = None;
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn can_redo(&self) -> bool {
        self.history.can_redo()
    }

    fn restore(&mut self, d: PaintDoc) {
        let resized = (d.width, d.height) != (self.doc.width, self.doc.height);
        self.edits += 1;
        self.doc = d;
        self.doc.selected = self
            .doc
            .selected
            .min(self.doc.layers.len().saturating_sub(1));
        if resized {
            self.flat = Raster::new(self.doc.width, self.doc.height);
            self.tex = None;
            self.fit_pending = true;
        }
        self.coalesce = None;
        self.mark_all();
    }

    pub fn undo(&mut self) -> bool {
        self.settle();
        match self.history.undo(self.doc.clone()) {
            Some(prev) => {
                self.restore(prev);
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        self.settle();
        match self.history.redo(self.doc.clone()) {
            Some(next) => {
                self.restore(next);
                true
            }
            None => false,
        }
    }

    /// Replace the whole document (AI-batch undo).
    /// How many GPU textures the canvas is drawn from (0 before the first frame).
    pub fn gpu_tile_count(&self) -> usize {
        self.tex.as_ref().map_or(0, |t| t.tile_count())
    }

    pub fn set_doc(&mut self, d: PaintDoc) {
        self.checkpoint();
        self.restore(d);
    }

    pub fn fit(&mut self) {
        self.fit_pending = true;
    }

    pub fn zoom_by(&mut self, zoom_in: bool) {
        self.zoom = zoom_step(self.zoom, zoom_in);
    }

    /// Resolve a layer reference: None = active, "3" = third from top,
    /// otherwise case-insensitive name (exact, then substring).
    pub fn find_layer(&self, key: Option<&str>) -> Result<usize, String> {
        let n = self.doc.layers.len();
        let Some(k) = key.map(|k| k.trim()).filter(|k| !k.is_empty()) else {
            return if n == 0 {
                Err("no layers".into())
            } else {
                Ok(self.doc.selected.min(n - 1))
            };
        };
        if let Ok(i) = k.parse::<usize>() {
            if i >= 1 && i <= n {
                return Ok(n - i);
            }
            return Err(format!("layer index {} out of range 1-{}", i, n));
        }
        let lk = k.to_lowercase();
        if let Some(i) = self
            .doc
            .layers
            .iter()
            .position(|l| l.name.to_lowercase() == lk)
        {
            return Ok(i);
        }
        if let Some(i) = self
            .doc
            .layers
            .iter()
            .position(|l| l.name.to_lowercase().contains(&lk))
        {
            return Ok(i);
        }
        Err(format!(
            "no layer named '{}' (layers: {})",
            k,
            self.layer_names().join(", ")
        ))
    }

    pub fn layer_names(&self) -> Vec<String> {
        self.doc
            .layers
            .iter()
            .rev()
            .map(|l| l.name.clone())
            .collect()
    }

    fn editable(&self, i: usize) -> Result<(), String> {
        let l = &self.doc.layers[i];
        if l.locked {
            return Err(format!("“{}” is locked", l.name));
        }
        Ok(())
    }

    /// Prepare the active layer for pixel edits. Text layers are
    /// rasterised; offsets are baked so tools hit the right pixels.
    fn begin_pixel_edit(&mut self) -> Result<usize, String> {
        let i = self.find_layer(None)?;
        self.editable(i)?;
        let l = &mut self.doc.layers[i];
        l.text = None;
        l.bake_to(self.doc.width, self.doc.height);
        Ok(i)
    }

    // ---- pixel tools ------------------------------------------------------

    pub fn begin_stroke(&mut self) -> Result<(), String> {
        self.checkpoint();
        let i = self.begin_pixel_edit()?;
        self.stroke_src = matches!(self.tool, Tool::Clone | Tool::Healing)
            .then(|| self.doc.layers[i].raster.clone());
        self.last_dab = None;
        self.stroking = true;
        self.stroke_rect = None;
        Ok(())
    }

    pub fn end_stroke(&mut self) {
        self.stroking = false;
        self.last_dab = None;
        self.stroke_src = None;
        self.thumb_gen += 1;
    }

    fn dab_at(&mut self, i: usize, pt: (f32, f32)) {
        let b = self.brush;
        let sel = self.doc.selection.clone();
        let sel = sel.as_ref();
        let tool = self.tool;
        let src = self.stroke_src.clone();
        let off = self.clone_offset;
        let r = self.doc.layers[i].raster_mut();
        match tool {
            Tool::Brush => paint_dab(r, &b, pt.0, pt.1, false, sel),
            Tool::Eraser => paint_dab(r, &b, pt.0, pt.1, true, sel),
            Tool::Dodge => tone_dab(r, &b, pt.0, pt.1, self.exposure, true, sel),
            Tool::Burn => tone_dab(r, &b, pt.0, pt.1, self.exposure, false, sel),
            Tool::Clone => {
                if let (Some(src), Some(o)) = (src, off) {
                    clone_dab(r, &src, &b, pt.0, pt.1, o, sel);
                }
            }
            Tool::Healing => {
                if let Some(src) = src {
                    heal_dab(r, &src, &b, pt.0, pt.1, sel);
                }
            }
            _ => {}
        }
        let rect = dab_rect(pt.0, pt.1, b.size, self.doc.width, self.doc.height);
        self.mark(rect);
    }

    /// Continue the stroke to `pt` (image space).
    pub fn stroke_to(&mut self, pt: (f32, f32)) {
        let Ok(i) = self.find_layer(None) else { return };
        if self.doc.layers[i].locked {
            return;
        }
        let pts = match self.last_dab {
            Some(from) => spaced_dab_points(&self.brush, from, pt),
            None => vec![pt],
        };
        for p in pts {
            self.dab_at(i, p);
            self.last_dab = Some(p);
        }
    }

    fn stroke_points(&mut self, pts: &[(f32, f32)]) -> Result<(), String> {
        if pts.is_empty() {
            return Err("no points".into());
        }
        if self.tool == Tool::Clone && self.clone_offset.is_none() {
            return Err("set a clone source first (set_clone_source)".into());
        }
        self.begin_stroke()?;
        for p in pts {
            self.stroke_to(*p);
        }
        self.end_stroke();
        Ok(())
    }

    pub fn apply_crop_rect(&mut self, rect: PixelRect) -> Result<String, String> {
        let r = rect
            .clamped(self.doc.width, self.doc.height)
            .ok_or("crop box is outside the canvas")?;
        self.checkpoint();
        let (dw, dh) = (self.doc.width, self.doc.height);
        for l in &mut self.doc.layers {
            l.bake_to(dw, dh);
            if let Some(c) = l.raster.cropped(r) {
                l.raster = Arc::new(c);
            }
            if let Some(m) = &mut l.mask {
                *m = m.cropped(r);
                m.prepare();
            }
            if let Some(t) = &mut l.text {
                t.x -= r.x as f32;
                t.y -= r.y as f32;
            }
        }
        self.doc.width = r.w;
        self.doc.height = r.h;
        self.doc.selection = None;
        self.crop = None;
        self.flat = Raster::new(r.w, r.h);
        self.tex = None;
        self.fit_pending = true;
        self.mark_all();
        for p in &mut self.pen {
            p.0 -= r.x as f32;
            p.1 -= r.y as f32;
        }
        self.clone_anchor = None;
        self.clone_offset = None;
        Ok(format!("cropped to {} × {} px", r.w, r.h))
    }

    fn render_text_layer(&mut self, i: usize) {
        let (w, h) = (self.doc.width, self.doc.height);
        if let Some(t) = self.doc.layers[i].text.clone() {
            self.doc.layers[i].raster = Arc::new(text_raster::render(&t, w, h));
            self.mark_all();
        }
    }

    pub fn add_text(
        &mut self,
        text: &str,
        x: f32,
        y: f32,
        size: Option<f32>,
        color: Option<[u8; 3]>,
    ) -> Result<String, String> {
        if text.trim().is_empty() {
            return Err("empty text".into());
        }
        self.checkpoint();
        let id = self.doc.alloc_id();
        let name: String = text
            .lines()
            .next()
            .unwrap_or("Text")
            .chars()
            .take(24)
            .collect();
        let mut l = PaintLayer::new(id, &name, Raster::new(self.doc.width, self.doc.height));
        l.text = Some(TextSpec {
            text: text.to_string(),
            x,
            y,
            size: size.unwrap_or(self.text_size).clamp(4.0, 600.0),
            color: color.unwrap_or(self.brush.color),
        });
        let at = (self.doc.selected + 1).min(self.doc.layers.len());
        l.group = self.doc.layers.get(self.doc.selected).and_then(|s| s.group);
        self.doc.layers.insert(at, l);
        self.doc.selected = at;
        self.render_text_layer(at);
        Ok(format!("added text layer “{}”", name))
    }

    fn apply_filter(&mut self, f: &str, strength: Option<f32>) -> Result<String, String> {
        let i = self.find_layer(None)?;
        self.editable(i)?;
        self.checkpoint();
        let i = self.begin_pixel_edit()?;
        let sel = self.doc.selection.clone();
        let before = self.doc.layers[i].raster.clone();
        let mut after = (*before).clone();
        let label = match f.to_lowercase().as_str() {
            "blur" | "gaussian" | "gaussian blur" => {
                let r = strength.unwrap_or(4.0).clamp(1.0, 50.0) as u32;
                let (w, h) = (after.width, after.height);
                let n = (w * h) as usize;
                for c in 0..4 {
                    let ch: Vec<u8> = (0..n).map(|k| after.data[k * 4 + c]).collect();
                    let b = blur_mask(&ch, w, h, r);
                    for (k, v) in b.iter().enumerate().take(n) {
                        after.data[k * 4 + c] = *v;
                    }
                }
                format!("Gaussian blur {} px", r)
            }
            other => {
                let filt = Filter::ALL
                    .iter()
                    .copied()
                    .find(|x| x.label().to_lowercase() == other)
                    .filter(|x| *x != Filter::None)
                    .ok_or_else(|| {
                        format!(
                            "unknown filter '{}' (levels, curves, grayscale, invert, blur)",
                            other
                        )
                    })?;
                filt.apply(&mut after, strength.unwrap_or(1.2));
                filt.label().to_string()
            }
        };
        if let Some(s) = sel {
            for k in 0..(after.width * after.height) as usize {
                let m = s.mask[k] as f32 / 255.0;
                for c in 0..4 {
                    let b = before.data[k * 4 + c] as f32;
                    after.data[k * 4 + c] =
                        (b + (after.data[k * 4 + c] as f32 - b) * m).round() as u8;
                }
            }
        }
        let name = self.doc.layers[i].name.clone();
        self.doc.layers[i].raster = Arc::new(after);
        self.mark_all();
        Ok(format!("applied {} to {}", label, name))
    }

    pub fn flattened(&self) -> Raster {
        let mut out = Raster::new(self.doc.width, self.doc.height);
        composite_into(
            &mut out,
            &self.doc.layers,
            &self.doc.groups,
            [0, 0, 0],
            None,
        );
        out
    }

    /// Open an image as a new layer, or read a PSD header.
    pub fn open(&mut self, path: &str) -> Result<String, String> {
        let path = path.trim().to_string();
        let lower = path.to_lowercase();
        if path.is_empty() {
            return Err("enter a file path to open".into());
        }
        self.path = path.clone();
        if lower.ends_with(".psd") {
            return self.open_psd(&path);
        }
        let r = load_raster(&path).map_err(|e| format!("couldn't open {}: {}", path, e))?;
        self.checkpoint();
        let name = std::path::Path::new(&path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Image")
            .to_string();
        let r = if (r.width, r.height) == (self.doc.width, self.doc.height) {
            r
        } else {
            r.resized(self.doc.width, self.doc.height)
        };
        let id = self.doc.alloc_id();
        let at = (self.doc.selected + 1).min(self.doc.layers.len());
        self.doc.layers.insert(at, PaintLayer::new(id, &name, r));
        self.doc.selected = at;
        self.mark_all();
        Ok(format!("added layer “{}”", name))
    }

    /// Decode image bytes into a new layer above the active one.
    pub fn add_image_layer(&mut self, name: &str, bytes: &[u8]) -> Result<String, String> {
        if let Ok((w, h)) = image::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| e.to_string())
            .and_then(|r| r.into_dimensions().map_err(|e| e.to_string()))
        {
            check_open_size(w, h)?;
        }
        let img =
            image::load_from_memory(bytes).map_err(|e| format!("couldn't decode image: {}", e))?;
        let img = img
            .resize_to_fill(
                self.doc.width,
                self.doc.height,
                image::imageops::FilterType::Triangle,
            )
            .to_rgba8();
        let r = Raster::from_rgba(self.doc.width, self.doc.height, img.into_raw())
            .ok_or("bad image size")?;
        self.checkpoint();
        let id = self.doc.alloc_id();
        let at = (self.doc.selected + 1).min(self.doc.layers.len());
        self.doc.layers.insert(at, PaintLayer::new(id, name, r));
        self.doc.selected = at;
        self.mark_all();
        Ok(format!("added layer “{}”", name))
    }

    /// Decode a layered PSD (pixels, opacity, visibility, blend, groups) into the document.
    pub fn open_psd(&mut self, path: &str) -> Result<String, String> {
        let file = resolve_asset(path);
        if let Some((w, h)) = psd_dimensions(&file) {
            check_open_size(w, h)?;
        }
        let d = match PsdPixels::load(&file) {
            Ok(d) => d,
            Err(e) => {
                if let Some(old) = Psd::load(&file) {
                    return Err(format!("{} only holds a layer list ({} layers, no pixel data) from an older Seraphina Studio; re-save it from the app", path, old.layers.len()));
                }
                return Err(format!("couldn't read {}: {}", path, e));
            }
        };
        let (w, h) = (d.width, d.height);
        let mut doc = PaintDoc {
            layers: Vec::new(),
            groups: Vec::new(),
            width: w,
            height: h,
            selection: None,
            selected: 0,
            next_id: 0,
        };
        let mut stack: Vec<u32> = Vec::new();
        let mut unsupported = 0;
        for l in &d.layers {
            match l.kind {
                LayerKind::GroupEnd => {
                    let gid = doc.alloc_id();
                    doc.groups.push(LayerGroup {
                        id: gid,
                        name: "Group".into(),
                        visible: true,
                        collapsed: false,
                    });
                    stack.push(gid);
                }
                LayerKind::GroupFolder { open } => {
                    if let Some(gid) = stack.pop() {
                        if let Some(g) = doc.groups.iter_mut().find(|g| g.id == gid) {
                            g.name = l.name.clone();
                            g.visible = l.visible;
                            g.collapsed = !open;
                        }
                    }
                }
                LayerKind::Pixels => {
                    let r =
                        Raster::from_rgba(w, h, l.to_canvas(w, h)).ok_or("layer size mismatch")?;
                    let id = doc.alloc_id();
                    let mut pl = PaintLayer::new(id, &l.name, r);
                    pl.visible = l.visible;
                    pl.opacity = l.opacity as f32 / 255.0;
                    pl.blend = blend_from_key(&l.blend).unwrap_or_else(|| {
                        unsupported += 1;
                        BlendMode::Normal
                    });
                    pl.group = stack.last().copied();
                    doc.layers.push(pl);
                }
            }
        }
        if doc.layers.is_empty() {
            let merged = d
                .merged
                .clone()
                .ok_or("PSD has neither layers nor a merged image")?;
            let id = doc.alloc_id();
            doc.layers.push(PaintLayer::new(
                id,
                "Background",
                Raster::from_rgba(w, h, merged).ok_or("merged image size mismatch")?,
            ));
        }
        doc.selected = doc.layers.len() - 1;
        self.checkpoint();
        self.doc = doc;
        self.doc_name = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("document")
            .trim_end_matches("_edit")
            .to_string();
        self.crop = None;
        self.pen.clear();
        self.clone_anchor = None;
        self.clone_offset = None;
        self.thumbs.clear();
        self.fit_pending = true;
        self.mark_all();
        let n = self.doc.layers.len();
        let summary = format!(
            "{} × {}, {} layers, {} groups",
            w,
            h,
            n,
            self.doc.groups.len()
        );
        self.psd = Some(summary.clone());
        Ok(format!(
            "opened {} ({}{})",
            path,
            summary,
            if unsupported > 0 {
                format!("; {} unsupported blend modes shown as Normal", unsupported)
            } else {
                String::new()
            }
        ))
    }

    /// The document as a layered PSD: pixels per layer (masks baked into
    /// alpha, fill folded into opacity), groups and the merged composite.
    pub fn to_psd(&self) -> PsdPixels {
        let (w, h) = (self.doc.width, self.doc.height);
        let mut layers = Vec::new();
        let mut prev: Option<u32> = None;
        let folder = |gid: u32| {
            let g = self.doc.groups.iter().find(|g| g.id == gid);
            PsdLayerPixels::group_folder(
                g.map(|g| g.name.as_str()).unwrap_or("Group"),
                g.map(|g| g.visible).unwrap_or(true),
                !g.map(|g| g.collapsed).unwrap_or(false),
            )
        };
        for l in &self.doc.layers {
            if l.group != prev {
                if let Some(g) = prev {
                    layers.push(folder(g));
                }
                if l.group.is_some() {
                    layers.push(PsdLayerPixels::group_end());
                }
                prev = l.group;
            }
            let mut rgba = l.raster.data.clone();
            if let Some(m) = &l.mask {
                let mut m = m.clone();
                m.prepare();
                let (ox, oy) = l.offset;
                for y in 0..l.raster.height {
                    for x in 0..l.raster.width {
                        let (dx, dy) = (x as i32 + ox, y as i32 + oy);
                        let k = if dx >= 0 && dy >= 0 && (dx as u32) < w && (dy as u32) < h {
                            m.factor(dx as u32, dy as u32)
                        } else {
                            0.0
                        };
                        let i = ((y * l.raster.width + x) * 4 + 3) as usize;
                        rgba[i] = (rgba[i] as f32 * k).round() as u8;
                    }
                }
            }
            let mut pl = PsdLayerPixels::from_canvas(
                &l.name,
                l.raster.width,
                l.raster.height,
                &rgba,
                l.offset,
            );
            pl.visible = l.visible;
            pl.opacity = (l.opacity.clamp(0.0, 1.0) * l.fill.clamp(0.0, 1.0) * 255.0).round() as u8;
            pl.blend = blend_key(l.blend);
            layers.push(pl);
        }
        if let Some(g) = prev {
            layers.push(folder(g));
        }
        PsdPixels {
            width: w,
            height: h,
            layers,
            merged: Some(self.flattened().data),
        }
    }

    /// Suggested PSD file name: `<name>_edit.psd`, never the source file.
    pub fn psd_file_name(&self) -> String {
        let t = save_target(&format!("{}.psd", self.doc_name));
        std::path::Path::new(&t)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(t)
    }

    /// Suggested PNG export file name: `<name>_export.png`.
    pub fn png_file_name(&self) -> String {
        let stem = std::path::Path::new(&self.doc_name)
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "untitled".into());
        format!("{}_export.png", stem)
    }

    /// Write the layered PSD to `path` (atomically).
    pub fn save_psd_to(&mut self, path: &std::path::Path) -> Result<String, String> {
        let d = self.to_psd();
        let bytes = d.to_bytes()?;
        crate::project::write_atomic(path, &bytes).map_err(|e| format!("save failed: {}", e))?;
        let n = d
            .layers
            .iter()
            .filter(|l| l.kind == LayerKind::Pixels)
            .count();
        self.psd = Some(format!("{} × {}, {} layers saved", d.width, d.height, n));
        Ok(format!(
            "saved {} ({} layers with pixels)",
            path.display(),
            n
        ))
    }

    /// Write the flattened image as PNG to `path` (atomically).
    pub fn export_png_to(&mut self, path: &std::path::Path) -> Result<String, String> {
        use image::ImageEncoder;
        let flat = self.flattened();
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(
                &flat.data,
                flat.width,
                flat.height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(|e| format!("export failed: {}", e))?;
        crate::project::write_atomic(path, &bytes).map_err(|e| format!("export failed: {}", e))?;
        Ok(format!(
            "exported {} ({} × {})",
            path.display(),
            flat.width,
            flat.height
        ))
    }

    fn set_tool(&mut self, t: Tool) {
        if self.tool != t {
            self.tool = t;
            self.drag_from = None;
            self.lasso.clear();
        }
    }

    /// Execute a Paint-domain command. `None` = not a Paint command.
    pub fn apply(&mut self, cmd: &AppCommand) -> Option<Result<String, String>> {
        use AppCommand as C;
        let pct = |v: f32| (v / 100.0).clamp(0.0, 1.0);
        let r: Result<String, String> = match cmd {
            C::SelectTool { tool } => match Tool::from_id(tool) {
                Some(t) => {
                    self.set_tool(t);
                    Ok(format!("selected {}", t.label()))
                }
                None => Err(format!("unknown tool '{}'", tool)),
            },
            C::SetBrush {
                size,
                hardness,
                flow,
                color,
                exposure,
            } => {
                let mut parts = Vec::new();
                if let Some(s) = size {
                    self.brush.size = s.clamp(1.0, 500.0);
                    parts.push(format!("size {:.0} px", self.brush.size));
                }
                if let Some(h) = hardness {
                    self.brush.hardness = pct(*h);
                    parts.push(format!("hardness {:.0}%", h));
                }
                if let Some(f) = flow {
                    self.brush.flow = pct(*f).max(0.01);
                    parts.push(format!("flow {:.0}%", f));
                }
                if let Some(e) = exposure {
                    self.exposure = pct(*e);
                    parts.push(format!("exposure {:.0}%", e));
                }
                if let Some(c) = color {
                    match parse_hex(c) {
                        Some(rgb) => {
                            self.brush.color = rgb;
                            parts.push(format!(
                                "colour #{:02X}{:02X}{:02X}",
                                rgb[0], rgb[1], rgb[2]
                            ));
                        }
                        None => return Some(Err(format!("bad colour '{}', use #RRGGBB", c))),
                    }
                }
                if parts.is_empty() {
                    Err("nothing to change".into())
                } else {
                    Ok(format!("brush {}", parts.join(", ")))
                }
            }
            C::BrushStroke { points } => {
                let pts: Vec<(f32, f32)> = points.iter().map(|p| (p[0], p[1])).collect();
                if !self.tool.is_stroke() {
                    self.set_tool(Tool::Brush);
                }
                self.stroke_points(&pts)
                    .map(|_| format!("{} stroke through {} points", self.tool.label(), pts.len()))
            }
            C::Gradient { x0, y0, x1, y1 } => (|| {
                self.checkpoint();
                let i = self.begin_pixel_edit()?;
                let c = self.brush.color;
                let sel = self.doc.selection.clone();
                gradient_fill(
                    self.doc.layers[i].raster_mut(),
                    (*x0, *y0),
                    (*x1, *y1),
                    [c[0], c[1], c[2], 255],
                    [c[0], c[1], c[2], 0],
                    sel.as_ref(),
                );
                self.mark_all();
                Ok(format!("gradient on {}", self.doc.layers[i].name))
            })(),
            C::SetCloneSource { x, y } => {
                self.clone_anchor = Some((*x, *y));
                self.clone_offset = None;
                Ok(format!("clone source at {:.0}, {:.0}", x, y))
            }
            C::AddLayer { name } => {
                self.checkpoint();
                let id = self.doc.alloc_id();
                let nm = name.clone().unwrap_or_else(|| format!("Layer {}", id));
                let mut l = PaintLayer::new(id, &nm, Raster::new(self.doc.width, self.doc.height));
                l.group = self.doc.layers.get(self.doc.selected).and_then(|s| s.group);
                let at = (self.doc.selected + 1).min(self.doc.layers.len());
                self.doc.layers.insert(at, l);
                self.doc.selected = at;
                self.mark_all();
                Ok(format!("added layer “{}”", nm))
            }
            C::AddGroup { name } => (|| {
                let i = self.find_layer(None)?;
                self.checkpoint();
                let gid = self.doc.alloc_id();
                let nm = name
                    .clone()
                    .unwrap_or_else(|| format!("Group {}", self.doc.groups.len() + 1));
                self.doc.groups.push(LayerGroup {
                    id: gid,
                    name: nm.clone(),
                    visible: true,
                    collapsed: false,
                });
                self.doc.layers[i].group = Some(gid);
                self.mark_all();
                Ok(format!("grouped {} into “{}”", self.doc.layers[i].name, nm))
            })(),
            C::DuplicateLayer { layer } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                self.checkpoint();
                let id = self.doc.alloc_id();
                let mut l = self.doc.layers[i].clone();
                l.id = id;
                l.name = format!("{} copy", l.name);
                let nm = l.name.clone();
                self.doc.layers.insert(i + 1, l);
                self.doc.selected = i + 1;
                self.mark_all();
                Ok(format!("duplicated as “{}”", nm))
            })(),
            C::RemoveLayer { layer } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                if self.doc.layers.len() <= 1 {
                    return Err("can't delete the last layer".into());
                }
                self.checkpoint();
                let l = self.doc.layers.remove(i);
                self.doc.selected = i
                    .min(self.doc.layers.len() - 1)
                    .max(if i > 0 { i - 1 } else { 0 })
                    .min(self.doc.layers.len() - 1);
                self.mark_all();
                Ok(format!("deleted “{}”", l.name))
            })(),
            C::MoveLayer { layer, direction } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                let n = self.doc.layers.len();
                let to = match direction.to_lowercase().as_str() {
                    "up" => (i + 1).min(n - 1),
                    "down" => i.saturating_sub(1),
                    "top" => n - 1,
                    "bottom" => 0,
                    d => return Err(format!("direction '{}' (up, down, top, bottom)", d)),
                };
                if to == i {
                    return Ok(format!("“{}” is already there", self.doc.layers[i].name));
                }
                self.checkpoint();
                let l = self.doc.layers.remove(i);
                let nm = l.name.clone();
                self.doc.layers.insert(to, l);
                // Keep groups contiguous: stay in the own group only while
                // touching a member, join a group we land inside of.
                let above = self.doc.layers.get(to + 1).and_then(|l| l.group);
                let below = to
                    .checked_sub(1)
                    .and_then(|k| self.doc.layers.get(k))
                    .and_then(|l| l.group);
                let own = self.doc.layers[to].group;
                if !(own.is_some() && (above == own || below == own)) {
                    self.doc.layers[to].group = if above.is_some() && above == below {
                        above
                    } else {
                        None
                    };
                }
                self.doc.selected = to;
                self.mark_all();
                Ok(format!("moved “{}” {}", nm, direction))
            })(),
            C::SelectLayer { layer } => self.find_layer(Some(layer)).map(|i| {
                self.doc.selected = i;
                format!("selected layer “{}”", self.doc.layers[i].name)
            }),
            C::RenameLayer { layer, name } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                if name.trim().is_empty() {
                    return Err("empty name".into());
                }
                self.checkpoint();
                let old = std::mem::replace(&mut self.doc.layers[i].name, name.trim().to_string());
                Ok(format!("renamed “{}” to “{}”", old, name.trim()))
            })(),
            C::SetLayerOpacity { layer, opacity } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                self.checkpoint_coalesced(format!("opacity{}", self.doc.layers[i].id));
                self.doc.layers[i].opacity = pct(*opacity);
                self.mark_all();
                Ok(format!(
                    "set {} opacity {:.0}%",
                    self.doc.layers[i].name,
                    opacity.clamp(0.0, 100.0)
                ))
            })(),
            C::SetLayerFill { layer, fill } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                self.checkpoint_coalesced(format!("fill{}", self.doc.layers[i].id));
                self.doc.layers[i].fill = pct(*fill);
                self.mark_all();
                Ok(format!(
                    "set {} fill {:.0}%",
                    self.doc.layers[i].name,
                    fill.clamp(0.0, 100.0)
                ))
            })(),
            C::SetLayerBlend { layer, mode } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                let m = BlendMode::ALL
                    .iter()
                    .copied()
                    .find(|b| {
                        b.label().eq_ignore_ascii_case(mode.trim())
                            || b.label()
                                .replace(' ', "_")
                                .eq_ignore_ascii_case(mode.trim())
                    })
                    .ok_or_else(|| format!("unknown blend mode '{}'", mode))?;
                self.checkpoint();
                self.doc.layers[i].blend = m;
                self.mark_all();
                Ok(format!(
                    "set {} blend to {}",
                    self.doc.layers[i].name,
                    m.label()
                ))
            })(),
            C::SetLayerVisibility { layer, visible } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                self.checkpoint();
                self.doc.layers[i].visible = *visible;
                self.mark_all();
                Ok(format!(
                    "{} {}",
                    if *visible { "showed" } else { "hid" },
                    self.doc.layers[i].name
                ))
            })(),
            C::SetLayerLocked { layer, locked } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                self.checkpoint();
                self.doc.layers[i].locked = *locked;
                Ok(format!(
                    "{} {}",
                    if *locked { "locked" } else { "unlocked" },
                    self.doc.layers[i].name
                ))
            })(),
            C::OffsetLayer { layer, dx, dy } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                self.editable(i)?;
                self.checkpoint();
                let l = &mut self.doc.layers[i];
                l.offset = (l.offset.0 + dx, l.offset.1 + dy);
                let nm = l.name.clone();
                self.mark_all();
                Ok(format!("moved {} by {}, {} px", nm, dx, dy))
            })(),
            C::SetGroupVisibility { group, visible } => (|| {
                let gi = self
                    .doc
                    .groups
                    .iter()
                    .position(|g| g.name.eq_ignore_ascii_case(group.trim()))
                    .ok_or_else(|| format!("no group named '{}'", group))?;
                self.checkpoint();
                self.doc.groups[gi].visible = *visible;
                self.mark_all();
                Ok(format!(
                    "{} group {}",
                    if *visible { "showed" } else { "hid" },
                    self.doc.groups[gi].name
                ))
            })(),
            C::AddMask { layer } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                self.checkpoint();
                let m = match &self.doc.selection {
                    Some(s) => LayerMask::from_selection(s),
                    None => LayerMask::reveal_all(self.doc.width, self.doc.height),
                };
                let from_sel = self.doc.selection.is_some();
                self.doc.layers[i].mask = Some(m);
                self.mark_all();
                Ok(format!(
                    "added {} mask to {}",
                    if from_sel { "selection" } else { "reveal-all" },
                    self.doc.layers[i].name
                ))
            })(),
            C::SetMask {
                layer,
                density,
                feather,
                invert,
                enabled,
            } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                if self.doc.layers[i].mask.is_none() {
                    return Err(format!(
                        "{} has no mask (add_mask first)",
                        self.doc.layers[i].name
                    ));
                }
                self.checkpoint_coalesced(format!("mask{}", self.doc.layers[i].id));
                let mut parts = Vec::new();
                if let Some(m) = self.doc.layers[i].mask.as_mut() {
                    if let Some(d) = density {
                        m.density = pct(*d);
                        parts.push(format!("density {:.0}%", d));
                    }
                    if let Some(f) = feather {
                        m.feather = f.clamp(0.0, 250.0);
                        parts.push(format!("feather {:.1} px", m.feather));
                    }
                    if let Some(v) = invert {
                        m.invert = *v;
                        parts.push(if *v {
                            "inverted".into()
                        } else {
                            "not inverted".into()
                        });
                    }
                    if let Some(e) = enabled {
                        m.enabled = *e;
                        parts.push(if *e {
                            "enabled".into()
                        } else {
                            "disabled".into()
                        });
                    }
                    m.prepare();
                }
                self.mark_all();
                Ok(format!(
                    "{} mask {}",
                    self.doc.layers[i].name,
                    parts.join(", ")
                ))
            })(),
            C::RemoveMask { layer } => (|| {
                let i = self.find_layer(layer.as_deref())?;
                if self.doc.layers[i].mask.is_none() {
                    return Err("no mask".into());
                }
                self.checkpoint();
                self.doc.layers[i].mask = None;
                self.mark_all();
                Ok(format!("removed mask from {}", self.doc.layers[i].name))
            })(),
            C::SelectRect { x, y, w, h } => {
                let r = PixelRect::from_corners((*x, *y), (x + w, y + h));
                self.checkpoint();
                self.doc.selection = Some(Selection::from_rect(self.doc.width, self.doc.height, r));
                Ok(format!("selected {:.0}×{:.0} at {:.0},{:.0}", w, h, x, y))
            }
            C::SelectPolygon { points } => {
                if points.len() < 3 {
                    Err("need at least 3 points".into())
                } else {
                    self.checkpoint();
                    let pts: Vec<(f32, f32)> = points.iter().map(|p| (p[0], p[1])).collect();
                    let s = Selection::from_polygon(self.doc.width, self.doc.height, &pts);
                    let n = s.count();
                    self.doc.selection = Some(s);
                    Ok(format!("lasso selection, {} px", n))
                }
            }
            C::MagicWand { x, y, tolerance } => {
                let flat = self.flattened();
                let tol = tolerance.unwrap_or(self.tolerance);
                let s = Selection::quick_select(&flat, (x.max(0.0) as u32, y.max(0.0) as u32), tol);
                if s.is_empty() {
                    Err("point is outside the canvas".into())
                } else {
                    self.checkpoint();
                    let n = s.count();
                    self.doc.selection = Some(s);
                    Ok(format!("quick select {} px (tolerance {})", n, tol))
                }
            }
            C::SelectAll => {
                self.checkpoint();
                self.doc.selection = Some(Selection::all(self.doc.width, self.doc.height));
                Ok("selected all".into())
            }
            C::Deselect => {
                if self.doc.selection.is_some() {
                    self.checkpoint();
                }
                self.doc.selection = None;
                Ok("deselected".into())
            }
            C::InvertSelection => match self.doc.selection.clone() {
                Some(mut s) => {
                    self.checkpoint();
                    s.invert();
                    self.doc.selection = Some(s);
                    Ok("inverted selection".into())
                }
                None => Err("nothing selected".into()),
            },
            C::ApplyFilter { filter, strength } => self.apply_filter(filter, *strength),
            C::Crop { x, y, w, h } => {
                self.apply_crop_rect(PixelRect::from_corners((*x, *y), (x + w, y + h)))
            }
            C::AddText {
                text,
                x,
                y,
                size,
                color,
            } => {
                let col = match color {
                    Some(c) => match parse_hex(c) {
                        Some(v) => Some(v),
                        None => return Some(Err(format!("bad colour '{}'", c))),
                    },
                    None => None,
                };
                self.add_text(text, *x, *y, *size, col)
            }
            C::PenAddPoint { x, y } => {
                self.pen.push((*x, *y));
                Ok(format!(
                    "path point {} at {:.0}, {:.0}",
                    self.pen.len(),
                    x,
                    y
                ))
            }
            C::PenStroke => {
                if self.pen.len() < 2 {
                    Err("the pen path needs 2+ points".into())
                } else {
                    let pts = self.pen.clone();
                    let prev = self.tool;
                    if !matches!(self.tool, Tool::Brush | Tool::Eraser) {
                        self.tool = Tool::Brush;
                    }
                    let r = self
                        .stroke_points(&pts)
                        .map(|_| format!("stroked path ({} points)", pts.len()));
                    self.tool = prev;
                    r
                }
            }
            C::PenToSelection => {
                if self.pen.len() < 3 {
                    Err("the pen path needs 3+ points".into())
                } else {
                    self.checkpoint();
                    self.doc.selection = Some(Selection::from_polygon(
                        self.doc.width,
                        self.doc.height,
                        &self.pen,
                    ));
                    Ok("path → selection".into())
                }
            }
            C::PenClear => {
                self.pen.clear();
                Ok("cleared path".into())
            }
            C::Undo => {
                if self.undo() {
                    Ok("undo".into())
                } else {
                    Err("nothing to undo".into())
                }
            }
            C::Redo => {
                if self.redo() {
                    Ok("redo".into())
                } else {
                    Err("nothing to redo".into())
                }
            }
            C::Zoom { percent, fit } => {
                if fit.unwrap_or(false) {
                    self.fit();
                    Ok("fit to window".into())
                } else if let Some(p) = percent {
                    self.zoom = (p / 100.0).clamp(0.05, 16.0);
                    Ok(format!("zoom {:.0}%", self.zoom * 100.0))
                } else {
                    Err("give percent or fit".into())
                }
            }
            C::Pan { dx, dy } => {
                self.pan += Vec2::new(*dx, *dy);
                Ok(format!("panned {:.0}, {:.0}", dx, dy))
            }
            _ => return None,
        };
        if r.is_ok()
            && !matches!(
                cmd,
                C::SetLayerOpacity { .. } | C::SetLayerFill { .. } | C::SetMask { .. }
            )
        {
            self.coalesce = None;
        }
        self.thumb_gen += 1;
        Some(r)
    }

    /// Short state summary for the assistant's context.
    pub fn context(&self) -> String {
        let layers: Vec<String> = self
            .doc
            .layers
            .iter()
            .enumerate()
            .rev()
            .map(|(i, l)| {
                let g = l
                    .group
                    .and_then(|g| self.doc.groups.iter().find(|x| x.id == g))
                    .map(|g| format!(" in {}", g.name))
                    .unwrap_or_default();
                format!(
                    "{}{} [{} {:.0}% fill {:.0}%{}{}{}{}]",
                    if i == self.doc.selected { "*" } else { "" },
                    l.name,
                    l.blend.label(),
                    l.opacity * 100.0,
                    l.fill * 100.0,
                    if l.visible { "" } else { " hidden" },
                    if l.locked { " locked" } else { "" },
                    if l.mask.is_some() { " masked" } else { "" },
                    g
                )
            })
            .collect();
        format!(
            "Canvas {}×{} px. Tool {}. Brush {:.0}px #{:02X}{:02X}{:02X} hardness {:.0}%. Selection: {}. Layers top→bottom (* = active): {}",
            self.doc.width,
            self.doc.height,
            self.tool.label(),
            self.brush.size,
            self.brush.color[0],
            self.brush.color[1],
            self.brush.color[2],
            self.brush.hardness * 100.0,
            self.doc.selection.as_ref().and_then(|s| s.bounds()).map(|b| format!("{}×{} at {},{}", b.w, b.h, b.x, b.y)).unwrap_or_else(|| "none".into()),
            layers.join("; ")
        )
    }
}

// ---- UI ---------------------------------------------------------------------

fn checker(painter: &egui::Painter, r: Rect, p: &Palette) {
    let a = if p.dark {
        Color32::from_gray(58)
    } else {
        Color32::from_gray(220)
    };
    let b = if p.dark {
        Color32::from_gray(38)
    } else {
        Color32::from_gray(250)
    };
    painter.rect_filled(r, CornerRadius::ZERO, a);
    let s = 5.0;
    let mut y = r.top();
    let mut row = 0;
    while y < r.bottom() {
        let mut x = r.left() + if row % 2 == 0 { 0.0 } else { s };
        while x < r.right() {
            painter.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x, y),
                    Pos2::new((x + s).min(r.right()), (y + s).min(r.bottom())),
                ),
                CornerRadius::ZERO,
                b,
            );
            x += 2.0 * s;
        }
        y += s;
        row += 1;
    }
}

fn color_image(r: &Raster) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied([r.width as usize, r.height as usize], &r.data)
}

/// Boundary pixels of a selection as a marching-ants overlay.
fn selection_outline(s: &Selection) -> Raster {
    let mut out = Raster::new(s.width, s.height);
    for y in 0..s.height {
        for x in 0..s.width {
            let v = s.at(x, y) > 127;
            if !v {
                continue;
            }
            // Two pixels thick so the ants survive texture minification when zoomed out.
            let out_at = |dx: i64, dy: i64| {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                nx < 0
                    || ny < 0
                    || nx >= s.width as i64
                    || ny >= s.height as i64
                    || s.at(nx as u32, ny as u32) <= 127
            };
            let edge =
                (1..=2).any(|d| out_at(-d, 0) || out_at(d, 0) || out_at(0, -d) || out_at(0, d));
            if edge {
                let i = ((y * s.width + x) * 4) as usize;
                let c = if ((x + y) / 4) % 2 == 0 { 255 } else { 0 };
                out.data[i..i + 4].copy_from_slice(&[c, c, c, 255]);
            }
        }
    }
    out
}

#[derive(Default)]
pub struct SelOverlay {
    src: Option<Selection>,
    tex: Option<crate::tiled_tex::TiledTexture>,
}

impl PaintState {
    fn refresh(&mut self, ctx: &egui::Context, ov: &mut SelOverlay) {
        if (self.flat.width, self.flat.height) != (self.doc.width, self.doc.height) {
            self.flat = Raster::new(self.doc.width, self.doc.height);
            self.tex = None;
            self.mark_all();
        }
        if self.tex.is_none() {
            self.mark_all();
        }
        if let Some(r) = self.dirty.take() {
            for l in &mut self.doc.layers {
                if let Some(m) = &mut l.mask {
                    m.prepare();
                }
            }
            composite_into(
                &mut self.flat,
                &self.doc.layers,
                &self.doc.groups,
                [0, 0, 0],
                Some(r),
            );
            // tiled, so documents bigger than the GPU's texture limit draw
            let side = crate::tiled_tex::tile_side(ctx);
            match &mut self.tex {
                Some(t) if t.size() == (self.flat.width, self.flat.height) => {
                    t.update(&self.flat, r)
                }
                _ => {
                    self.tex = Some(crate::tiled_tex::TiledTexture::new(
                        ctx,
                        "paint-flat",
                        &self.flat,
                        side,
                        egui::TextureOptions::LINEAR,
                    ))
                }
            }
        }
        if !self.stroking {
            let gen = self.thumb_gen;
            for l in &self.doc.layers {
                let stale = self.thumbs.get(&l.id).map(|t| t.0 != gen).unwrap_or(true);
                if stale {
                    let img = color_image(&layer_thumbnail(l, self.doc.width, self.doc.height, 44));
                    match self.thumbs.get_mut(&l.id) {
                        Some(t) => {
                            t.1.set(img, egui::TextureOptions::LINEAR);
                            t.0 = gen;
                        }
                        None => {
                            let tex = ctx.load_texture(
                                format!("thumb{}", l.id),
                                img,
                                egui::TextureOptions::LINEAR,
                            );
                            self.thumbs.insert(l.id, (gen, tex));
                        }
                    }
                }
                if let Some(m) = &l.mask {
                    let key = l.id | 0x8000_0000;
                    if self.thumbs.get(&key).map(|t| t.0 != gen).unwrap_or(true) {
                        let mut r = Raster::new(m.width, m.height);
                        for (k, v) in m.data.iter().enumerate() {
                            let v = if m.invert { 255 - *v } else { *v };
                            r.data[k * 4..k * 4 + 4].copy_from_slice(&[v, v, v, 255]);
                        }
                        let img = color_image(&thumbnail(&r, 44));
                        let tex = ctx.load_texture(
                            format!("mthumb{}", l.id),
                            img,
                            egui::TextureOptions::LINEAR,
                        );
                        self.thumbs.insert(key, (gen, tex));
                    }
                }
            }
        }
        if ov.src != self.doc.selection {
            ov.src = self.doc.selection.clone();
            let side = crate::tiled_tex::tile_side(ctx);
            ov.tex = ov.src.as_ref().map(|s| {
                crate::tiled_tex::TiledTexture::new(
                    ctx,
                    "sel-outline",
                    &selection_outline(s),
                    side,
                    egui::TextureOptions::NEAREST,
                )
            });
        }
    }

    pub fn tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        ui.spacing_mut().item_spacing.y = 0.0;
        for (si, (title, tools)) in Tool::SECTIONS.iter().enumerate() {
            if theme::section_header(ui, p, title, self.sections_open[si]).clicked() {
                self.sections_open[si] = !self.sections_open[si];
            }
            if self.sections_open[si] {
                for t in tools.iter() {
                    let resp = theme::tool_row(
                        ui,
                        p,
                        t.icon(),
                        t.label(),
                        t.key_label(),
                        self.tool == *t,
                        true,
                    )
                    .on_hover_text(t.hint());
                    if resp.clicked() {
                        bus.push(AppCommand::SelectTool {
                            tool: t.id().into(),
                        });
                    }
                }
            }
        }
        ui.spacing_mut().item_spacing.y = 5.0;
        ui.add_space(SP_SM);
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(SP_SM as i8, 0))
            .show(ui, |ui| {
                ui.label(theme::section(p, "File"));
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.path)
                        .hint_text("image.jpg / .png or .psd")
                        .desired_width(f32::INFINITY),
                );
                let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 2.0 * SP_SM) / 3.0;
                    if ui
                        .add_sized([w, 24.0], theme::toggle_button(p, "Open", false))
                        .on_hover_text(
                            "Images become a new layer; a PSD opens with all of its layers",
                        )
                        .clicked()
                        || enter
                    {
                        bus.push(AppCommand::OpenFile {
                            path: self.path.clone(),
                        });
                    }
                    if ui
                        .add_sized([w, 24.0], theme::toggle_button(p, "Save PSD", false))
                        .on_hover_text("Save a layered PSD (asks where; suggests <name>_edit.psd)")
                        .clicked()
                    {
                        bus.push(AppCommand::SavePsd { path: None });
                    }
                    if ui
                        .add_sized([w, 24.0], theme::primary_button(p, "Export"))
                        .on_hover_text("Export a flattened PNG (asks where)")
                        .clicked()
                    {
                        bus.push(AppCommand::ExportPng { path: None });
                    }
                });
            });
    }

    /// Photoshop-style contextual options bar above the canvas.
    pub fn options_bar(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        ui.horizontal(|ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(ui, p, ic::CORNERS_OUT, "Fit to window  (Ctrl+0)", false)
                    .clicked()
                {
                    bus.push(AppCommand::Zoom {
                        percent: None,
                        fit: Some(true),
                    });
                }
                if theme::icon_button(ui, p, ic::MAGNIFYING_GLASS_PLUS, "Zoom in  (Ctrl+=)", false)
                    .clicked()
                {
                    bus.push(AppCommand::Zoom {
                        percent: Some(zoom_step(self.zoom, true) * 100.0),
                        fit: None,
                    });
                }
                ui.label(
                    egui::RichText::new(format!("{:.0}%", self.zoom * 100.0))
                        .monospace()
                        .size(11.5)
                        .color(p.muted),
                );
                if theme::icon_button(
                    ui,
                    p,
                    ic::MAGNIFYING_GLASS_MINUS,
                    "Zoom out  (Ctrl+−)",
                    false,
                )
                .clicked()
                {
                    bus.push(AppCommand::Zoom {
                        percent: Some(zoom_step(self.zoom, false) * 100.0),
                        fit: None,
                    });
                }
                theme::vdivider(ui, p, 20.0);
                ui.add_enabled_ui(self.can_redo(), |ui| {
                    if theme::icon_button(ui, p, ic::ARROW_CLOCKWISE, "Redo  (Ctrl+Shift+Z)", false)
                        .clicked()
                    {
                        bus.push(AppCommand::Redo);
                    }
                });
                ui.add_enabled_ui(self.can_undo(), |ui| {
                    if theme::icon_button(
                        ui,
                        p,
                        ic::ARROW_COUNTER_CLOCKWISE,
                        "Undo  (Ctrl+Z)",
                        false,
                    )
                    .clicked()
                    {
                        bus.push(AppCommand::Undo);
                    }
                });
                ui.with_layout(
                    egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        ui.label(
                            egui::RichText::new(self.tool.icon())
                                .size(16.0)
                                .color(p.accent),
                        );
                        ui.label(
                            egui::RichText::new(self.tool.label())
                                .strong()
                                .color(p.text),
                        );
                        theme::vdivider(ui, p, 20.0);
                        let t = self.tool;
                        let mut brush = self.brush;
                        let mut changed = false;
                        let dv = |ui: &mut egui::Ui,
                                  label: &str,
                                  v: &mut f32,
                                  range: std::ops::RangeInclusive<f32>,
                                  suffix: &str,
                                  speed: f64|
                         -> bool {
                            ui.label(theme::muted(p, label));
                            ui.add(
                                egui::DragValue::new(v)
                                    .range(range)
                                    .suffix(suffix)
                                    .speed(speed)
                                    .max_decimals(0),
                            )
                            .changed()
                        };
                        if t.is_stroke() || t == Tool::Pen {
                            changed |= dv(ui, "Size", &mut brush.size, 1.0..=500.0, " px", 0.5);
                            let mut h = brush.hardness * 100.0;
                            if dv(ui, "Hardness", &mut h, 0.0..=100.0, "%", 0.5) {
                                brush.hardness = h / 100.0;
                                changed = true;
                            }
                        }
                        if matches!(t, Tool::Brush | Tool::Eraser | Tool::Clone | Tool::Pen) {
                            let mut f = brush.flow * 100.0;
                            if dv(ui, "Flow", &mut f, 1.0..=100.0, "%", 0.5) {
                                brush.flow = f / 100.0;
                                changed = true;
                            }
                        }
                        if changed {
                            bus.push(AppCommand::SetBrush {
                                size: Some(brush.size),
                                hardness: Some(brush.hardness * 100.0),
                                flow: Some(brush.flow * 100.0),
                                color: None,
                                exposure: None,
                            });
                        }
                        if matches!(t, Tool::Dodge | Tool::Burn) {
                            let mut e = self.exposure * 100.0;
                            if dv(ui, "Exposure", &mut e, 1.0..=100.0, "%", 0.5) {
                                bus.push(AppCommand::SetBrush {
                                    size: None,
                                    hardness: None,
                                    flow: None,
                                    color: None,
                                    exposure: Some(e),
                                });
                            }
                        }
                        if matches!(
                            t,
                            Tool::Brush
                                | Tool::Gradient
                                | Tool::Type
                                | Tool::Pen
                                | Tool::Eyedropper
                        ) {
                            ui.label(theme::muted(p, "Colour"));
                            let mut c = self.brush.color;
                            let resp = egui::color_picker::color_edit_button_srgb(ui, &mut c)
                                .on_hover_text(format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2]));
                            if resp.changed() {
                                bus.push(AppCommand::SetBrush {
                                    size: None,
                                    hardness: None,
                                    flow: None,
                                    color: Some(format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])),
                                    exposure: None,
                                });
                            }
                        }
                        match t {
                            Tool::Type => {
                                ui.label(theme::muted(p, "Size"));
                                ui.add(
                                    egui::DragValue::new(&mut self.text_size)
                                        .range(6.0..=400.0)
                                        .suffix(" px")
                                        .speed(0.5),
                                );
                            }
                            Tool::QuickSelect => {
                                ui.label(theme::muted(p, "Tolerance"));
                                ui.add(egui::DragValue::new(&mut self.tolerance).range(0..=255));
                            }
                            Tool::Clone => {
                                let txt = match (self.clone_anchor, self.clone_offset) {
                                    (_, Some(o)) => format!("Source offset {:.0}, {:.0}", o.0, o.1),
                                    (Some(a), None) => {
                                        format!("Source {:.0}, {:.0}: paint to clone", a.0, a.1)
                                    }
                                    _ => "Alt-click to set the source".into(),
                                };
                                ui.label(theme::muted(p, &txt));
                            }
                            Tool::Crop => {
                                if let Some(c) = self.crop {
                                    ui.label(
                                        egui::RichText::new(format!("{} × {}", c.w, c.h))
                                            .monospace()
                                            .color(p.text),
                                    );
                                    if ui.add(theme::primary_button(p, "Apply")).clicked() {
                                        bus.push(AppCommand::Crop {
                                            x: c.x as f32,
                                            y: c.y as f32,
                                            w: c.w as f32,
                                            h: c.h as f32,
                                        });
                                    }
                                    if ui.add(theme::toggle_button(p, "Cancel", false)).clicked() {
                                        self.crop = None;
                                    }
                                } else {
                                    ui.label(theme::muted(p, "Drag a crop box"));
                                }
                            }
                            Tool::Pen => {
                                ui.label(theme::muted(p, &format!("{} points", self.pen.len())));
                                let ok = self.pen.len() >= 2;
                                if ui
                                    .add_enabled(ok, theme::toggle_button(p, "Stroke", false))
                                    .clicked()
                                {
                                    bus.push(AppCommand::PenStroke);
                                }
                                if ui
                                    .add_enabled(
                                        self.pen.len() >= 3,
                                        theme::toggle_button(p, "To selection", false),
                                    )
                                    .clicked()
                                {
                                    bus.push(AppCommand::PenToSelection);
                                }
                                if ui
                                    .add_enabled(
                                        !self.pen.is_empty(),
                                        theme::toggle_button(p, "Clear", false),
                                    )
                                    .clicked()
                                {
                                    bus.push(AppCommand::PenClear);
                                }
                            }
                            _ => {}
                        }
                        if matches!(t, Tool::Marquee | Tool::Lasso | Tool::QuickSelect) {
                            if ui.add(theme::toggle_button(p, "All", false)).clicked() {
                                bus.push(AppCommand::SelectAll);
                            }
                            if ui
                                .add_enabled(
                                    self.doc.selection.is_some(),
                                    theme::toggle_button(p, "Deselect", false),
                                )
                                .clicked()
                            {
                                bus.push(AppCommand::Deselect);
                            }
                            if ui
                                .add_enabled(
                                    self.doc.selection.is_some(),
                                    theme::toggle_button(p, "Invert", false),
                                )
                                .clicked()
                            {
                                bus.push(AppCommand::InvertSelection);
                            }
                        }
                    },
                );
            });
        });
    }

    pub fn center(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        bus: &mut Vec<AppCommand>,
        toasts: &mut Toasts,
        ov: &mut SelOverlay,
    ) {
        let ctx = ui.ctx().clone();
        self.refresh(&ctx, ov);
        let avail = ui.available_size();
        let (resp, painter) = ui.allocate_painter(avail, egui::Sense::click_and_drag());
        let rect = resp.rect;
        painter.rect_filled(rect, CornerRadius::ZERO, p.canvas);
        let (w, h) = (self.doc.width, self.doc.height);
        if self.fit_pending && rect.width() > 10.0 {
            self.zoom = ViewTransform::fit_zoom(rect.width(), rect.height(), (w, h), 28.0);
            self.pan = Vec2::ZERO;
            self.fit_pending = false;
        }
        let mut view = ViewTransform {
            viewport: (rect.min.x, rect.min.y, rect.width(), rect.height()),
            image: (w, h),
            zoom: self.zoom,
            pan: (self.pan.x, self.pan.y),
        };
        let (alt, shift, cmd) =
            ctx.input(|i| (i.modifiers.alt, i.modifiers.shift, i.modifiers.command));
        if resp.hovered() {
            let (scroll, zoom_delta) = ctx.input(|i| (i.raw_scroll_delta, i.zoom_delta()));
            let pointer = resp.hover_pos().unwrap_or(rect.center());
            let mut new_zoom = self.zoom;
            if (zoom_delta - 1.0).abs() > 1e-3 {
                new_zoom = (self.zoom * zoom_delta).clamp(0.05, 16.0);
            } else if (cmd || alt) && scroll.y.abs() > 0.0 {
                new_zoom = (self.zoom * (1.0 + scroll.y * 0.0015)).clamp(0.05, 16.0);
            } else if scroll != Vec2::ZERO {
                self.pan += scroll;
            }
            if new_zoom != self.zoom {
                let before = view.to_image(pointer.x, pointer.y);
                self.zoom = new_zoom;
                view.zoom = new_zoom;
                let after = view.to_screen(before.0, before.1);
                self.pan += Vec2::new(pointer.x - after.0, pointer.y - after.1);
            }
            view.zoom = self.zoom;
            view.pan = (self.pan.x, self.pan.y);
        }
        let panning = resp.dragged_by(egui::PointerButton::Middle)
            || (self.tool == Tool::Hand && resp.dragged_by(egui::PointerButton::Primary));
        if panning {
            self.pan += resp.drag_delta();
            view.pan = (self.pan.x, self.pan.y);
        }
        let (ix, iy, iw, ih) = view.image_rect();
        let img_rect = Rect::from_min_size(Pos2::new(ix, iy), Vec2::new(iw, ih));
        let clip = painter.with_clip_rect(rect);
        clip.rect_filled(
            img_rect.translate(Vec2::new(0.0, 5.0)).expand(3.0),
            CornerRadius::same(3),
            Color32::from_black_alpha(if p.dark { 140 } else { 50 }),
        );
        if let Some(t) = &self.tex {
            t.paint(&clip, img_rect, Color32::WHITE);
        }
        clip.rect_stroke(
            img_rect.expand(0.5),
            CornerRadius::ZERO,
            Stroke::new(1.0_f32, p.accent.gamma_multiply(0.85)),
            egui::StrokeKind::Middle,
        );
        if let Some(t) = &ov.tex {
            t.paint(&clip, img_rect, Color32::WHITE);
        }
        let hover = resp.hover_pos();
        self.cursor_px = hover.and_then(|hp| {
            let (x, y) = view.to_image(hp.x, hp.y);
            if view.in_image(x, y) {
                Some((x as u32, y as u32))
            } else {
                None
            }
        });
        let pointer_img = resp
            .interact_pointer_pos()
            .map(|pp| view.to_image(pp.x, pp.y));
        // egui reports drag_started only after the pointer moved; anchor
        // gestures at the press position so fast drags keep their start.
        let origin_img = ui
            .input(|i| i.pointer.press_origin())
            .map(|pp| view.to_image(pp.x, pp.y))
            .or(pointer_img);
        let started = resp.drag_started_by(egui::PointerButton::Primary) && !panning;
        let dragging = resp.dragged_by(egui::PointerButton::Primary) && !panning;
        let stopped = resp.drag_stopped();
        let clicked = resp.clicked();
        match self.tool {
            t if t.is_stroke() => {
                if t == Tool::Clone && alt && (clicked || started) {
                    if let Some(pt) = pointer_img {
                        bus.push(AppCommand::SetCloneSource { x: pt.0, y: pt.1 });
                    }
                } else if t == Tool::Clone
                    && self.clone_anchor.is_none()
                    && self.clone_offset.is_none()
                    && (clicked || started)
                {
                    toasts.push(ToastKind::Info, "Alt-click to set a clone source first.");
                } else {
                    if started || clicked {
                        if let (Tool::Clone, Some(a), None, Some(pt)) =
                            (t, self.clone_anchor, self.clone_offset, origin_img)
                        {
                            self.clone_offset = Some((a.0 - pt.0, a.1 - pt.1));
                        }
                        match self.begin_stroke() {
                            Ok(()) => {
                                if let (true, Some(o)) = (started, origin_img) {
                                    self.stroke_to(o);
                                }
                            }
                            Err(e) => {
                                toasts.push(ToastKind::Warn, &e);
                                self.stroking = false;
                            }
                        }
                    }
                    if self.stroking && (dragging || clicked) {
                        if let Some(pt) = pointer_img {
                            self.stroke_to(pt);
                        }
                    }
                    if self.stroking && (stopped || clicked) {
                        self.end_stroke();
                    }
                }
            }
            Tool::Move => {
                if started {
                    match self
                        .find_layer(None)
                        .and_then(|i| self.editable(i).map(|_| i))
                    {
                        Ok(i) => {
                            self.checkpoint();
                            self.move_origin = Some(self.doc.layers[i].offset);
                            self.drag_from = origin_img;
                        }
                        Err(e) => toasts.push(ToastKind::Warn, &e),
                    }
                }
                if let (true, Some(o), Some(a), Some(b)) =
                    (dragging, self.move_origin, self.drag_from, pointer_img)
                {
                    let i = self.doc.selected;
                    self.doc.layers[i].offset = (
                        o.0 + (b.0 - a.0).round() as i32,
                        o.1 + (b.1 - a.1).round() as i32,
                    );
                    self.mark_all();
                }
                if stopped {
                    self.move_origin = None;
                    self.drag_from = None;
                    self.thumb_gen += 1;
                }
            }
            Tool::Marquee | Tool::Crop => {
                if started {
                    self.drag_from = origin_img;
                    if self.tool == Tool::Marquee {
                        self.checkpoint();
                    }
                }
                if let (Some(a), Some(b), true) = (self.drag_from, pointer_img, dragging) {
                    if let Some(r) = PixelRect::from_corners(a, b).clamped(w, h) {
                        if self.tool == Tool::Marquee {
                            let mut s = Selection::from_rect(w, h, r);
                            if shift {
                                if let Some(prev) = self.history_top_selection() {
                                    for (d, v) in s.mask.iter_mut().zip(prev.mask.iter()) {
                                        *d = (*d).max(*v);
                                    }
                                }
                            }
                            self.doc.selection = Some(s);
                        } else {
                            self.crop = Some(r);
                        }
                    }
                }
                if stopped {
                    self.drag_from = None;
                }
                if clicked {
                    if self.tool == Tool::Marquee {
                        bus.push(AppCommand::Deselect);
                    } else {
                        self.crop = None;
                    }
                }
            }
            Tool::Lasso => {
                if started {
                    self.lasso.clear();
                    if let Some(o) = origin_img {
                        self.lasso.push(o);
                    }
                }
                if dragging {
                    if let Some(pt) = pointer_img {
                        if self
                            .lasso
                            .last()
                            .map(|l| (l.0 - pt.0).abs() + (l.1 - pt.1).abs() > 1.5 / self.zoom)
                            .unwrap_or(true)
                        {
                            self.lasso.push(pt);
                        }
                    }
                }
                if stopped {
                    if self.lasso.len() >= 3 {
                        bus.push(AppCommand::SelectPolygon {
                            points: self.lasso.iter().map(|p| [p.0, p.1]).collect(),
                        });
                    }
                    self.lasso.clear();
                }
                if clicked {
                    bus.push(AppCommand::Deselect);
                }
            }
            Tool::QuickSelect => {
                if clicked {
                    if let Some(pt) = pointer_img {
                        if shift {
                            let prev = self.doc.selection.clone();
                            if let Some(Ok(_)) = self.apply(&AppCommand::MagicWand {
                                x: pt.0,
                                y: pt.1,
                                tolerance: None,
                            }) {
                                if let (Some(prev), Some(cur)) = (prev, self.doc.selection.as_mut())
                                {
                                    for (d, v) in cur.mask.iter_mut().zip(prev.mask.iter()) {
                                        *d = (*d).max(*v);
                                    }
                                }
                            }
                        } else {
                            bus.push(AppCommand::MagicWand {
                                x: pt.0,
                                y: pt.1,
                                tolerance: None,
                            });
                        }
                    }
                }
            }
            Tool::Gradient => {
                if started {
                    self.drag_from = origin_img;
                }
                if dragging {
                    self.drag_to = pointer_img;
                }
                if stopped {
                    if let (Some(a), Some(b)) = (self.drag_from, self.drag_to) {
                        bus.push(AppCommand::Gradient {
                            x0: a.0,
                            y0: a.1,
                            x1: b.0,
                            y1: b.1,
                        });
                    }
                    self.drag_from = None;
                    self.drag_to = None;
                }
            }
            Tool::Pen => {
                if clicked {
                    if let Some(pt) = pointer_img {
                        bus.push(AppCommand::PenAddPoint { x: pt.0, y: pt.1 });
                    }
                }
            }
            Tool::Type => {
                if clicked && self.text_draft.is_none() {
                    if let (Some(pt), Some(sp)) = (pointer_img, resp.interact_pointer_pos()) {
                        self.text_draft = Some(TextDraft {
                            x: pt.0,
                            y: pt.1,
                            text: String::new(),
                            screen: sp,
                            focus: true,
                        });
                    }
                }
            }
            Tool::Zoom => {
                if clicked {
                    let pointer = resp.interact_pointer_pos().unwrap_or(rect.center());
                    let before = view.to_image(pointer.x, pointer.y);
                    self.zoom = zoom_step(self.zoom, !alt);
                    view.zoom = self.zoom;
                    let after = view.to_screen(before.0, before.1);
                    self.pan += Vec2::new(pointer.x - after.0, pointer.y - after.1);
                }
            }
            Tool::Eyedropper if clicked => {
                if let Some((x, y)) = pointer_img {
                    if let Some(px) = self.flat.pixel(x.max(0.0) as u32, y.max(0.0) as u32) {
                        bus.push(AppCommand::SetBrush {
                            size: None,
                            hardness: None,
                            flow: None,
                            color: Some(format!("#{:02X}{:02X}{:02X}", px[0], px[1], px[2])),
                            exposure: None,
                        });
                    }
                }
            }
            _ => {}
        }

        // overlays
        let to_screen = |x: f32, y: f32| {
            let s = view.to_screen(x, y);
            Pos2::new(s.0, s.1)
        };
        if let Some(c) = self.crop {
            let r = Rect::from_min_max(
                to_screen(c.x as f32, c.y as f32),
                to_screen((c.x + c.w) as f32, (c.y + c.h) as f32),
            );
            let shade = Color32::from_black_alpha(150);
            for s in [
                Rect::from_min_max(img_rect.min, Pos2::new(img_rect.max.x, r.min.y)),
                Rect::from_min_max(Pos2::new(img_rect.min.x, r.max.y), img_rect.max),
                Rect::from_min_max(
                    Pos2::new(img_rect.min.x, r.min.y),
                    Pos2::new(r.min.x, r.max.y),
                ),
                Rect::from_min_max(
                    Pos2::new(r.max.x, r.min.y),
                    Pos2::new(img_rect.max.x, r.max.y),
                ),
            ] {
                clip.rect_filled(s, CornerRadius::ZERO, shade);
            }
            clip.rect_stroke(
                r,
                CornerRadius::ZERO,
                Stroke::new(1.5_f32, p.accent),
                egui::StrokeKind::Middle,
            );
            for k in 1..3 {
                let fx = r.left() + r.width() * k as f32 / 3.0;
                let fy = r.top() + r.height() * k as f32 / 3.0;
                clip.vline(
                    fx,
                    r.y_range(),
                    Stroke::new(1.0_f32, p.accent.gamma_multiply(0.4)),
                );
                clip.hline(
                    r.x_range(),
                    fy,
                    Stroke::new(1.0_f32, p.accent.gamma_multiply(0.4)),
                );
            }
        }
        if self.lasso.len() > 1 {
            let pts: Vec<Pos2> = self.lasso.iter().map(|p| to_screen(p.0, p.1)).collect();
            clip.add(egui::Shape::line(pts, Stroke::new(1.5_f32, Color32::WHITE)));
        }
        if !self.pen.is_empty() {
            let pts: Vec<Pos2> = self.pen.iter().map(|p| to_screen(p.0, p.1)).collect();
            if pts.len() > 1 {
                clip.add(egui::Shape::line(
                    pts.clone(),
                    Stroke::new(1.5_f32, p.accent),
                ));
            }
            for q in &pts {
                clip.rect(
                    Rect::from_center_size(*q, Vec2::splat(7.0)),
                    CornerRadius::ZERO,
                    p.panel,
                    Stroke::new(1.2_f32, p.accent),
                    egui::StrokeKind::Middle,
                );
            }
        }
        if let (Some(a), Some(b)) = (self.drag_from, self.drag_to) {
            if self.tool == Tool::Gradient {
                clip.line_segment(
                    [to_screen(a.0, a.1), to_screen(b.0, b.1)],
                    Stroke::new(2.0_f32, Color32::WHITE),
                );
                clip.circle_filled(to_screen(a.0, a.1), 4.0, p.accent);
            }
        }
        if let Some(hp) = hover {
            if self.tool.is_stroke() {
                let r = (self.brush.size / 2.0 * self.zoom).max(1.0);
                clip.circle_stroke(hp, r, Stroke::new(1.0_f32, Color32::from_white_alpha(210)));
                clip.circle_stroke(
                    hp,
                    r + 1.0,
                    Stroke::new(1.0_f32, Color32::from_black_alpha(160)),
                );
                if self.tool == Tool::Clone {
                    let src = match (self.clone_offset, self.clone_anchor) {
                        (Some(o), _) => {
                            Some(Pos2::new(hp.x + o.0 * self.zoom, hp.y + o.1 * self.zoom))
                        }
                        (None, Some(a)) => Some(to_screen(a.0, a.1)),
                        _ => None,
                    };
                    if let Some(s) = src {
                        clip.line_segment(
                            [s - Vec2::new(6.0, 0.0), s + Vec2::new(6.0, 0.0)],
                            Stroke::new(1.5_f32, p.accent),
                        );
                        clip.line_segment(
                            [s - Vec2::new(0.0, 6.0), s + Vec2::new(0.0, 6.0)],
                            Stroke::new(1.5_f32, p.accent),
                        );
                    }
                }
            }
        }
        let cursor = match self.tool {
            Tool::Hand => {
                if resp.dragged() {
                    egui::CursorIcon::Grabbing
                } else {
                    egui::CursorIcon::Grab
                }
            }
            Tool::Zoom => {
                if alt {
                    egui::CursorIcon::ZoomOut
                } else {
                    egui::CursorIcon::ZoomIn
                }
            }
            Tool::Move => egui::CursorIcon::Move,
            Tool::Type => egui::CursorIcon::Text,
            t if t.is_stroke() => egui::CursorIcon::None,
            _ => egui::CursorIcon::Crosshair,
        };
        if resp.hovered() {
            ctx.set_cursor_icon(if panning {
                egui::CursorIcon::Grabbing
            } else {
                cursor
            });
        }
        // floating type box
        let mut commit = None;
        let mut cancel = false;
        if let Some(d) = &mut self.text_draft {
            egui::Area::new(egui::Id::new("type-draft"))
                .fixed_pos(d.screen)
                .order(egui::Order::Foreground)
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style())
                        .stroke(Stroke::new(1.0_f32, p.accent))
                        .show(ui, |ui| {
                            ui.label(theme::section(p, "New text layer"));
                            let r = ui.add(
                                egui::TextEdit::singleline(&mut d.text)
                                    .hint_text("Type, then Enter")
                                    .desired_width(220.0)
                                    .font(egui::FontId::proportional(15.0)),
                            );
                            if d.focus {
                                r.request_focus();
                                d.focus = false;
                            }
                            if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                commit = Some((d.text.clone(), d.x, d.y));
                            }
                            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                cancel = true;
                            }
                            ui.horizontal(|ui| {
                                if ui.add(theme::primary_button(p, "Add")).clicked() {
                                    commit = Some((d.text.clone(), d.x, d.y));
                                }
                                if ui.add(theme::toggle_button(p, "Cancel", false)).clicked() {
                                    cancel = true;
                                }
                            });
                        });
                });
        }
        if let Some((t, x, y)) = commit {
            if !t.trim().is_empty() {
                bus.push(AppCommand::AddText {
                    text: t,
                    x,
                    y,
                    size: Some(self.text_size),
                    color: None,
                });
            }
            self.text_draft = None;
        }
        if cancel {
            self.text_draft = None;
        }
    }

    /// Undo memory: (steps, redo steps, delta bytes, budget bytes).
    pub fn undo_stats(&self) -> (usize, usize, usize, usize) {
        (
            self.history.undo_len(),
            self.history.redo_len(),
            self.history.delta_bytes(),
            self.history.budget(),
        )
    }

    /// Bytes held by the document plus its undo history: distinct pixel
    /// buffers of the live document and both history anchors, plus every
    /// stored delta (which may double-count shared buffers, so this errs high).
    pub fn retained_bytes(&self) -> usize {
        let mut seen = HashSet::new();
        let (u, r) = self.history.anchors();
        let mut n = 0;
        for d in [Some(&self.doc), u, r].into_iter().flatten() {
            for l in &d.layers {
                if seen.insert(Arc::as_ptr(&l.raster) as usize) {
                    n += l.raster.data.len();
                }
                if let Some(m) = &l.mask {
                    if seen.insert(Arc::as_ptr(&m.data) as usize) {
                        n += m.data.len();
                    }
                }
            }
            n += d.selection.as_ref().map_or(0, |s| s.mask.len());
        }
        n + self.history.delta_bytes()
    }

    /// Bytes of distinct layer pixels in the document.
    pub fn pixel_bytes(&self) -> usize {
        let mut seen = HashSet::new();
        self.doc
            .layers
            .iter()
            .filter(|l| seen.insert(Arc::as_ptr(&l.raster) as usize))
            .map(|l| l.raster.data.len())
            .sum()
    }

    fn history_top_selection(&self) -> Option<Selection> {
        self.history.peek().and_then(|d| d.selection.clone())
    }

    /// Right-hand panels: Layers, Masks, Properties.
    pub fn inspector(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        ui.spacing_mut().item_spacing.y = 0.0;
        if theme::panel_header(ui, p, "Layers", Some("Ps"), self.layers_open) {
            self.layers_open = !self.layers_open;
        }
        if self.layers_open {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(SP_SM as i8, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    self.layers_ui(ui, p, bus);
                });
        }
        if theme::panel_header(ui, p, "Masks", Some("Ps"), self.masks_open) {
            self.masks_open = !self.masks_open;
        }
        if self.masks_open {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(SP_SM as i8, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    self.masks_ui(ui, p, bus);
                });
        }
        if theme::panel_header(ui, p, "Properties", Some("Ps"), self.props_open) {
            self.props_open = !self.props_open;
        }
        if self.props_open {
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(SP_SM as i8, 6))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    self.properties_ui(ui, p);
                });
        }
    }

    fn layers_ui(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        let n = self.doc.layers.len();
        if n == 0 {
            return;
        }
        let sel = self.doc.selected.min(n - 1);
        let (blend, op, fill, locked, name) = {
            let l = &self.doc.layers[sel];
            (l.blend, l.opacity, l.fill, l.locked, l.name.clone())
        };
        ui.horizontal(|ui| {
            egui::ComboBox::from_id_salt("blend")
                .width(118.0)
                .selected_text(blend.label())
                .show_ui(ui, |ui| {
                    for m in BlendMode::ALL {
                        if ui.selectable_label(m == blend, m.label()).clicked() {
                            bus.push(AppCommand::SetLayerBlend {
                                layer: Some(name.clone()),
                                mode: m.label().into(),
                            });
                        }
                    }
                });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::icon_button(
                    ui,
                    p,
                    if locked { ic::LOCK } else { ic::LOCK_OPEN },
                    if locked {
                        "Unlock layer"
                    } else {
                        "Lock layer pixels"
                    },
                    locked,
                )
                .clicked()
                {
                    bus.push(AppCommand::SetLayerLocked {
                        layer: Some(name.clone()),
                        locked: !locked,
                    });
                }
                let mut v = op * 100.0;
                let r = ui.add(
                    egui::DragValue::new(&mut v)
                        .range(0.0..=100.0)
                        .suffix("%")
                        .speed(0.5)
                        .max_decimals(0),
                );
                if r.changed() {
                    bus.push(AppCommand::SetLayerOpacity {
                        layer: Some(name.clone()),
                        opacity: v,
                    });
                }
                if r.drag_stopped() || r.lost_focus() {
                    self.end_coalesce();
                }
                ui.label(theme::muted(p, "Opacity:"));
            });
        });
        ui.horizontal(|ui| {
            ui.label(theme::muted(
                p,
                &format!("{} layers · {} groups", n, self.doc.groups.len()),
            ));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(26.0);
                let mut v = fill * 100.0;
                let r = ui.add(
                    egui::DragValue::new(&mut v)
                        .range(0.0..=100.0)
                        .suffix("%")
                        .speed(0.5)
                        .max_decimals(0),
                );
                if r.changed() {
                    bus.push(AppCommand::SetLayerFill {
                        layer: Some(name.clone()),
                        fill: v,
                    });
                }
                if r.drag_stopped() || r.lost_focus() {
                    self.end_coalesce();
                }
                ui.label(theme::muted(p, "Fill:"));
            });
        });
        ui.add_space(2.0);
        egui::Frame::NONE
            .stroke(Stroke::new(1.0_f32, p.border))
            .fill(p.bg)
            // egui >= 0.31 adds the stroke to the frame size; a -1 outer margin
            // keeps the 0.29 list geometry (stroke straddles the edge again).
            .outer_margin(egui::Margin::same(-1))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                egui::ScrollArea::vertical()
                    .id_salt("layers-scroll")
                    .max_height(250.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        let mut last_group: Option<u32> = None;
                        for i in (0..n).rev() {
                            let g = self.doc.layers[i].group;
                            if g.is_some() && g != last_group {
                                if let Some(gi) =
                                    self.doc.groups.iter().position(|x| Some(x.id) == g)
                                {
                                    self.group_row(ui, p, gi, bus);
                                }
                            }
                            last_group = g;
                            let collapsed = g
                                .and_then(|id| self.doc.groups.iter().find(|x| x.id == id))
                                .map(|x| x.collapsed)
                                .unwrap_or(false);
                            if !collapsed {
                                self.layer_row(ui, p, i, g.is_some(), bus);
                            }
                        }
                    });
            });
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let acts: [(&str, &str, AppCommand); 7] = [
                (
                    ic::FILE_PLUS,
                    "New layer",
                    AppCommand::AddLayer { name: None },
                ),
                (
                    ic::FOLDER_SIMPLE_PLUS,
                    "Group active layer",
                    AppCommand::AddGroup { name: None },
                ),
                (
                    ic::CIRCLE_HALF,
                    "Add layer mask (from selection)",
                    AppCommand::AddMask { layer: None },
                ),
                (
                    ic::COPY,
                    "Duplicate layer",
                    AppCommand::DuplicateLayer { layer: None },
                ),
                (
                    ic::ARROW_UP,
                    "Move layer up",
                    AppCommand::MoveLayer {
                        layer: None,
                        direction: "up".into(),
                    },
                ),
                (
                    ic::ARROW_DOWN,
                    "Move layer down",
                    AppCommand::MoveLayer {
                        layer: None,
                        direction: "down".into(),
                    },
                ),
                (
                    ic::TRASH,
                    "Delete layer",
                    AppCommand::RemoveLayer { layer: None },
                ),
            ];
            for (icon, tip, cmd) in acts {
                if theme::icon_button_sized(ui, p, icon, tip, false, 26.0, 15.0).clicked() {
                    bus.push(cmd);
                }
            }
        });
    }

    fn group_row(&mut self, ui: &mut egui::Ui, p: &Palette, gi: usize, bus: &mut Vec<AppCommand>) {
        let g = self.doc.groups[gi].clone();
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), egui::Sense::click());
        if resp.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::ZERO, p.surface_hi);
        }
        let eye = Rect::from_min_size(rect.min, Vec2::new(24.0, rect.height()));
        let eresp = ui
            .interact(eye, egui::Id::new(("geye", g.id)), egui::Sense::click())
            .on_hover_text(if g.visible {
                "Hide group"
            } else {
                "Show group"
            });
        ui.painter().text(
            eye.center(),
            egui::Align2::CENTER_CENTER,
            if g.visible { ic::EYE } else { ic::EYE_SLASH },
            egui::FontId::proportional(13.0),
            if g.visible { p.text } else { p.muted },
        );
        if eresp.clicked() {
            bus.push(AppCommand::SetGroupVisibility {
                group: g.name.clone(),
                visible: !g.visible,
            });
        }
        let caret = if g.collapsed {
            ic::CARET_RIGHT
        } else {
            ic::CARET_DOWN
        };
        ui.painter().text(
            Pos2::new(rect.left() + 32.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            caret,
            egui::FontId::proportional(11.0),
            p.muted,
        );
        ui.painter().text(
            Pos2::new(rect.left() + 48.0, rect.center().y),
            egui::Align2::CENTER_CENTER,
            if g.collapsed {
                ic::FOLDER
            } else {
                ic::FOLDER_OPEN
            },
            egui::FontId::proportional(14.0),
            p.accent,
        );
        ui.painter().text(
            Pos2::new(rect.left() + 62.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &g.name,
            egui::FontId::proportional(12.0),
            p.text,
        );
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(1.0_f32, p.border),
        );
        if resp.clicked() && !eresp.clicked() {
            self.doc.groups[gi].collapsed = !g.collapsed;
        }
    }

    fn layer_row(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        i: usize,
        indent: bool,
        bus: &mut Vec<AppCommand>,
    ) {
        let l = &self.doc.layers[i];
        let (id, name, visible, locked, is_text, has_mask) = (
            l.id,
            l.name.clone(),
            l.visible,
            l.locked,
            l.text.is_some(),
            l.mask.is_some(),
        );
        let selected = i == self.doc.selected;
        let (rect, resp) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), egui::Sense::click());
        let painter = ui.painter();
        if selected {
            painter.rect(
                rect.shrink(1.0),
                CornerRadius::same(2),
                p.accent_soft(),
                Stroke::new(1.0_f32, p.accent),
                egui::StrokeKind::Middle,
            );
        } else if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::ZERO, p.surface_hi);
        }
        let eye = Rect::from_min_size(rect.min, Vec2::new(24.0, rect.height()));
        painter.text(
            eye.center(),
            egui::Align2::CENTER_CENTER,
            if visible { ic::EYE } else { ic::EYE_SLASH },
            egui::FontId::proportional(13.0),
            if visible { p.text } else { p.muted },
        );
        let x0 = rect.left() + 26.0 + if indent { 14.0 } else { 0.0 };
        let thumb = Rect::from_min_size(Pos2::new(x0, rect.top() + 4.0), Vec2::new(36.0, 24.0));
        checker(painter, thumb, p);
        if is_text {
            painter.rect_filled(thumb, CornerRadius::ZERO, p.surface_hi);
            painter.text(
                thumb.center(),
                egui::Align2::CENTER_CENTER,
                ic::TEXT_T,
                egui::FontId::proportional(15.0),
                p.text,
            );
        } else if let Some((_, t)) = self.thumbs.get(&id) {
            let ts = t.size_vec2();
            let s = (thumb.width() / ts.x).min(thumb.height() / ts.y);
            let r = Rect::from_center_size(thumb.center(), ts * s);
            painter.image(
                t.id(),
                r,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        painter.rect_stroke(
            thumb,
            CornerRadius::ZERO,
            Stroke::new(1.0_f32, p.border),
            egui::StrokeKind::Middle,
        );
        let mut nx = thumb.right() + 6.0;
        if has_mask {
            let mr = Rect::from_min_size(Pos2::new(nx, rect.top() + 4.0), Vec2::new(24.0, 24.0));
            painter.rect_filled(mr, CornerRadius::ZERO, Color32::BLACK);
            if let Some((_, t)) = self.thumbs.get(&(id | 0x8000_0000)) {
                painter.image(
                    t.id(),
                    mr,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE,
                );
            }
            painter.rect_stroke(
                mr,
                CornerRadius::ZERO,
                Stroke::new(1.0_f32, p.line),
                egui::StrokeKind::Middle,
            );
            nx = mr.right() + 6.0;
        }
        let editing = matches!(&self.rename, Some((k, _)) if *k == i);
        if !editing {
            painter.text(
                Pos2::new(nx, rect.center().y),
                egui::Align2::LEFT_CENTER,
                &name,
                egui::FontId::proportional(12.0),
                if selected { p.accent } else { p.text },
            );
        }
        let mut rx = rect.right() - 12.0;
        if locked {
            painter.text(
                Pos2::new(rx, rect.center().y),
                egui::Align2::CENTER_CENTER,
                ic::LOCK,
                egui::FontId::proportional(12.0),
                p.muted,
            );
            rx -= 16.0;
        }
        if is_text {
            painter.text(
                Pos2::new(rx, rect.center().y),
                egui::Align2::CENTER_CENTER,
                ic::TEXT_AA,
                egui::FontId::proportional(12.0),
                p.muted,
            );
        }
        let eresp = ui
            .interact(eye, egui::Id::new(("eye", id)), egui::Sense::click())
            .on_hover_text(if visible { "Hide layer" } else { "Show layer" });
        if eresp.clicked() {
            bus.push(AppCommand::SetLayerVisibility {
                layer: Some(name.clone()),
                visible: !visible,
            });
        } else if resp.clicked() && !selected {
            bus.push(AppCommand::SelectLayer {
                layer: (self.doc.layers.len() - i).to_string(),
            });
        }
        if resp.double_clicked() {
            self.rename = Some((i, name.clone()));
        }
        if editing {
            let mut done = None;
            let er = Rect::from_min_max(
                Pos2::new(nx, rect.top() + 5.0),
                Pos2::new(rect.right() - 30.0, rect.bottom() - 5.0),
            );
            if let Some((_, buf)) = &mut self.rename {
                let r = ui.put(
                    er,
                    egui::TextEdit::singleline(buf).font(egui::FontId::proportional(12.0)),
                );
                r.request_focus();
                if r.lost_focus() {
                    done = Some(buf.clone());
                }
            }
            if let Some(nm) = done {
                if nm.trim() != name && !nm.trim().is_empty() {
                    bus.push(AppCommand::RenameLayer {
                        layer: Some((self.doc.layers.len() - i).to_string()),
                        name: nm,
                    });
                }
                self.rename = None;
            }
        }
        let idx = (self.doc.layers.len() - i).to_string();
        resp.context_menu(|ui| {
            if ui.button("Rename…").clicked() {
                self.rename = Some((i, name.clone()));
                ui.close();
            }
            for (label, cmd) in [
                (
                    "Duplicate",
                    AppCommand::DuplicateLayer {
                        layer: Some(idx.clone()),
                    },
                ),
                (
                    "Add mask",
                    AppCommand::AddMask {
                        layer: Some(idx.clone()),
                    },
                ),
                (
                    if locked { "Unlock" } else { "Lock" },
                    AppCommand::SetLayerLocked {
                        layer: Some(idx.clone()),
                        locked: !locked,
                    },
                ),
                (
                    "Move up",
                    AppCommand::MoveLayer {
                        layer: Some(idx.clone()),
                        direction: "up".into(),
                    },
                ),
                (
                    "Move down",
                    AppCommand::MoveLayer {
                        layer: Some(idx.clone()),
                        direction: "down".into(),
                    },
                ),
                (
                    "Delete",
                    AppCommand::RemoveLayer {
                        layer: Some(idx.clone()),
                    },
                ),
            ] {
                if ui.button(label).clicked() {
                    bus.push(cmd);
                    ui.close();
                }
            }
        });
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            Stroke::new(1.0_f32, p.border),
        );
    }

    fn masks_ui(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        let Some(l) = self.doc.layers.get(self.doc.selected) else {
            return;
        };
        let name = l.name.clone();
        let id = l.id;
        match l.mask.clone() {
            Some(m) => {
                ui.horizontal(|ui| {
                    ui.label(theme::muted(p, "Density:"));
                    let mut d = m.density * 100.0;
                    let r = ui.add(
                        egui::Slider::new(&mut d, 0.0..=100.0)
                            .suffix("%")
                            .max_decimals(0),
                    );
                    if r.changed() {
                        bus.push(AppCommand::SetMask {
                            layer: Some(name.clone()),
                            density: Some(d),
                            feather: None,
                            invert: None,
                            enabled: None,
                        });
                    }
                    if r.drag_stopped() {
                        self.end_coalesce();
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(theme::muted(p, "Feather:"));
                    let mut f = m.feather;
                    let r = ui.add(
                        egui::Slider::new(&mut f, 0.0..=100.0)
                            .suffix(" px")
                            .max_decimals(1),
                    );
                    if r.changed() {
                        bus.push(AppCommand::SetMask {
                            layer: Some(name.clone()),
                            density: None,
                            feather: Some(f),
                            invert: None,
                            enabled: None,
                        });
                    }
                    if r.drag_stopped() {
                        self.end_coalesce();
                    }
                });
                ui.horizontal(|ui| {
                    if let Some((_, t)) = self.thumbs.get(&(id | 0x8000_0000)) {
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::new(54.0, 36.0), egui::Sense::hover());
                        ui.painter().image(
                            t.id(),
                            r,
                            Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                            Color32::WHITE,
                        );
                        ui.painter().rect_stroke(
                            r,
                            CornerRadius::ZERO,
                            Stroke::new(1.0_f32, p.line),
                            egui::StrokeKind::Middle,
                        );
                    }
                    ui.vertical(|ui| {
                        theme::prop(ui, p, "Mask", name.to_string());
                        theme::prop(ui, p, "Kind", "Pixel · Grayscale");
                        theme::prop(ui, p, "Invert", if m.invert { "On" } else { "Off" });
                    });
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 2.0;
                    if theme::icon_button_sized(
                        ui,
                        p,
                        ic::PLUS,
                        "Replace mask from selection",
                        false,
                        26.0,
                        15.0,
                    )
                    .clicked()
                    {
                        bus.push(AppCommand::AddMask {
                            layer: Some(name.clone()),
                        });
                    }
                    if theme::icon_button_sized(
                        ui,
                        p,
                        ic::SWAP,
                        "Invert mask",
                        m.invert,
                        26.0,
                        15.0,
                    )
                    .clicked()
                    {
                        bus.push(AppCommand::SetMask {
                            layer: Some(name.clone()),
                            density: None,
                            feather: None,
                            invert: Some(!m.invert),
                            enabled: None,
                        });
                    }
                    if theme::icon_button_sized(
                        ui,
                        p,
                        if m.enabled { ic::EYE } else { ic::EYE_SLASH },
                        if m.enabled {
                            "Disable mask"
                        } else {
                            "Enable mask"
                        },
                        !m.enabled,
                        26.0,
                        15.0,
                    )
                    .clicked()
                    {
                        bus.push(AppCommand::SetMask {
                            layer: Some(name.clone()),
                            density: None,
                            feather: None,
                            invert: None,
                            enabled: Some(!m.enabled),
                        });
                    }
                    if theme::icon_button_sized(
                        ui,
                        p,
                        ic::SELECTION,
                        "Load mask as selection",
                        false,
                        26.0,
                        15.0,
                    )
                    .clicked()
                    {
                        let mut s = Selection {
                            width: m.width,
                            height: m.height,
                            mask: (*m.data).clone(),
                        };
                        if m.invert {
                            s.invert();
                        }
                        self.checkpoint();
                        self.doc.selection = Some(s);
                    }
                    if theme::icon_button_sized(ui, p, ic::TRASH, "Delete mask", false, 26.0, 15.0)
                        .clicked()
                    {
                        bus.push(AppCommand::RemoveMask {
                            layer: Some(name.clone()),
                        });
                    }
                });
            }
            None => {
                ui.label(theme::muted(p, &format!("“{}” has no mask.", name)));
                ui.horizontal(|ui| {
                    if ui
                        .add(theme::toggle_button(
                            p,
                            if self.doc.selection.is_some() {
                                "Add mask from selection"
                            } else {
                                "Add reveal-all mask"
                            },
                            false,
                        ))
                        .clicked()
                    {
                        bus.push(AppCommand::AddMask {
                            layer: Some(name.clone()),
                        });
                    }
                });
            }
        }
    }

    fn properties_ui(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let (w, h) = (self.doc.width, self.doc.height);
        ui.label(
            egui::RichText::new("Canvas Size")
                .size(11.5)
                .strong()
                .color(p.text),
        );
        theme::prop(ui, p, "Width:", format!("{} px", w));
        theme::prop(ui, p, "Height:", format!("{} px", h));
        theme::prop(ui, p, "Resolution:", "72 Pixels/Inch (default)");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Color Profile")
                .size(11.5)
                .strong()
                .color(p.text),
        );
        theme::prop(ui, p, "Working RGB:", "sRGB (assumed)");
        let mut proof = false;
        ui.add_enabled(false, egui::Checkbox::new(&mut proof, "Proof Colors"))
            .on_disabled_hover_text("Coming soon: soft-proofing needs ICC support");
        ui.add_space(4.0);
        ui.label(
            egui::RichText::new("Info")
                .size(11.5)
                .strong()
                .color(p.text),
        );
        let mut seen = std::collections::HashSet::new();
        let bytes: usize = self
            .doc
            .layers
            .iter()
            .filter(|l| seen.insert(Arc::as_ptr(&l.raster) as usize))
            .map(|l| l.raster.data.len())
            .sum();
        theme::prop(
            ui,
            p,
            "Document Size:",
            format!("{:.1} MB", bytes as f64 / 1_048_576.0),
        );
        let (steps, _, held, budget) = self.undo_stats();
        theme::prop(
            ui,
            p,
            "Undo Memory:",
            format!(
                "{:.1} / {} MB, {} steps",
                held as f64 / 1_048_576.0,
                budget >> 20,
                steps
            ),
        );
        if let Some(n) = Some(self.gpu_tile_count()).filter(|n| *n > 1) {
            theme::prop(ui, p, "GPU Tiles:", format!("{} (over texture limit)", n));
        }
        theme::prop(ui, p, "Bit Depth:", "8-bit/Channel");
        theme::prop(ui, p, "Channels:", "RGB + Alpha");
        theme::prop(
            ui,
            p,
            "Layers:",
            format!(
                "{} in {} groups",
                self.doc.layers.len(),
                self.doc.groups.len()
            ),
        );
        let sel = self
            .doc
            .selection
            .as_ref()
            .and_then(|s| s.bounds())
            .map(|b| format!("{} × {} at {}, {}", b.w, b.h, b.x, b.y))
            .unwrap_or_else(|| "None".into());
        theme::prop(ui, p, "Selection:", sel);
        if let Some(d) = &self.psd {
            theme::prop(ui, p, "PSD:", d.clone());
        }
    }
}

// ---- project files ------------------------------------------------------------

use crate::project::{Assets, RasterRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MaskFile {
    pub width: u32,
    pub height: u32,
    pub data: String,
    pub density: f32,
    pub feather: f32,
    pub invert: bool,
    pub enabled: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LayerFile {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub fill: f32,
    pub blend: BlendMode,
    pub raster: RasterRef,
    pub offset: (i32, i32),
    pub mask: Option<MaskFile>,
    pub group: Option<u32>,
    pub locked: bool,
    pub text: Option<TextSpec>,
}

/// Paint mode in a `.seraphina` project.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PaintFile {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<LayerFile>,
    pub groups: Vec<LayerGroup>,
    /// Selection plane (`width * height` bytes).
    pub selection: Option<String>,
    pub selected: usize,
    pub next_id: u32,
    pub doc_name: String,
    pub path: String,
    pub psd: Option<String>,
    pub source_ppi: Option<f32>,
    pub tool: String,
    pub brush: Brush,
    pub exposure: f32,
    pub tolerance: u8,
    pub text_size: f32,
    pub pen: Vec<(f32, f32)>,
    pub clone_offset: Option<(f32, f32)>,
}

impl PaintState {
    pub fn to_file(&self, a: &mut Assets) -> PaintFile {
        let d = &self.doc;
        let layers = d
            .layers
            .iter()
            .map(|l| LayerFile {
                id: l.id,
                name: l.name.clone(),
                visible: l.visible,
                opacity: l.opacity,
                fill: l.fill,
                blend: l.blend,
                raster: a.put_raster(&format!("paint {}", l.name), &l.raster),
                offset: l.offset,
                mask: l.mask.as_ref().map(|m| MaskFile {
                    width: m.width,
                    height: m.height,
                    data: a.put(&format!("mask {}", l.name), (*m.data).clone()),
                    density: m.density,
                    feather: m.feather,
                    invert: m.invert,
                    enabled: m.enabled,
                }),
                group: l.group,
                locked: l.locked,
                text: l.text.clone(),
            })
            .collect();
        PaintFile {
            width: d.width,
            height: d.height,
            layers,
            groups: d.groups.clone(),
            selection: d
                .selection
                .as_ref()
                .map(|s| a.put("selection", s.mask.clone())),
            selected: d.selected,
            next_id: d.next_id,
            doc_name: self.doc_name.clone(),
            path: self.path.clone(),
            psd: self.psd.clone(),
            source_ppi: self.source_ppi,
            tool: self.tool.id().to_string(),
            brush: self.brush,
            exposure: self.exposure,
            tolerance: self.tolerance,
            text_size: self.text_size,
            pen: self.pen.clone(),
            clone_offset: self.clone_offset,
        }
    }

    /// Replace the document with a project's (undo history starts fresh).
    pub fn load_file(&mut self, f: PaintFile, a: &Assets) -> Result<(), String> {
        let (w, h) = (f.width, f.height);
        if w == 0 || h == 0 {
            return Err("paint document has no size".into());
        }
        check_open_size(w, h)?;
        let mut layers = Vec::with_capacity(f.layers.len());
        for l in f.layers {
            let mut p = PaintLayer::new(l.id, &l.name, a.raster(&l.raster)?);
            p.visible = l.visible;
            p.opacity = l.opacity;
            p.fill = l.fill;
            p.blend = l.blend;
            p.offset = l.offset;
            p.group = l.group;
            p.locked = l.locked;
            p.text = l.text;
            if let Some(m) = l.mask {
                let sel = Selection {
                    width: m.width,
                    height: m.height,
                    mask: a.plane(&m.data, m.width, m.height)?,
                };
                let mut mk = LayerMask::from_selection(&sel);
                mk.density = m.density;
                mk.feather = m.feather;
                mk.invert = m.invert;
                mk.enabled = m.enabled;
                mk.prepare();
                p.mask = Some(mk);
            }
            layers.push(p);
        }
        let selection = match f.selection {
            Some(s) => Some(Selection {
                width: w,
                height: h,
                mask: a.plane(&s, w, h)?,
            }),
            None => None,
        };
        let doc = PaintDoc {
            layers,
            groups: f.groups,
            width: w,
            height: h,
            selection,
            selected: f.selected,
            next_id: f.next_id,
        };
        self.history.clear();
        self.settled.clear();
        self.restore(doc);
        self.flat = Raster::new(w, h);
        self.tex = None;
        self.thumbs.clear();
        self.fit_pending = true;
        self.crop = None;
        self.text_draft = None;
        self.rename = None;
        self.stroking = false;
        self.stroke_src = None;
        self.drag_from = None;
        self.drag_to = None;
        self.lasso.clear();
        self.doc_name = f.doc_name;
        self.path = f.path;
        self.psd = f.psd;
        self.source_ppi = f.source_ppi;
        self.tool = Tool::from_id(&f.tool).unwrap_or(Tool::Brush);
        self.brush = f.brush;
        self.exposure = f.exposure;
        self.tolerance = f.tolerance;
        self.text_size = f.text_size;
        self.pen = f.pen;
        self.clone_offset = f.clone_offset;
        self.mark_all();
        Ok(())
    }
}

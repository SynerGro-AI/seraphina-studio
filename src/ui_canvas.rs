//! Safe pure UiCanvas (pixel model behind the Tuxedo Edit window).
//!
//! RGBA8 raster, per-layer blend and opacity, a paint layer with
//! brush and eraser dabs, rectangle selection, crop, filters and
//! view-space mapping. No egui, no unwraps, linear time.
//!
//! Differential: a different blend mode produces different pixels.

/// Straight-alpha RGBA8 raster.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, RGBA.
    pub data: Vec<u8>,
}

impl Raster {
    /// Fully transparent raster.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; (width as usize) * (height as usize) * 4],
        }
    }

    /// Solid colour raster.
    pub fn filled(width: u32, height: u32, rgba: [u8; 4]) -> Self {
        let mut r = Self::new(width, height);
        for px in r.data.as_chunks_mut::<4>().0 {
            px.copy_from_slice(&rgba);
        }
        r
    }

    /// Wrap raw bytes. None if the length does not match.
    pub fn from_rgba(width: u32, height: u32, data: Vec<u8>) -> Option<Self> {
        if data.len() != (width as usize) * (height as usize) * 4 {
            return None;
        }
        Some(Self {
            width,
            height,
            data,
        })
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = ((y as usize) * (self.width as usize) + x as usize) * 4;
        Some([
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ])
    }

    /// Nearest-neighbour resample to a new size.
    pub fn resized(&self, width: u32, height: u32) -> Self {
        if self.width == width && self.height == height {
            return self.clone();
        }
        let mut out = Self::new(width, height);
        if self.is_empty() {
            return out;
        }
        for y in 0..height {
            let sy = ((y as u64 * self.height as u64) / height.max(1) as u64) as u32;
            for x in 0..width {
                let sx = ((x as u64 * self.width as u64) / width.max(1) as u64) as u32;
                let s = ((sy as usize) * (self.width as usize) + sx as usize) * 4;
                let d = ((y as usize) * (width as usize) + x as usize) * 4;
                out.data[d..d + 4].copy_from_slice(&self.data[s..s + 4]);
            }
        }
        out
    }

    /// Copy of the region clamped to the raster. None if empty.
    pub fn cropped(&self, rect: PixelRect) -> Option<Self> {
        let r = rect.clamped(self.width, self.height)?;
        let mut out = Self::new(r.w, r.h);
        for row in 0..r.h {
            let s = (((r.y + row) as usize) * (self.width as usize) + r.x as usize) * 4;
            let d = (row as usize) * (r.w as usize) * 4;
            let n = (r.w as usize) * 4;
            out.data[d..d + n].copy_from_slice(&self.data[s..s + n]);
        }
        Some(out)
    }
}

/// Integer pixel rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl PixelRect {
    /// Rectangle spanning two corner points in any order.
    pub fn from_corners(a: (f32, f32), b: (f32, f32)) -> Self {
        let x0 = a.0.min(b.0).max(0.0);
        let y0 = a.1.min(b.1).max(0.0);
        let x1 = a.0.max(b.0).max(0.0);
        let y1 = a.1.max(b.1).max(0.0);
        Self {
            x: x0 as u32,
            y: y0 as u32,
            w: (x1 - x0).round() as u32,
            h: (y1 - y0).round() as u32,
        }
    }

    /// Clamp to a raster. None if nothing remains.
    pub fn clamped(&self, width: u32, height: u32) -> Option<Self> {
        if self.x >= width || self.y >= height {
            return None;
        }
        let w = self.w.min(width - self.x);
        let h = self.h.min(height - self.y);
        if w == 0 || h == 0 {
            return None;
        }
        Some(Self {
            x: self.x,
            y: self.y,
            w,
            h,
        })
    }

    pub fn contains(&self, x: u32, y: u32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
}

/// Layer blend modes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    SoftLight,
    Difference,
}

impl BlendMode {
    pub const ALL: [BlendMode; 8] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::SoftLight,
        BlendMode::Difference,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            BlendMode::Normal => "Normal",
            BlendMode::Multiply => "Multiply",
            BlendMode::Screen => "Screen",
            BlendMode::Overlay => "Overlay",
            BlendMode::Darken => "Darken",
            BlendMode::Lighten => "Lighten",
            BlendMode::SoftLight => "Soft Light",
            BlendMode::Difference => "Difference",
        }
    }

    /// Blend one channel. `base` and `top` in 0..=1.
    pub fn channel(&self, base: f32, top: f32) -> f32 {
        match self {
            BlendMode::Normal => top,
            BlendMode::Multiply => base * top,
            BlendMode::Screen => 1.0 - (1.0 - base) * (1.0 - top),
            BlendMode::Overlay => {
                if base < 0.5 {
                    2.0 * base * top
                } else {
                    1.0 - 2.0 * (1.0 - base) * (1.0 - top)
                }
            }
            BlendMode::Darken => base.min(top),
            BlendMode::Lighten => base.max(top),
            BlendMode::SoftLight => (1.0 - 2.0 * top) * base * base + 2.0 * top * base,
            BlendMode::Difference => (base - top).abs(),
        }
    }
}

/// Whole-image adjustment applied after compositing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Filter {
    None,
    Levels,
    Curves,
    Grayscale,
    Invert,
}

impl Filter {
    pub const ALL: [Filter; 5] = [
        Filter::None,
        Filter::Levels,
        Filter::Curves,
        Filter::Grayscale,
        Filter::Invert,
    ];

    pub fn label(&self) -> &'static str {
        match self {
            Filter::None => "None",
            Filter::Levels => "Levels",
            Filter::Curves => "Curves",
            Filter::Grayscale => "Grayscale",
            Filter::Invert => "Invert",
        }
    }

    /// Apply in place. `strength` is the Levels gain (ignored by others).
    pub fn apply(&self, r: &mut Raster, strength: f32) {
        match self {
            Filter::None => {}
            Filter::Levels => {
                let gain = strength.max(0.0);
                for px in r.data.as_chunks_mut::<4>().0 {
                    for c in px.iter_mut().take(3) {
                        *c = ((*c as f32 - 10.0).max(0.0) * gain).clamp(0.0, 255.0) as u8;
                    }
                }
            }
            Filter::Curves => {
                for px in r.data.as_chunks_mut::<4>().0 {
                    for c in px.iter_mut().take(3) {
                        let v = *c as f32 / 255.0;
                        *c = (v * v * 255.0).round() as u8;
                    }
                }
            }
            Filter::Grayscale => {
                for px in r.data.as_chunks_mut::<4>().0 {
                    let l = (0.299 * px[0] as f32 + 0.587 * px[1] as f32 + 0.114 * px[2] as f32)
                        .round() as u8;
                    px[0] = l;
                    px[1] = l;
                    px[2] = l;
                }
            }
            Filter::Invert => {
                for px in r.data.as_chunks_mut::<4>().0 {
                    for c in px.iter_mut().take(3) {
                        *c = 255 - *c;
                    }
                }
            }
        }
    }
}

/// One image layer.
#[derive(Clone, Debug)]
pub struct ImageLayer {
    pub name: String,
    pub visible: bool,
    /// 0..=1
    pub opacity: f32,
    pub blend: BlendMode,
    pub raster: Raster,
}

impl ImageLayer {
    pub fn new(name: &str, raster: Raster) -> Self {
        Self {
            name: name.to_string(),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            raster,
        }
    }
}

/// Composite visible layers bottom-to-top into a `width × height`
/// opaque raster. Layers of another size are resampled. Linear time.
pub fn composite(layers: &[ImageLayer], width: u32, height: u32, background: [u8; 3]) -> Raster {
    let mut out = Raster::filled(
        width,
        height,
        [background[0], background[1], background[2], 255],
    );
    for layer in layers.iter().filter(|l| l.visible && l.opacity > 0.0) {
        let resized;
        let src = if layer.raster.width == width && layer.raster.height == height {
            &layer.raster
        } else {
            resized = layer.raster.resized(width, height);
            &resized
        };
        blend_into(&mut out, src, layer.blend, layer.opacity);
    }
    out
}

/// Blend `top` over `base` (same size) with mode and opacity.
pub fn blend_into(base: &mut Raster, top: &Raster, mode: BlendMode, opacity: f32) {
    let op = opacity.clamp(0.0, 1.0);
    for (b, t) in base
        .data
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(top.data.as_chunks::<4>().0)
    {
        let a = (t[3] as f32 / 255.0) * op;
        if a <= 0.0 {
            continue;
        }
        for c in 0..3 {
            let bv = b[c] as f32 / 255.0;
            let tv = t[c] as f32 / 255.0;
            let mixed = mode.channel(bv, tv);
            b[c] = ((bv + (mixed - bv) * a) * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Brush settings.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct Brush {
    /// Diameter in image pixels.
    pub size: f32,
    /// 0..=1, 1 is a hard edge.
    pub hardness: f32,
    /// 0..=1 per dab.
    pub flow: f32,
    /// Spacing as a fraction of size.
    pub spacing: f32,
    pub color: [u8; 3],
}

impl Default for Brush {
    fn default() -> Self {
        Self {
            size: 24.0,
            hardness: 0.8,
            flow: 1.0,
            spacing: 0.25,
            color: [201, 168, 76],
        }
    }
}

impl Brush {
    /// Coverage 0..=1 at distance `d` from the dab centre.
    pub fn coverage(&self, d: f32) -> f32 {
        let r = (self.size / 2.0).max(0.5);
        if d >= r {
            return 0.0;
        }
        let hard = self.hardness.clamp(0.0, 1.0);
        let inner = r * hard;
        if d <= inner {
            1.0
        } else {
            1.0 - (d - inner) / (r - inner).max(1e-3)
        }
    }

    /// Stamp one dab. Erase lowers alpha instead of painting.
    /// Only pixels inside `clip` (if any) are touched.
    pub fn dab(&self, r: &mut Raster, cx: f32, cy: f32, erase: bool, clip: Option<PixelRect>) {
        let rad = self.size / 2.0;
        let x0 = (cx - rad).floor().max(0.0) as u32;
        let y0 = (cy - rad).floor().max(0.0) as u32;
        let x1 = ((cx + rad).ceil().max(0.0) as u32).min(r.width);
        let y1 = ((cy + rad).ceil().max(0.0) as u32).min(r.height);
        let flow = self.flow.clamp(0.0, 1.0);
        for y in y0..y1 {
            for x in x0..x1 {
                if let Some(c) = clip {
                    if !c.contains(x, y) {
                        continue;
                    }
                }
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cov = self.coverage(d) * flow;
                if cov <= 0.0 {
                    continue;
                }
                let i = ((y as usize) * (r.width as usize) + x as usize) * 4;
                let px = &mut r.data[i..i + 4];
                let a = px[3] as f32 / 255.0;
                if erase {
                    px[3] = ((a * (1.0 - cov)) * 255.0).round() as u8;
                } else {
                    let out_a = cov + a * (1.0 - cov);
                    for (p, &sc) in px.iter_mut().zip(&self.color).take(3) {
                        let v = (sc as f32 * cov + *p as f32 * a * (1.0 - cov)) / out_a.max(1e-6);
                        *p = v.round().clamp(0.0, 255.0) as u8;
                    }
                    px[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }

    /// Stamp dabs along a segment at the brush spacing. Returns dab count.
    pub fn stroke(
        &self,
        r: &mut Raster,
        from: (f32, f32),
        to: (f32, f32),
        erase: bool,
        clip: Option<PixelRect>,
    ) -> usize {
        let step = (self.size * self.spacing.clamp(0.05, 2.0)).max(1.0);
        let dx = to.0 - from.0;
        let dy = to.1 - from.1;
        let len = (dx * dx + dy * dy).sqrt();
        let n = (len / step).ceil().max(1.0) as usize;
        for i in 1..=n {
            let t = i as f32 / n as f32;
            self.dab(r, from.0 + dx * t, from.1 + dy * t, erase, clip);
        }
        n
    }
}

/// Maps between screen space and image space for a centred,
/// zoomed and panned image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewTransform {
    /// Viewport origin and size in screen points.
    pub viewport: (f32, f32, f32, f32),
    pub image: (u32, u32),
    /// Screen points per image pixel.
    pub zoom: f32,
    pub pan: (f32, f32),
}

impl ViewTransform {
    /// Zoom that fits the image in the viewport with a margin.
    pub fn fit_zoom(viewport_w: f32, viewport_h: f32, image: (u32, u32), margin: f32) -> f32 {
        if image.0 == 0 || image.1 == 0 {
            return 1.0;
        }
        let zw = (viewport_w - 2.0 * margin).max(1.0) / image.0 as f32;
        let zh = (viewport_h - 2.0 * margin).max(1.0) / image.1 as f32;
        zw.min(zh).max(0.01)
    }

    /// Top-left of the image on screen and its size.
    pub fn image_rect(&self) -> (f32, f32, f32, f32) {
        let w = self.image.0 as f32 * self.zoom;
        let h = self.image.1 as f32 * self.zoom;
        let x = self.viewport.0 + (self.viewport.2 - w) / 2.0 + self.pan.0;
        let y = self.viewport.1 + (self.viewport.3 - h) / 2.0 + self.pan.1;
        (x, y, w, h)
    }

    pub fn to_image(&self, sx: f32, sy: f32) -> (f32, f32) {
        let (x, y, _, _) = self.image_rect();
        ((sx - x) / self.zoom, (sy - y) / self.zoom)
    }

    pub fn to_screen(&self, ix: f32, iy: f32) -> (f32, f32) {
        let (x, y, _, _) = self.image_rect();
        (x + ix * self.zoom, y + iy * self.zoom)
    }

    /// True if the image point is inside the image.
    pub fn in_image(&self, ix: f32, iy: f32) -> bool {
        ix >= 0.0 && iy >= 0.0 && ix < self.image.0 as f32 && iy < self.image.1 as f32
    }
}

/// Zoom steps used by the zoom buttons and shortcuts.
pub fn zoom_step(current: f32, zoom_in: bool) -> f32 {
    const STEPS: [f32; 15] = [
        0.05, 0.1, 0.167, 0.25, 0.333, 0.5, 0.667, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 16.0,
    ];
    if zoom_in {
        STEPS
            .iter()
            .copied()
            .find(|s| *s > current * 1.001)
            .unwrap_or(16.0)
    } else {
        STEPS
            .iter()
            .rev()
            .copied()
            .find(|s| *s < current * 0.999)
            .unwrap_or(0.05)
    }
}

/// Bounded undo history of snapshots.
#[derive(Clone, Debug)]
pub struct History<T: Clone> {
    undo: Vec<T>,
    redo: Vec<T>,
    limit: usize,
}

impl<T: Clone> History<T> {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Record the state before a change.
    pub fn push(&mut self, before: T) {
        self.undo.push(before);
        if self.undo.len() > self.limit {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Most recent undo state, without popping it.
    pub fn peek(&self) -> Option<&T> {
        self.undo.last()
    }

    pub fn undo(&mut self, current: T) -> Option<T> {
        let prev = self.undo.pop()?;
        self.redo.push(current);
        Some(prev)
    }

    pub fn redo(&mut self, current: T) -> Option<T> {
        let next = self.redo.pop()?;
        self.undo.push(current);
        Some(next)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

/// `hh:mm:ss` for a duration in seconds.
pub fn format_clock(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        secs / 3600,
        (secs / 60) % 60,
        secs % 60
    )
}

/// Save target for a document: never the bundled asset itself.
/// `foo.psd` → `foo_edit.psd`; non-PSD paths get a `.psd` extension.
pub fn save_target(path: &str) -> String {
    let p = std::path::Path::new(path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("untitled");
    let name = if stem.ends_with("_edit") {
        format!("{}.psd", stem)
    } else {
        format!("{}_edit.psd", stem)
    };
    // Keep the caller's directory text as-is so the separator style is
    // unchanged (Path::join would insert `\` on Windows).
    let file = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let dir = path.strip_suffix(file).unwrap_or("");
    format!("{dir}{name}")
}

/// Animated scalar: keys sorted by time, linear interpolation,
/// held at both ends. Setting a key near an existing one replaces it.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct KeyTrack {
    keys: Vec<(f32, f32)>,
}

impl KeyTrack {
    pub const SNAP: f32 = 1e-3;

    pub fn new() -> Self {
        Self::default()
    }

    pub fn keys(&self) -> &[(f32, f32)] {
        &self.keys
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Insert or replace the key at `t`.
    pub fn set(&mut self, t: f32, v: f32) {
        if let Some(k) = self.keys.iter_mut().find(|k| (k.0 - t).abs() <= Self::SNAP) {
            k.1 = v;
            return;
        }
        self.keys.push((t, v));
        self.keys
            .sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    }

    /// Remove the key closest to `t` within `tol`. True if removed.
    pub fn remove_near(&mut self, t: f32, tol: f32) -> bool {
        let best = self
            .keys
            .iter()
            .enumerate()
            .filter(|(_, k)| (k.0 - t).abs() <= tol)
            .min_by(|a, b| {
                (a.1 .0 - t)
                    .abs()
                    .partial_cmp(&(b.1 .0 - t).abs())
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|(i, _)| i);
        match best {
            Some(i) => {
                self.keys.remove(i);
                true
            }
            None => false,
        }
    }

    pub fn has_key_near(&self, t: f32, tol: f32) -> bool {
        self.keys.iter().any(|k| (k.0 - t).abs() <= tol)
    }

    /// Linear sample. None if there are no keys.
    pub fn sample(&self, t: f32) -> Option<f32> {
        let first = self.keys.first()?;
        if t <= first.0 {
            return Some(first.1);
        }
        for w in self.keys.windows(2) {
            let (a, b) = (w[0], w[1]);
            if t <= b.0 {
                let span = (b.0 - a.0).max(1e-6);
                return Some(a.1 + (b.1 - a.1) * ((t - a.0) / span));
            }
        }
        self.keys.last().map(|k| k.1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Raster {
        Raster::filled(w, h, c)
    }

    #[test]
    fn from_rgba_checks_len() {
        assert!(Raster::from_rgba(2, 2, vec![0; 16]).is_some());
        assert!(Raster::from_rgba(2, 2, vec![0; 15]).is_none());
    }

    #[test]
    fn normal_full_opacity_replaces() {
        let layers = vec![
            ImageLayer::new("a", solid(4, 4, [10, 20, 30, 255])),
            ImageLayer::new("b", solid(4, 4, [200, 100, 50, 255])),
        ];
        let out = composite(&layers, 4, 4, [0, 0, 0]);
        assert_eq!(out.pixel(0, 0), Some([200, 100, 50, 255]));
    }

    #[test]
    fn opacity_half_mixes() {
        let mut top = ImageLayer::new("b", solid(1, 1, [255, 255, 255, 255]));
        top.opacity = 0.5;
        let layers = vec![ImageLayer::new("a", solid(1, 1, [0, 0, 0, 255])), top];
        let out = composite(&layers, 1, 1, [0, 0, 0]);
        let p = out.pixel(0, 0).unwrap_or([0; 4]);
        assert!((127..=128).contains(&p[0]));
    }

    #[test]
    fn hidden_layer_ignored() {
        let mut top = ImageLayer::new("b", solid(1, 1, [255, 0, 0, 255]));
        top.visible = false;
        let layers = vec![ImageLayer::new("a", solid(1, 1, [0, 0, 255, 255])), top];
        assert_eq!(
            composite(&layers, 1, 1, [0, 0, 0]).pixel(0, 0),
            Some([0, 0, 255, 255])
        );
    }

    #[test]
    fn no_visible_layers_gives_background() {
        let out = composite(&[], 2, 2, [9, 8, 7]);
        assert_eq!(out.pixel(1, 1), Some([9, 8, 7, 255]));
    }

    #[test]
    fn blend_modes_differ() {
        let base = solid(1, 1, [128, 128, 128, 255]);
        let top = solid(1, 1, [200, 200, 200, 255]);
        let mut seen = Vec::new();
        for m in BlendMode::ALL {
            let mut b = base.clone();
            blend_into(&mut b, &top, m, 1.0);
            seen.push(b.data[0]);
        }
        seen.dedup();
        assert_eq!(seen.len(), BlendMode::ALL.len());
    }

    #[test]
    fn multiply_and_screen_math() {
        assert!((BlendMode::Multiply.channel(0.5, 0.5) - 0.25).abs() < 1e-6);
        assert!((BlendMode::Screen.channel(0.5, 0.5) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn mismatched_sizes_resample() {
        let layers = vec![
            ImageLayer::new("a", solid(8, 8, [0, 0, 0, 255])),
            ImageLayer::new("b", solid(2, 2, [255, 255, 255, 255])),
        ];
        let out = composite(&layers, 8, 8, [0, 0, 0]);
        assert_eq!(out.pixel(7, 7), Some([255, 255, 255, 255]));
    }

    #[test]
    fn composite_large_is_linear() {
        // 1168×784 matches the bundled assets; the old UI hung here.
        let layers = vec![
            ImageLayer::new("a", solid(1168, 784, [1, 2, 3, 255])),
            ImageLayer::new("b", solid(1168, 784, [4, 5, 6, 255])),
        ];
        let t = std::time::Instant::now();
        let out = composite(&layers, 1168, 784, [0, 0, 0]);
        assert_eq!(out.data.len(), 1168 * 784 * 4);
        assert!(t.elapsed().as_secs() < 10);
    }

    #[test]
    fn filters_change_pixels() {
        for f in Filter::ALL {
            let mut r = solid(1, 1, [100, 150, 200, 255]);
            f.apply(&mut r, 1.2);
            if f == Filter::None {
                assert_eq!(r.data[..3], [100, 150, 200]);
            } else {
                assert_ne!(r.data[..3], [100, 150, 200], "{}", f.label());
            }
        }
    }

    #[test]
    fn grayscale_is_gray() {
        let mut r = solid(1, 1, [255, 0, 0, 255]);
        Filter::Grayscale.apply(&mut r, 1.0);
        assert_eq!(r.data[0], r.data[1]);
        assert_eq!(r.data[1], r.data[2]);
    }

    #[test]
    fn brush_paints_at_point_only() {
        let mut r = Raster::new(50, 50);
        let b = Brush {
            size: 10.0,
            hardness: 1.0,
            flow: 1.0,
            spacing: 0.25,
            color: [255, 0, 0],
        };
        b.dab(&mut r, 25.0, 25.0, false, None);
        assert_eq!(r.pixel(25, 25), Some([255, 0, 0, 255]));
        assert_eq!(r.pixel(0, 0), Some([0, 0, 0, 0]));
        assert_eq!(r.pixel(40, 25).map(|p| p[3]), Some(0));
    }

    #[test]
    fn eraser_clears_alpha() {
        let mut r = Raster::filled(20, 20, [255, 255, 255, 255]);
        let b = Brush {
            size: 8.0,
            hardness: 1.0,
            flow: 1.0,
            ..Brush::default()
        };
        b.dab(&mut r, 10.0, 10.0, true, None);
        assert_eq!(r.pixel(10, 10).map(|p| p[3]), Some(0));
        assert_eq!(r.pixel(0, 0).map(|p| p[3]), Some(255));
    }

    #[test]
    fn dab_respects_clip() {
        let mut r = Raster::new(20, 20);
        let b = Brush {
            size: 20.0,
            hardness: 1.0,
            ..Brush::default()
        };
        b.dab(
            &mut r,
            10.0,
            10.0,
            false,
            Some(PixelRect {
                x: 0,
                y: 0,
                w: 10,
                h: 20,
            }),
        );
        assert_eq!(r.pixel(5, 10).map(|p| p[3]), Some(255));
        assert_eq!(r.pixel(15, 10).map(|p| p[3]), Some(0));
    }

    #[test]
    fn dab_off_canvas_is_safe() {
        let mut r = Raster::new(5, 5);
        Brush::default().dab(&mut r, -100.0, 900.0, false, None);
        assert!(r.data.iter().all(|b| *b == 0));
    }

    #[test]
    fn stroke_fills_gap() {
        let mut r = Raster::new(100, 10);
        let b = Brush {
            size: 4.0,
            hardness: 1.0,
            ..Brush::default()
        };
        let n = b.stroke(&mut r, (2.0, 5.0), (98.0, 5.0), false, None);
        assert!(n > 10);
        assert!(r.pixel(50, 5).map(|p| p[3]).unwrap_or(0) > 0);
    }

    #[test]
    fn soft_brush_falls_off() {
        let b = Brush {
            size: 20.0,
            hardness: 0.0,
            ..Brush::default()
        };
        assert!(b.coverage(0.0) > b.coverage(5.0));
        assert_eq!(b.coverage(10.0), 0.0);
    }

    #[test]
    fn rect_from_corners_any_order() {
        let r = PixelRect::from_corners((10.0, 20.0), (2.0, 4.0));
        assert_eq!(
            r,
            PixelRect {
                x: 2,
                y: 4,
                w: 8,
                h: 16
            }
        );
    }

    #[test]
    fn crop_clamps() {
        let r = solid(10, 10, [1, 2, 3, 255]);
        let c = r.cropped(PixelRect {
            x: 8,
            y: 8,
            w: 10,
            h: 10,
        });
        assert_eq!(c.map(|c| (c.width, c.height)), Some((2, 2)));
        assert!(r
            .cropped(PixelRect {
                x: 20,
                y: 0,
                w: 1,
                h: 1
            })
            .is_none());
    }

    #[test]
    fn view_round_trip() {
        let v = ViewTransform {
            viewport: (100.0, 50.0, 800.0, 600.0),
            image: (400, 300),
            zoom: 1.5,
            pan: (10.0, -5.0),
        };
        let (sx, sy) = v.to_screen(123.0, 45.0);
        let (ix, iy) = v.to_image(sx, sy);
        assert!((ix - 123.0).abs() < 1e-3 && (iy - 45.0).abs() < 1e-3);
    }

    #[test]
    fn fit_zoom_fits() {
        let z = ViewTransform::fit_zoom(800.0, 600.0, (1168, 784), 16.0);
        assert!(1168.0 * z <= 800.0 && 784.0 * z <= 600.0);
        assert_eq!(ViewTransform::fit_zoom(800.0, 600.0, (0, 0), 0.0), 1.0);
    }

    #[test]
    fn zoom_steps_move() {
        assert!(zoom_step(1.0, true) > 1.0);
        assert!(zoom_step(1.0, false) < 1.0);
        assert_eq!(zoom_step(16.0, true), 16.0);
        assert_eq!(zoom_step(0.05, false), 0.05);
    }

    #[test]
    fn history_undo_redo() {
        let mut h = History::new(2);
        h.push(1);
        h.push(2);
        h.push(3);
        assert_eq!(h.undo(4), Some(3));
        assert_eq!(h.undo(3), Some(2));
        assert_eq!(h.undo(2), None);
        assert!(h.can_redo());
        assert_eq!(h.redo(2), Some(3));
    }

    #[test]
    fn clock_format() {
        assert_eq!(format_clock(0), "00:00:00");
        assert_eq!(format_clock(2852), "00:47:32");
        assert_eq!(format_clock(3661), "01:01:01");
    }

    #[test]
    fn save_target_never_overwrites_source() {
        assert_eq!(
            save_target("assets/mountain_spire.psd"),
            "assets/mountain_spire_edit.psd"
        );
        assert_eq!(
            save_target("assets/mountain_spire_edit.psd"),
            "assets/mountain_spire_edit.psd"
        );
        assert_eq!(save_target("photo.jpg"), "photo_edit.psd");
        assert_eq!(save_target(r"C:\art\photo.jpg"), r"C:\art\photo_edit.psd");
    }

    #[test]
    fn key_track_interpolates() {
        let mut k = KeyTrack::new();
        assert_eq!(k.sample(0.0), None);
        k.set(1.0, 10.0);
        k.set(0.0, 0.0);
        assert_eq!(k.sample(-1.0), Some(0.0));
        assert_eq!(k.sample(0.5), Some(5.0));
        assert_eq!(k.sample(9.0), Some(10.0));
    }

    #[test]
    fn key_track_replace_and_remove() {
        let mut k = KeyTrack::new();
        k.set(1.0, 1.0);
        k.set(1.0, 2.0);
        assert_eq!(k.keys().len(), 1);
        assert_eq!(k.sample(1.0), Some(2.0));
        assert!(k.has_key_near(1.02, 0.05));
        assert!(!k.remove_near(3.0, 0.1));
        assert!(k.remove_near(1.02, 0.05));
        assert!(k.is_empty());
    }
}

//! Paint engine behind Seraphina Studio's Paint mode.
//!
//! Pure and egui-free: layer stack with groups, offsets, fill and
//! layer masks (density + feather), selection masks (rectangle,
//! polygon/lasso, magic wand), and the raster tools (brush, eraser,
//! gradient, dodge/burn, clone stamp, healing-lite). Rasters are
//! shared through `Arc` so undo snapshots only copy what changed.

use crate::ui_canvas::{BlendMode, Brush, PixelRect, Raster};
use std::sync::Arc;

// ---- selection ------------------------------------------------------------

/// 8-bit selection mask the size of the document. 255 = selected.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    pub width: u32,
    pub height: u32,
    pub mask: Vec<u8>,
}

impl Selection {
    pub fn empty(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            mask: vec![0; (width as usize) * (height as usize)],
        }
    }

    pub fn all(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            mask: vec![255; (width as usize) * (height as usize)],
        }
    }

    pub fn from_rect(width: u32, height: u32, r: PixelRect) -> Self {
        let mut s = Self::empty(width, height);
        if let Some(r) = r.clamped(width, height) {
            for y in r.y..r.y + r.h {
                let row = (y as usize) * (width as usize);
                for x in r.x..r.x + r.w {
                    s.mask[row + x as usize] = 255;
                }
            }
        }
        s
    }

    /// Even-odd scanline fill of a closed polygon (lasso / pen path).
    pub fn from_polygon(width: u32, height: u32, pts: &[(f32, f32)]) -> Self {
        let mut s = Self::empty(width, height);
        if pts.len() < 3 {
            return s;
        }
        let mut xs: Vec<f32> = Vec::new();
        for y in 0..height {
            let fy = y as f32 + 0.5;
            xs.clear();
            for i in 0..pts.len() {
                let (x0, y0) = pts[i];
                let (x1, y1) = pts[(i + 1) % pts.len()];
                if (y0 <= fy && y1 > fy) || (y1 <= fy && y0 > fy) {
                    xs.push(x0 + (fy - y0) / (y1 - y0) * (x1 - x0));
                }
            }
            xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            for pair in xs.as_chunks::<2>().0 {
                let a = pair[0].round().clamp(0.0, width as f32) as u32;
                let b = pair[1].round().clamp(0.0, width as f32) as u32;
                let row = (y as usize) * (width as usize);
                for x in a..b {
                    s.mask[row + x as usize] = 255;
                }
            }
        }
        s
    }

    /// 4-connected flood fill of pixels within `tolerance` (max channel
    /// difference) of the seed colour.
    pub fn magic_wand(r: &Raster, seed: (u32, u32), tolerance: u8) -> Self {
        let mut s = Self::empty(r.width, r.height);
        let Some(c0) = r.pixel(seed.0, seed.1) else {
            return s;
        };
        let close = |p: [u8; 4]| {
            (0..3).all(|k| (p[k] as i16 - c0[k] as i16).unsigned_abs() as u8 <= tolerance)
        };
        let w = r.width as usize;
        let mut stack = vec![(seed.0, seed.1)];
        while let Some((x, y)) = stack.pop() {
            let i = (y as usize) * w + x as usize;
            if s.mask[i] != 0 {
                continue;
            }
            match r.pixel(x, y) {
                Some(p) if close(p) => {}
                _ => continue,
            }
            s.mask[i] = 255;
            if x > 0 {
                stack.push((x - 1, y));
            }
            if y > 0 {
                stack.push((x, y - 1));
            }
            if x + 1 < r.width {
                stack.push((x + 1, y));
            }
            if y + 1 < r.height {
                stack.push((x, y + 1));
            }
        }
        s
    }

    /// Edge-aware flood ("Quick Select"): grows while each step to a
    /// neighbour stays small (photo gradients) and the colour stays
    /// within a looser bound of the seed.
    pub fn quick_select(r: &Raster, seed: (u32, u32), tolerance: u8) -> Self {
        let mut s = Self::empty(r.width, r.height);
        let Some(c0) = r.pixel(seed.0, seed.1) else {
            return s;
        };
        let step = (tolerance as i16 / 3).max(4);
        let span = (tolerance as i16 * 3).min(255);
        let diff = |a: [u8; 4], b: [u8; 4]| {
            (0..3)
                .map(|k| (a[k] as i16 - b[k] as i16).abs())
                .max()
                .unwrap_or(0)
        };
        let w = r.width as usize;
        s.mask[(seed.1 as usize) * w + seed.0 as usize] = 255;
        let mut stack = vec![(seed.0, seed.1, c0)];
        while let Some((x, y, from)) = stack.pop() {
            let mut visit = |nx: u32, ny: u32| {
                let i = (ny as usize) * w + nx as usize;
                if s.mask[i] != 0 {
                    return;
                }
                if let Some(p) = r.pixel(nx, ny) {
                    if diff(p, from) <= step && diff(p, c0) <= span {
                        s.mask[i] = 255;
                        stack.push((nx, ny, p));
                    }
                }
            };
            if x > 0 {
                visit(x - 1, y);
            }
            if y > 0 {
                visit(x, y - 1);
            }
            if x + 1 < r.width {
                visit(x + 1, y);
            }
            if y + 1 < r.height {
                visit(x, y + 1);
            }
        }
        s
    }

    #[inline]
    pub fn at(&self, x: u32, y: u32) -> u8 {
        if x >= self.width || y >= self.height {
            return 0;
        }
        self.mask[(y as usize) * (self.width as usize) + x as usize]
    }

    pub fn count(&self) -> usize {
        self.mask.iter().filter(|v| **v > 0).count()
    }

    pub fn is_empty(&self) -> bool {
        self.mask.iter().all(|v| *v == 0)
    }

    pub fn invert(&mut self) {
        for v in &mut self.mask {
            *v = 255 - *v;
        }
    }

    /// Tight bounding box of the selected pixels.
    pub fn bounds(&self) -> Option<PixelRect> {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..self.height {
            let row = (y as usize) * (self.width as usize);
            for x in 0..self.width {
                if self.mask[row + x as usize] > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        if x0 == u32::MAX {
            None
        } else {
            Some(PixelRect {
                x: x0,
                y: y0,
                w: x1 - x0 + 1,
                h: y1 - y0 + 1,
            })
        }
    }

    pub fn cropped(&self, r: PixelRect) -> Self {
        let mut out = Self::empty(r.w, r.h);
        for y in 0..r.h {
            for x in 0..r.w {
                out.mask[(y as usize) * (r.w as usize) + x as usize] = self.at(r.x + x, r.y + y);
            }
        }
        out
    }
}

/// Selection factor 0..=1 at a pixel (1 when nothing is selected).
#[inline]
fn sel_factor(sel: Option<&Selection>, x: u32, y: u32) -> f32 {
    match sel {
        Some(s) => s.at(x, y) as f32 / 255.0,
        None => 1.0,
    }
}

// ---- masks ------------------------------------------------------------------

/// Separable box blur, three passes (close to a Gaussian).
pub fn blur_mask(data: &[u8], w: u32, h: u32, radius: u32) -> Vec<u8> {
    if radius == 0 || w == 0 || h == 0 {
        return data.to_vec();
    }
    let (w, h, r) = (w as usize, h as usize, radius as usize);
    let mut a: Vec<f32> = data.iter().map(|v| *v as f32).collect();
    let mut b = vec![0f32; a.len()];
    for _ in 0..3 {
        for y in 0..h {
            let row = y * w;
            let mut sum = 0f32;
            let mut n = 0f32;
            for x in 0..=r.min(w - 1) {
                sum += a[row + x];
                n += 1.0;
            }
            for x in 0..w {
                b[row + x] = sum / n;
                if x + r + 1 < w {
                    sum += a[row + x + r + 1];
                    n += 1.0;
                }
                if x >= r {
                    sum -= a[row + x - r];
                    n -= 1.0;
                }
            }
        }
        for x in 0..w {
            let mut sum = 0f32;
            let mut n = 0f32;
            for y in 0..=r.min(h - 1) {
                sum += b[y * w + x];
                n += 1.0;
            }
            for y in 0..h {
                a[y * w + x] = sum / n;
                if y + r + 1 < h {
                    sum += b[(y + r + 1) * w + x];
                    n += 1.0;
                }
                if y >= r {
                    sum -= b[(y - r) * w + x];
                    n -= 1.0;
                }
            }
        }
    }
    a.iter()
        .map(|v| v.round().clamp(0.0, 255.0) as u8)
        .collect()
}

/// Grayscale layer mask. White reveals, black hides.
#[derive(Clone, Debug)]
pub struct LayerMask {
    pub width: u32,
    pub height: u32,
    pub data: Arc<Vec<u8>>,
    /// 0..=1. 1 = mask fully applied, 0 = no effect.
    pub density: f32,
    /// Blur radius in pixels.
    pub feather: f32,
    pub invert: bool,
    pub enabled: bool,
    cache: Option<(u32, Arc<Vec<u8>>)>,
}

impl LayerMask {
    pub fn from_selection(sel: &Selection) -> Self {
        Self {
            width: sel.width,
            height: sel.height,
            data: Arc::new(sel.mask.clone()),
            density: 1.0,
            feather: 0.0,
            invert: false,
            enabled: true,
            cache: None,
        }
    }

    pub fn reveal_all(width: u32, height: u32) -> Self {
        Self::from_selection(&Selection::all(width, height))
    }

    /// Recompute the feathered copy if the radius changed.
    pub fn prepare(&mut self) {
        let r = self.feather.max(0.0).round() as u32;
        if r == 0 {
            self.cache = None;
            return;
        }
        if self.cache.as_ref().map(|c| c.0) != Some(r) {
            self.cache = Some((
                r,
                Arc::new(blur_mask(&self.data, self.width, self.height, r)),
            ));
        }
    }

    pub fn invalidate(&mut self) {
        self.cache = None;
    }

    /// Alpha multiplier 0..=1 at a pixel.
    #[inline]
    pub fn factor(&self, x: u32, y: u32) -> f32 {
        if !self.enabled || x >= self.width || y >= self.height {
            return 1.0;
        }
        let r = self.feather.max(0.0).round() as u32;
        let src: &[u8] = match &self.cache {
            Some((cr, d)) if *cr == r => d,
            _ => &self.data,
        };
        let mut m = src[(y as usize) * (self.width as usize) + x as usize] as f32 / 255.0;
        if self.invert {
            m = 1.0 - m;
        }
        1.0 - self.density.clamp(0.0, 1.0) * (1.0 - m)
    }

    pub fn cropped(&self, r: PixelRect) -> Self {
        let sel = Selection {
            width: self.width,
            height: self.height,
            mask: (*self.data).clone(),
        }
        .cropped(r);
        let mut m = Self::from_selection(&sel);
        m.density = self.density;
        m.feather = self.feather;
        m.invert = self.invert;
        m.enabled = self.enabled;
        m
    }
}

// ---- layers -----------------------------------------------------------------

/// Text layer source, re-rasterised whenever it changes.
#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct TextSpec {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub size: f32,
    pub color: [u8; 3],
}

#[derive(Clone, Debug)]
pub struct PaintLayer {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    /// 0..=1
    pub opacity: f32,
    /// 0..=1. No layer effects exist yet, so fill multiplies like opacity.
    pub fill: f32,
    pub blend: BlendMode,
    pub raster: Arc<Raster>,
    pub offset: (i32, i32),
    pub mask: Option<LayerMask>,
    pub group: Option<u32>,
    pub locked: bool,
    pub text: Option<TextSpec>,
}

impl PaintLayer {
    pub fn new(id: u32, name: &str, raster: Raster) -> Self {
        Self {
            id,
            name: name.to_string(),
            visible: true,
            opacity: 1.0,
            fill: 1.0,
            blend: BlendMode::Normal,
            raster: Arc::new(raster),
            offset: (0, 0),
            mask: None,
            group: None,
            locked: false,
            text: None,
        }
    }

    pub fn raster_mut(&mut self) -> &mut Raster {
        Arc::make_mut(&mut self.raster)
    }

    /// Move the pixels by the offset and reset it (before crop/paint).
    pub fn bake_offset(&mut self) {
        let (w, h) = (self.raster.width, self.raster.height);
        self.bake_to(w, h);
    }

    /// Make the raster exactly `w` × `h` at offset (0, 0), keeping what
    /// lies inside the document. Undoes [`PaintLayer::trim`] before tools
    /// that work in document coordinates.
    pub fn bake_to(&mut self, w: u32, h: u32) {
        if self.offset == (0, 0) && (self.raster.width, self.raster.height) == (w, h) {
            return;
        }
        let src = self.raster.clone();
        let (dx, dy) = self.offset;
        let mut out = Raster::new(w, h);
        let (sw, sh) = (src.width as i64, src.height as i64);
        for y in 0..h as i64 {
            let sy = y - dy as i64;
            if sy < 0 || sy >= sh {
                continue;
            }
            let x0 = (dx as i64).max(0);
            let x1 = (dx as i64 + sw).min(w as i64);
            if x0 >= x1 {
                continue;
            }
            let s = ((sy * sw + (x0 - dx as i64)) * 4) as usize;
            let d = ((y * w as i64 + x0) * 4) as usize;
            let n = ((x1 - x0) * 4) as usize;
            out.data[d..d + n].copy_from_slice(&src.data[s..s + n]);
        }
        self.raster = Arc::new(out);
        self.offset = (0, 0);
    }

    /// Shrink the raster to the box holding every non-zero byte, moving the
    /// offset so the document looks the same. Lossless; text layers (which
    /// re-render at document size) are left alone. Returns true if trimmed.
    pub fn trim(&mut self) -> bool {
        if self.text.is_some() {
            return false;
        }
        let r = &self.raster;
        let (w, h) = (r.width as usize, r.height as usize);
        let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0usize, 0usize);
        for y in 0..h {
            let row = &r.data[y * w * 4..(y + 1) * w * 4];
            let px = row.as_chunks::<4>().0;
            let Some(first) = px.iter().position(|p| *p != [0; 4]) else {
                continue;
            };
            let last = px.iter().rposition(|p| *p != [0; 4]).unwrap();
            x0 = x0.min(first);
            x1 = x1.max(last + 1);
            y0 = y0.min(y);
            y1 = y + 1;
        }
        if x0 >= x1 {
            if w * h == 0 {
                return false;
            }
            self.raster = Arc::new(Raster::new(0, 0));
            self.offset = (0, 0);
            return true;
        }
        let (bw, bh) = (x1 - x0, y1 - y0);
        // Not worth a copy unless it frees at least a quarter.
        if bw * bh * 4 > w * h * 3 {
            return false;
        }
        let mut out = Raster::new(bw as u32, bh as u32);
        for y in 0..bh {
            let s = ((y0 + y) * w + x0) * 4;
            out.data[y * bw * 4..(y + 1) * bw * 4].copy_from_slice(&r.data[s..s + bw * 4]);
        }
        self.raster = Arc::new(out);
        self.offset = (self.offset.0 + x0 as i32, self.offset.1 + y0 as i32);
        true
    }
}

/// Thumbnail of a layer as it sits in a `w` × `h` document (honours
/// trimming and offsets; nearest neighbour).
pub fn layer_thumbnail(l: &PaintLayer, w: u32, h: u32, max: u32) -> Raster {
    if w == 0 || h == 0 {
        return Raster::new(1, 1);
    }
    let s = (max as f32 / w.max(h) as f32).min(1.0);
    let tw = ((w as f32 * s).round() as u32).max(1);
    let th = ((h as f32 * s).round() as u32).max(1);
    let mut out = Raster::new(tw, th);
    let r = &l.raster;
    for ty in 0..th {
        let dy = (ty as u64 * h as u64 / th as u64) as i64 - l.offset.1 as i64;
        if dy < 0 || dy >= r.height as i64 {
            continue;
        }
        for tx in 0..tw {
            let dx = (tx as u64 * w as u64 / tw as u64) as i64 - l.offset.0 as i64;
            if dx < 0 || dx >= r.width as i64 {
                continue;
            }
            let si = ((dy * r.width as i64 + dx) * 4) as usize;
            let di = ((ty * tw + tx) * 4) as usize;
            out.data[di..di + 4].copy_from_slice(&r.data[si..si + 4]);
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ui", derive(serde::Serialize, serde::Deserialize))]
pub struct LayerGroup {
    pub id: u32,
    pub name: String,
    pub visible: bool,
    pub collapsed: bool,
}

/// Composite visible layers bottom-to-top into `out` (document sized),
/// touching only `region` when given. Linear in pixels × layers.
pub fn composite_into(
    out: &mut Raster,
    layers: &[PaintLayer],
    groups: &[LayerGroup],
    background: [u8; 3],
    region: Option<PixelRect>,
) {
    let full = PixelRect {
        x: 0,
        y: 0,
        w: out.width,
        h: out.height,
    };
    let Some(r) = region.unwrap_or(full).clamped(out.width, out.height) else {
        return;
    };
    let group_visible = |g: Option<u32>| {
        g.map(|id| {
            groups
                .iter()
                .find(|x| x.id == id)
                .map(|x| x.visible)
                .unwrap_or(true)
        })
        .unwrap_or(true)
    };
    let ow = out.width as usize;
    for y in r.y..r.y + r.h {
        for x in r.x..r.x + r.w {
            let i = ((y as usize) * ow + x as usize) * 4;
            out.data[i] = background[0];
            out.data[i + 1] = background[1];
            out.data[i + 2] = background[2];
            out.data[i + 3] = 255;
        }
    }
    for l in layers {
        let op = l.opacity.clamp(0.0, 1.0) * l.fill.clamp(0.0, 1.0);
        if !l.visible || op <= 0.0 || !group_visible(l.group) {
            continue;
        }
        let src = &l.raster;
        let (ox, oy) = l.offset;
        for y in r.y..r.y + r.h {
            let sy = y as i32 - oy;
            if sy < 0 || sy >= src.height as i32 {
                continue;
            }
            for x in r.x..r.x + r.w {
                let sx = x as i32 - ox;
                if sx < 0 || sx >= src.width as i32 {
                    continue;
                }
                let s = ((sy as usize) * (src.width as usize) + sx as usize) * 4;
                let mut a = src.data[s + 3] as f32 / 255.0 * op;
                if a <= 0.0 {
                    continue;
                }
                if let Some(m) = &l.mask {
                    a *= m.factor(x, y);
                    if a <= 0.0 {
                        continue;
                    }
                }
                let d = ((y as usize) * ow + x as usize) * 4;
                for c in 0..3 {
                    let bv = out.data[d + c] as f32 / 255.0;
                    let tv = src.data[s + c] as f32 / 255.0;
                    let mixed = l.blend.channel(bv, tv);
                    out.data[d + c] =
                        ((bv + (mixed - bv) * a) * 255.0).round().clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
}

/// Small RGBA thumbnail (nearest neighbour, aspect preserved).
pub fn thumbnail(r: &Raster, max: u32) -> Raster {
    if r.is_empty() {
        return Raster::new(1, 1);
    }
    let s = (max as f32 / r.width.max(r.height) as f32).min(1.0);
    let w = ((r.width as f32 * s).round() as u32).max(1);
    let h = ((r.height as f32 * s).round() as u32).max(1);
    r.resized(w, h)
}

// ---- tools ------------------------------------------------------------------

fn dab_bounds(r: &Raster, cx: f32, cy: f32, rad: f32) -> (u32, u32, u32, u32) {
    let x0 = (cx - rad).floor().max(0.0) as u32;
    let y0 = (cy - rad).floor().max(0.0) as u32;
    let x1 = ((cx + rad).ceil().max(0.0) as u32).min(r.width);
    let y1 = ((cy + rad).ceil().max(0.0) as u32).min(r.height);
    (x0, y0, x1, y1)
}

#[inline]
fn dist(x: u32, y: u32, cx: f32, cy: f32) -> f32 {
    ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt()
}

/// Brush or eraser dab honouring a soft selection mask.
pub fn paint_dab(
    r: &mut Raster,
    b: &Brush,
    cx: f32,
    cy: f32,
    erase: bool,
    sel: Option<&Selection>,
) {
    let (x0, y0, x1, y1) = dab_bounds(r, cx, cy, b.size / 2.0);
    let flow = b.flow.clamp(0.0, 1.0);
    for y in y0..y1 {
        for x in x0..x1 {
            let cov = b.coverage(dist(x, y, cx, cy)) * flow * sel_factor(sel, x, y);
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
                for (p, &bc) in px.iter_mut().zip(&b.color).take(3) {
                    let v = (bc as f32 * cov + *p as f32 * a * (1.0 - cov)) / out_a.max(1e-6);
                    *p = v.round().clamp(0.0, 255.0) as u8;
                }
                px[3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

/// Positions of dabs along a segment at the brush spacing (excluding `from`).
pub fn dab_points(b: &Brush, from: (f32, f32), to: (f32, f32)) -> Vec<(f32, f32)> {
    let step = (b.size * b.spacing.clamp(0.05, 2.0)).max(1.0);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let n = ((dx * dx + dy * dy).sqrt() / step).ceil().max(1.0) as usize;
    (1..=n)
        .map(|i| {
            let t = i as f32 / n as f32;
            (from.0 + dx * t, from.1 + dy * t)
        })
        .collect()
}

/// Dabs every `spacing` pixels along `from → to`, where `from` is the last
/// dab placed. Returns the new dabs; a pointer that hasn't moved a full
/// spacing yet (or is paused) yields none, so tools don't build up in place.
pub fn spaced_dab_points(b: &Brush, from: (f32, f32), to: (f32, f32)) -> Vec<(f32, f32)> {
    let step = (b.size * b.spacing.clamp(0.05, 2.0)).max(1.0);
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let dist = (dx * dx + dy * dy).sqrt();
    let n = (dist / step + 1e-4).floor() as usize;
    if n == 0 {
        return Vec::new();
    }
    let (ux, uy) = (dx / dist, dy / dist);
    (1..=n.min(100_000))
        .map(|k| (from.0 + ux * step * k as f32, from.1 + uy * step * k as f32))
        .collect()
}

/// Dodge (lighten) or burn (darken) under the brush. Alpha untouched.
pub fn tone_dab(
    r: &mut Raster,
    b: &Brush,
    cx: f32,
    cy: f32,
    exposure: f32,
    dodge: bool,
    sel: Option<&Selection>,
) {
    let (x0, y0, x1, y1) = dab_bounds(r, cx, cy, b.size / 2.0);
    let e = exposure.clamp(0.0, 1.0) * 0.25;
    for y in y0..y1 {
        for x in x0..x1 {
            let cov = b.coverage(dist(x, y, cx, cy)) * sel_factor(sel, x, y) * e;
            if cov <= 0.0 {
                continue;
            }
            let i = ((y as usize) * (r.width as usize) + x as usize) * 4;
            for c in 0..3 {
                let v = r.data[i + c] as f32;
                let nv = if dodge {
                    v + (255.0 - v) * cov
                } else {
                    v * (1.0 - cov)
                };
                r.data[i + c] = nv.round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

fn sample(src: &Raster, x: i64, y: i64) -> Option<[f32; 4]> {
    if x < 0 || y < 0 || x >= src.width as i64 || y >= src.height as i64 {
        return None;
    }
    let i = ((y as usize) * (src.width as usize) + x as usize) * 4;
    Some([
        src.data[i] as f32,
        src.data[i + 1] as f32,
        src.data[i + 2] as f32,
        src.data[i + 3] as f32,
    ])
}

fn mix_px(r: &mut Raster, x: u32, y: u32, c: [f32; 4], cov: f32) {
    let i = ((y as usize) * (r.width as usize) + x as usize) * 4;
    for (k, &ck) in c.iter().enumerate().take(4) {
        let v = r.data[i + k] as f32;
        r.data[i + k] = (v + (ck - v) * cov).round().clamp(0.0, 255.0) as u8;
    }
}

/// Clone stamp: copy from `src` (snapshot at stroke start) at
/// `offset` (source minus destination) under the brush.
pub fn clone_dab(
    r: &mut Raster,
    src: &Raster,
    b: &Brush,
    cx: f32,
    cy: f32,
    offset: (f32, f32),
    sel: Option<&Selection>,
) {
    let (x0, y0, x1, y1) = dab_bounds(r, cx, cy, b.size / 2.0);
    let (ox, oy) = (offset.0.round() as i64, offset.1.round() as i64);
    for y in y0..y1 {
        for x in x0..x1 {
            let cov =
                b.coverage(dist(x, y, cx, cy)) * b.flow.clamp(0.0, 1.0) * sel_factor(sel, x, y);
            if cov <= 0.0 {
                continue;
            }
            if let Some(c) = sample(src, x as i64 + ox, y as i64 + oy) {
                mix_px(r, x, y, c, cov);
            }
        }
    }
}

fn ring_mean(src: &Raster, cx: f32, cy: f32, r0: f32, r1: f32) -> Option<[f32; 3]> {
    let mut acc = [0f64; 3];
    let mut n = 0f64;
    let (x0, y0, x1, y1) = dab_bounds(src, cx, cy, r1);
    for y in y0..y1 {
        for x in x0..x1 {
            let d = dist(x, y, cx, cy);
            if d >= r0 && d < r1 {
                if let Some(p) = sample(src, x as i64, y as i64) {
                    for k in 0..3 {
                        acc[k] += p[k] as f64;
                    }
                    n += 1.0;
                }
            }
        }
    }
    if n == 0.0 {
        None
    } else {
        Some([
            (acc[0] / n) as f32,
            (acc[1] / n) as f32,
            (acc[2] / n) as f32,
        ])
    }
}

/// Healing-lite (spot heal): take texture from a nearby patch and
/// shift its tone to match the ring around the dab.
pub fn heal_dab(
    r: &mut Raster,
    src: &Raster,
    b: &Brush,
    cx: f32,
    cy: f32,
    sel: Option<&Selection>,
) {
    let rad = (b.size / 2.0).max(1.0);
    let off = if cx + rad * 3.0 < src.width as f32 {
        rad * 2.2
    } else {
        -rad * 2.2
    };
    let (Some(dest_ring), Some(src_ring)) = (
        ring_mean(src, cx, cy, rad, rad * 1.5),
        ring_mean(src, cx + off, cy, rad, rad * 1.5),
    ) else {
        return;
    };
    let (x0, y0, x1, y1) = dab_bounds(r, cx, cy, rad);
    for y in y0..y1 {
        for x in x0..x1 {
            let cov = b.coverage(dist(x, y, cx, cy)) * sel_factor(sel, x, y);
            if cov <= 0.0 {
                continue;
            }
            if let (Some(s), Some(orig)) = (
                sample(src, (x as f32 + off).round() as i64, y as i64),
                sample(src, x as i64, y as i64),
            ) {
                let c = [
                    s[0] + dest_ring[0] - src_ring[0],
                    s[1] + dest_ring[1] - src_ring[1],
                    s[2] + dest_ring[2] - src_ring[2],
                    orig[3],
                ];
                mix_px(r, x, y, c, cov);
            }
        }
    }
}

/// Linear gradient from `a` to `b`, colour `c0` to `c1` (RGBA),
/// composited over the existing pixels inside the selection.
pub fn gradient_fill(
    r: &mut Raster,
    a: (f32, f32),
    b: (f32, f32),
    c0: [u8; 4],
    c1: [u8; 4],
    sel: Option<&Selection>,
) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = (dx * dx + dy * dy).max(1e-3);
    for y in 0..r.height {
        for x in 0..r.width {
            let f = sel_factor(sel, x, y);
            if f <= 0.0 {
                continue;
            }
            let t = (((x as f32 + 0.5 - a.0) * dx + (y as f32 + 0.5 - a.1) * dy) / len2)
                .clamp(0.0, 1.0);
            let col: Vec<f32> = (0..4)
                .map(|k| c0[k] as f32 + (c1[k] as f32 - c0[k] as f32) * t)
                .collect();
            let ga = col[3] / 255.0 * f;
            let i = ((y as usize) * (r.width as usize) + x as usize) * 4;
            let ba = r.data[i + 3] as f32 / 255.0;
            let out_a = ga + ba * (1.0 - ga);
            for (k, &ck) in col.iter().enumerate().take(3) {
                let v = (ck * ga + r.data[i + k] as f32 * ba * (1.0 - ga)) / out_a.max(1e-6);
                r.data[i + k] = v.round().clamp(0.0, 255.0) as u8;
            }
            r.data[i + 3] = (out_a * 255.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// Fill a closed polygon with a colour (pen path fill).
pub fn fill_polygon(r: &mut Raster, pts: &[(f32, f32)], color: [u8; 4], sel: Option<&Selection>) {
    let poly = Selection::from_polygon(r.width, r.height, pts);
    for y in 0..r.height {
        for x in 0..r.width {
            let f = (poly.at(x, y) as f32 / 255.0) * sel_factor(sel, x, y);
            if f > 0.0 {
                mix_px(
                    r,
                    x,
                    y,
                    [
                        color[0] as f32,
                        color[1] as f32,
                        color[2] as f32,
                        color[3] as f32,
                    ],
                    f,
                );
            }
        }
    }
}

/// Bounding rect of a dab, for partial texture updates.
pub fn dab_rect(cx: f32, cy: f32, size: f32, w: u32, h: u32) -> Option<PixelRect> {
    let rad = size / 2.0 + 2.0;
    PixelRect {
        x: (cx - rad).max(0.0) as u32,
        y: (cy - rad).max(0.0) as u32,
        w: (size + 4.0).ceil() as u32 + 1,
        h: (size + 4.0).ceil() as u32 + 1,
    }
    .clamped(w, h)
}

pub fn union_rect(a: Option<PixelRect>, b: Option<PixelRect>) -> Option<PixelRect> {
    match (a, b) {
        (None, x) | (x, None) => x,
        (Some(a), Some(b)) => {
            let x0 = a.x.min(b.x);
            let y0 = a.y.min(b.y);
            let x1 = (a.x + a.w).max(b.x + b.w);
            let y1 = (a.y + a.h).max(b.y + b.h);
            Some(PixelRect {
                x: x0,
                y: y0,
                w: x1 - x0,
                h: y1 - y0,
            })
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn trimming_a_layer_is_lossless() {
        let mut r = Raster::new(300, 200);
        for y in 40..90 {
            for x in 120..170 {
                let k = (y * 300 + x) * 4;
                r.data[k..k + 4].copy_from_slice(&[x as u8, y as u8, 0, (x + y) as u8]);
            }
        }
        r.data[(150 * 300 + 10) * 4 + 3] = 1; // a lone, nearly clear pixel
        let mut l = PaintLayer::new(1, "a", r.clone());
        l.offset = (5, -3);
        let mut full = l.clone();
        full.bake_to(300, 200);
        assert!(l.trim());
        assert!(l.raster.width < 300 && l.raster.height < 200);
        let mut back = l.clone();
        back.bake_to(300, 200);
        assert_eq!(back.raster.data, full.raster.data);
        let thumb = layer_thumbnail(&l, 300, 200, 44);
        assert_eq!(thumb.width, 44);
        // an empty layer trims to nothing and bakes back to clear pixels
        let mut e = PaintLayer::new(2, "e", Raster::new(64, 64));
        assert!(e.trim());
        assert_eq!((e.raster.width, e.raster.height), (0, 0));
        e.bake_to(64, 64);
        assert!(e.raster.data.iter().all(|&b| b == 0) && e.raster.data.len() == 64 * 64 * 4);
    }

    use super::*;

    fn solid(w: u32, h: u32, c: [u8; 4]) -> Raster {
        Raster::filled(w, h, c)
    }

    #[test]
    fn rect_and_polygon_selection() {
        let s = Selection::from_rect(
            10,
            10,
            PixelRect {
                x: 2,
                y: 2,
                w: 3,
                h: 3,
            },
        );
        assert_eq!(s.count(), 9);
        assert_eq!(
            s.bounds(),
            Some(PixelRect {
                x: 2,
                y: 2,
                w: 3,
                h: 3
            })
        );
        let tri = Selection::from_polygon(20, 20, &[(0.0, 0.0), (20.0, 0.0), (0.0, 20.0)]);
        let n = tri.count();
        assert!(n > 150 && n < 250, "triangle area ~200, got {}", n);
        assert_eq!(tri.at(1, 1), 255);
        assert_eq!(tri.at(19, 19), 0);
    }

    #[test]
    fn magic_wand_stops_at_edges() {
        let mut r = solid(10, 10, [0, 0, 0, 255]);
        for y in 0..10 {
            let i = ((y * 10 + 5) * 4) as usize;
            r.data[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
        let s = Selection::magic_wand(&r, (1, 1), 10);
        assert_eq!(s.count(), 50);
        assert_eq!(s.at(7, 1), 0);
    }

    #[test]
    fn invert_and_crop_selection() {
        let mut s = Selection::from_rect(
            4,
            4,
            PixelRect {
                x: 0,
                y: 0,
                w: 2,
                h: 4,
            },
        );
        s.invert();
        assert_eq!(s.at(0, 0), 0);
        assert_eq!(s.at(3, 0), 255);
        let c = s.cropped(PixelRect {
            x: 2,
            y: 0,
            w: 2,
            h: 2,
        });
        assert_eq!(c.count(), 4);
    }

    #[test]
    fn blur_keeps_flat_fields() {
        let d = vec![200u8; 64];
        assert_eq!(blur_mask(&d, 8, 8, 2), d);
        let mut e = vec![0u8; 64];
        e[27] = 255;
        let b = blur_mask(&e, 8, 8, 1);
        assert!(b[27] < 255 && b[26] > 0);
    }

    #[test]
    fn mask_density_and_feather() {
        let sel = Selection::from_rect(
            10,
            10,
            PixelRect {
                x: 0,
                y: 0,
                w: 5,
                h: 10,
            },
        );
        let mut m = LayerMask::from_selection(&sel);
        assert_eq!(m.factor(8, 5), 0.0);
        m.density = 0.5;
        assert!((m.factor(8, 5) - 0.5).abs() < 1e-6);
        m.density = 1.0;
        m.feather = 2.0;
        m.prepare();
        let edge = m.factor(5, 5);
        assert!(edge > 0.0 && edge < 1.0, "feathered edge {}", edge);
        m.invert = true;
        m.feather = 0.0;
        m.prepare();
        assert_eq!(m.factor(1, 1), 0.0);
    }

    #[test]
    fn composite_respects_visibility_groups_fill_masks_offsets() {
        let base = PaintLayer::new(1, "base", solid(4, 4, [0, 0, 0, 255]));
        let mut top = PaintLayer::new(2, "top", solid(4, 4, [255, 255, 255, 255]));
        let mut out = Raster::new(4, 4);
        composite_into(&mut out, &[base.clone(), top.clone()], &[], [9, 9, 9], None);
        assert_eq!(out.pixel(0, 0), Some([255, 255, 255, 255]));
        top.fill = 0.5;
        composite_into(&mut out, &[base.clone(), top.clone()], &[], [9, 9, 9], None);
        assert_eq!(out.pixel(0, 0).map(|p| p[0]), Some(128));
        top.fill = 1.0;
        top.group = Some(7);
        let g = LayerGroup {
            id: 7,
            name: "g".into(),
            visible: false,
            collapsed: false,
        };
        composite_into(
            &mut out,
            &[base.clone(), top.clone()],
            &[g],
            [9, 9, 9],
            None,
        );
        assert_eq!(out.pixel(0, 0).map(|p| p[0]), Some(0));
        top.group = None;
        top.mask = Some(LayerMask::from_selection(&Selection::from_rect(
            4,
            4,
            PixelRect {
                x: 0,
                y: 0,
                w: 2,
                h: 4,
            },
        )));
        composite_into(&mut out, &[base.clone(), top.clone()], &[], [9, 9, 9], None);
        assert_eq!(out.pixel(0, 0).map(|p| p[0]), Some(255));
        assert_eq!(out.pixel(3, 0).map(|p| p[0]), Some(0));
        top.mask = None;
        let mut half = Raster::new(4, 4);
        half.data[0..4].copy_from_slice(&[255, 0, 0, 255]);
        let mut moved = PaintLayer::new(3, "m", half);
        moved.offset = (2, 1);
        composite_into(
            &mut out,
            &[base.clone(), moved.clone()],
            &[],
            [9, 9, 9],
            None,
        );
        assert_eq!(out.pixel(2, 1), Some([255, 0, 0, 255]));
        moved.bake_offset();
        assert_eq!(moved.raster.pixel(2, 1), Some([255, 0, 0, 255]));
        assert_eq!(moved.offset, (0, 0));
    }

    #[test]
    fn region_composite_only_touches_region() {
        let l = PaintLayer::new(1, "w", solid(8, 8, [200, 0, 0, 255]));
        let mut out = Raster::filled(8, 8, [1, 2, 3, 255]);
        composite_into(
            &mut out,
            &[l],
            &[],
            [0, 0, 0],
            Some(PixelRect {
                x: 0,
                y: 0,
                w: 2,
                h: 2,
            }),
        );
        assert_eq!(out.pixel(0, 0), Some([200, 0, 0, 255]));
        assert_eq!(out.pixel(5, 5), Some([1, 2, 3, 255]));
    }

    #[test]
    fn brush_respects_selection_mask() {
        let mut r = Raster::new(20, 20);
        let sel = Selection::from_rect(
            20,
            20,
            PixelRect {
                x: 0,
                y: 0,
                w: 10,
                h: 20,
            },
        );
        let b = Brush {
            size: 30.0,
            hardness: 1.0,
            ..Brush::default()
        };
        paint_dab(&mut r, &b, 10.0, 10.0, false, Some(&sel));
        assert!(r.pixel(5, 10).map(|p| p[3]).unwrap_or(0) > 200);
        assert_eq!(r.pixel(15, 10).map(|p| p[3]), Some(0));
        paint_dab(&mut r, &b, 10.0, 10.0, true, None);
        assert_eq!(r.pixel(5, 10).map(|p| p[3]), Some(0));
        assert!(dab_points(&b, (0.0, 0.0), (30.0, 0.0)).len() >= 4);
    }

    #[test]
    fn dodge_lightens_burn_darkens() {
        let mut r = solid(10, 10, [100, 100, 100, 255]);
        let b = Brush {
            size: 10.0,
            hardness: 1.0,
            ..Brush::default()
        };
        tone_dab(&mut r, &b, 5.0, 5.0, 1.0, true, None);
        assert!(r.pixel(5, 5).map(|p| p[0]).unwrap_or(0) > 100);
        let mut r2 = solid(10, 10, [100, 100, 100, 255]);
        tone_dab(&mut r2, &b, 5.0, 5.0, 1.0, false, None);
        assert!(r2.pixel(5, 5).map(|p| p[0]).unwrap_or(255) < 100);
        assert_eq!(r2.pixel(5, 5).map(|p| p[3]), Some(255));
    }

    #[test]
    fn clone_copies_source_pixels() {
        let mut src = solid(20, 10, [0, 0, 0, 255]);
        for y in 0..10 {
            for x in 10..20 {
                let i = ((y * 20 + x) * 4) as usize;
                src.data[i] = 250;
            }
        }
        let mut r = src.clone();
        let b = Brush {
            size: 6.0,
            hardness: 1.0,
            flow: 1.0,
            ..Brush::default()
        };
        clone_dab(&mut r, &src, &b, 4.0, 5.0, (10.0, 0.0), None);
        assert_eq!(r.pixel(4, 5).map(|p| p[0]), Some(250));
    }

    #[test]
    fn heal_removes_a_spot() {
        let mut src = solid(60, 30, [120, 120, 120, 255]);
        let i = ((15 * 60 + 15) * 4) as usize;
        src.data[i..i + 3].copy_from_slice(&[255, 0, 0]);
        let mut r = src.clone();
        let b = Brush {
            size: 8.0,
            hardness: 1.0,
            ..Brush::default()
        };
        heal_dab(&mut r, &src, &b, 15.5, 15.5, None);
        let p = r.pixel(15, 15).unwrap_or([0; 4]);
        assert!(
            (p[0] as i32 - 120).abs() < 6 && (p[1] as i32 - 120).abs() < 6,
            "{:?}",
            p
        );
    }

    #[test]
    fn gradient_runs_from_c0_to_c1() {
        let mut r = Raster::new(100, 4);
        gradient_fill(
            &mut r,
            (0.0, 0.0),
            (100.0, 0.0),
            [255, 0, 0, 255],
            [0, 0, 255, 255],
            None,
        );
        let a = r.pixel(0, 0).unwrap_or([0; 4]);
        let b = r.pixel(99, 0).unwrap_or([0; 4]);
        assert!(a[0] > 240 && b[2] > 240);
        let mut t = Raster::new(10, 10);
        fill_polygon(
            &mut t,
            &[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)],
            [1, 2, 3, 255],
            None,
        );
        assert_eq!(t.pixel(5, 5), Some([1, 2, 3, 255]));
    }

    #[test]
    fn rect_helpers() {
        let a = dab_rect(5.0, 5.0, 4.0, 100, 100);
        let b = dab_rect(50.0, 50.0, 4.0, 100, 100);
        let u = union_rect(a, b).unwrap_or(PixelRect {
            x: 0,
            y: 0,
            w: 0,
            h: 0,
        });
        assert!(u.x <= 3 && u.x + u.w >= 52);
        assert_eq!(thumbnail(&Raster::new(400, 200), 40).width, 40);
    }

    #[test]
    fn spaced_dabs_ignore_pauses_and_keep_spacing() {
        let b = Brush {
            size: 20.0,
            spacing: 0.25,
            ..Brush::default()
        };
        assert!(spaced_dab_points(&b, (10.0, 10.0), (10.0, 10.0)).is_empty());
        assert!(spaced_dab_points(&b, (10.0, 10.0), (13.0, 10.0)).is_empty());
        let d = spaced_dab_points(&b, (0.0, 0.0), (17.5, 0.0));
        assert_eq!(d.len(), 3);
        assert!((d[2].0 - 15.0).abs() < 1e-4);
    }

    #[test]
    fn quick_select_follows_gradients_but_stops_at_edges() {
        let mut r = Raster::new(100, 10);
        for x in 0..100u32 {
            for y in 0..10u32 {
                let v = if x < 60 { x as u8 } else { 250 };
                let i = ((y * 100 + x) * 4) as usize;
                r.data[i..i + 4].copy_from_slice(&[v, v, v, 255]);
            }
        }
        let q = Selection::quick_select(&r, (0, 0), 32);
        assert_eq!(q.bounds().map(|b| b.w), Some(60));
        let m = Selection::magic_wand(&r, (0, 0), 32);
        assert!(m.count() < q.count());
    }
}

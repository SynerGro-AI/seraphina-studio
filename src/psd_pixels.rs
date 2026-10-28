//! Layered PSD with real pixel data (8-bit RGB, version 1).
//!
//! Reads and writes per-layer RGBA channels (raw or PackBits RLE), layer
//! names (Pascal + `luni`), opacity, visibility, blend-mode keys, groups
//! (`lsct` section dividers) and the merged composite. ZIP-compressed
//! channels, 16/32-bit depth, PSB, CMYK and layer masks are reported as
//! unsupported (masks are skipped) instead of being misread.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LayerKind {
    Pixels,
    /// Folder record (written *after* its children, bottom-first order).
    GroupFolder {
        open: bool,
    },
    /// Hidden divider that opens a group (written *before* its children).
    GroupEnd,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PsdLayerPixels {
    pub name: String,
    pub visible: bool,
    pub opacity: u8,
    pub blend: [u8; 4],
    pub top: i32,
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
    /// RGBA for the layer rect, row-major.
    pub rgba: Vec<u8>,
    pub kind: LayerKind,
}

impl PsdLayerPixels {
    pub fn width(&self) -> u32 {
        (self.right as i64 - self.left as i64).clamp(0, u32::MAX as i64) as u32
    }
    pub fn height(&self) -> u32 {
        (self.bottom as i64 - self.top as i64).clamp(0, u32::MAX as i64) as u32
    }

    /// A pixel layer from a full-canvas RGBA buffer; the stored rect is
    /// trimmed to the non-transparent bounds.
    pub fn from_canvas(name: &str, w: u32, h: u32, rgba: &[u8], offset: (i32, i32)) -> Self {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0u32, 0u32);
        for y in 0..h {
            for x in 0..w {
                if rgba[((y * w + x) * 4 + 3) as usize] > 0 {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x + 1);
                    y1 = y1.max(y + 1);
                }
            }
        }
        let mut l = PsdLayerPixels {
            name: name.to_string(),
            visible: true,
            opacity: 255,
            blend: *b"norm",
            top: 0,
            left: 0,
            bottom: 0,
            right: 0,
            rgba: Vec::new(),
            kind: LayerKind::Pixels,
        };
        if x0 == u32::MAX {
            return l;
        }
        let (rw, rh) = (x1 - x0, y1 - y0);
        let mut out = Vec::with_capacity((rw * rh * 4) as usize);
        for y in y0..y1 {
            let s = ((y * w + x0) * 4) as usize;
            out.extend_from_slice(&rgba[s..s + (rw * 4) as usize]);
        }
        l.left = x0 as i32 + offset.0;
        l.top = y0 as i32 + offset.1;
        l.right = l.left + rw as i32;
        l.bottom = l.top + rh as i32;
        l.rgba = out;
        l
    }

    /// Paste into a `w`×`h` RGBA canvas (clipped).
    pub fn to_canvas(&self, w: u32, h: u32) -> Vec<u8> {
        let mut out = vec![0u8; (w * h * 4) as usize];
        let lw = self.width() as i32;
        for ly in 0..self.height() as i32 {
            let y = self.top + ly;
            if y < 0 || y >= h as i32 {
                continue;
            }
            for lx in 0..lw {
                let x = self.left + lx;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                let s = ((ly * lw + lx) * 4) as usize;
                let d = ((y as u32 * w + x as u32) * 4) as usize;
                out[d..d + 4].copy_from_slice(&self.rgba[s..s + 4]);
            }
        }
        out
    }

    fn group(name: &str, kind: LayerKind, visible: bool) -> Self {
        PsdLayerPixels {
            name: name.to_string(),
            visible,
            opacity: 255,
            blend: *b"pass",
            top: 0,
            left: 0,
            bottom: 0,
            right: 0,
            rgba: Vec::new(),
            kind,
        }
    }
    pub fn group_end() -> Self {
        Self::group("</Layer group>", LayerKind::GroupEnd, true)
    }
    pub fn group_folder(name: &str, visible: bool, open: bool) -> Self {
        Self::group(name, LayerKind::GroupFolder { open }, visible)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PsdPixels {
    pub width: u32,
    pub height: u32,
    /// Bottom-first, as stored in the file.
    pub layers: Vec<PsdLayerPixels>,
    /// Merged composite, RGBA.
    pub merged: Option<Vec<u8>>,
}

// ---- PackBits -------------------------------------------------------------------

pub fn packbits_encode(src: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(src.len() / 2 + 2);
    let mut i = 0;
    while i < src.len() {
        let mut run = 1;
        while i + run < src.len() && run < 128 && src[i + run] == src[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((1i16 - run as i16) as i8 as u8);
            out.push(src[i]);
            i += run;
            continue;
        }
        let start = i;
        let mut lit = 0;
        while i < src.len() && lit < 128 {
            if i + 1 < src.len() && src[i + 1] == src[i] {
                break;
            }
            i += 1;
            lit += 1;
        }
        if lit == 0 {
            i += 1;
            lit = 1;
        }
        out.push((lit - 1) as u8);
        out.extend_from_slice(&src[start..start + lit]);
    }
    out
}

pub fn packbits_decode(src: &[u8], want: usize) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(want);
    let mut i = 0;
    while i < src.len() && out.len() < want {
        let n = src[i] as i8;
        i += 1;
        if n >= 0 {
            let k = n as usize + 1;
            let s = src.get(i..i + k).ok_or("truncated RLE literal")?;
            out.extend_from_slice(s);
            i += k;
        } else if n != -128 {
            let k = 1 - n as isize;
            let b = *src.get(i).ok_or("truncated RLE run")?;
            i += 1;
            out.extend(std::iter::repeat_n(b, k as usize));
        }
    }
    if out.len() < want {
        return Err(format!(
            "RLE row decodes to {} bytes, expected {}",
            out.len(),
            want
        ));
    }
    out.truncate(want);
    Ok(out)
}

// ---- writer ---------------------------------------------------------------------

fn be16(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_be_bytes());
}
fn be32(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_be_bytes());
}

/// One planar channel, RLE-compressed with its 2-byte compression tag.
fn rle_channel(plane: &[u8], w: usize, h: usize) -> Vec<u8> {
    let mut out = Vec::new();
    be16(&mut out, 1);
    let rows: Vec<Vec<u8>> = (0..h)
        .map(|y| packbits_encode(&plane[y * w..(y + 1) * w]))
        .collect();
    for r in &rows {
        be16(&mut out, r.len() as u16);
    }
    for r in rows {
        out.extend(r);
    }
    out
}

fn planes(rgba: &[u8], n: usize) -> [Vec<u8>; 4] {
    let mut p = [
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
        Vec::with_capacity(n),
    ];
    for px in rgba.as_chunks::<4>().0 {
        for c in 0..4 {
            p[c].push(px[c]);
        }
    }
    p
}

fn pascal_padded(name: &str) -> Vec<u8> {
    let mut bytes: Vec<u8> = name
        .chars()
        .map(|c| if c.is_ascii() { c as u8 } else { b'?' })
        .take(255)
        .collect();
    let mut out = vec![bytes.len() as u8];
    out.append(&mut bytes);
    while out.len() % 4 != 0 {
        out.push(0);
    }
    out
}

fn tagged(key: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"8BIM");
    out.extend_from_slice(key);
    let mut d = data.to_vec();
    while !d.len().is_multiple_of(4) {
        d.push(0);
    }
    be32(&mut out, d.len() as u32);
    out.extend(d);
    out
}

impl PsdPixels {
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        if self.width == 0 || self.height == 0 || self.width > 30000 || self.height > 30000 {
            return Err(format!(
                "unsupported canvas size {}×{}",
                self.width, self.height
            ));
        }
        let mut f = Vec::new();
        f.extend_from_slice(b"8BPS");
        be16(&mut f, 1);
        f.extend_from_slice(&[0; 6]);
        be16(&mut f, 3);
        be32(&mut f, self.height);
        be32(&mut f, self.width);
        be16(&mut f, 8);
        be16(&mut f, 3);
        be32(&mut f, 0); // colour mode data
        be32(&mut f, 0); // image resources

        let mut records = Vec::new();
        let mut chdata = Vec::new();
        for l in &self.layers {
            let (w, h) = (l.width() as usize, l.height() as usize);
            if l.rgba.len() != w * h * 4 {
                return Err(format!(
                    "layer '{}' has {} bytes for a {}×{} rect",
                    l.name,
                    l.rgba.len(),
                    w,
                    h
                ));
            }
            let chans: Vec<(i16, Vec<u8>)> = if w * h == 0 {
                [-1i16, 0, 1, 2]
                    .iter()
                    .map(|&id| (id, vec![0, 0]))
                    .collect()
            } else {
                let p = planes(&l.rgba, w * h);
                vec![
                    (-1, rle_channel(&p[3], w, h)),
                    (0, rle_channel(&p[0], w, h)),
                    (1, rle_channel(&p[1], w, h)),
                    (2, rle_channel(&p[2], w, h)),
                ]
            };
            for v in [l.top, l.left, l.bottom, l.right] {
                be32(&mut records, v as u32);
            }
            be16(&mut records, chans.len() as u16);
            for (id, d) in &chans {
                be16(&mut records, *id as u16);
                be32(&mut records, d.len() as u32);
            }
            records.extend_from_slice(b"8BIM");
            records.extend_from_slice(&l.blend);
            records.push(l.opacity);
            records.push(0);
            let mut flags = if l.visible { 0 } else { 0x02 };
            if l.kind != LayerKind::Pixels {
                flags |= 0x18; // bit 3 + bit 4: pixel data irrelevant
            }
            records.push(flags);
            records.push(0);
            let mut extra = Vec::new();
            be32(&mut extra, 0); // mask
            be32(&mut extra, 0); // blending ranges
            extra.extend(pascal_padded(&l.name));
            let units: Vec<u16> = l.name.encode_utf16().collect();
            let mut luni = Vec::new();
            be32(&mut luni, units.len() as u32);
            for u in units {
                be16(&mut luni, u);
            }
            extra.extend(tagged(b"luni", &luni));
            match l.kind {
                LayerKind::Pixels => {}
                LayerKind::GroupFolder { open } => {
                    let mut d = Vec::new();
                    be32(&mut d, if open { 1 } else { 2 });
                    d.extend_from_slice(b"8BIMpass");
                    extra.extend(tagged(b"lsct", &d));
                }
                LayerKind::GroupEnd => {
                    let mut d = Vec::new();
                    be32(&mut d, 3);
                    extra.extend(tagged(b"lsct", &d));
                }
            }
            be32(&mut records, extra.len() as u32);
            records.extend(extra);
            for (_, d) in chans {
                chdata.extend(d);
            }
        }
        let mut info = Vec::new();
        be16(&mut info, self.layers.len() as u16);
        info.extend(records);
        info.extend(chdata);
        while info.len() % 4 != 0 {
            info.push(0);
        }
        let mut lmi = Vec::new();
        be32(&mut lmi, info.len() as u32);
        lmi.extend(info);
        be32(&mut lmi, 0); // global mask
        be32(&mut f, lmi.len() as u32);
        f.extend(lmi);

        // merged image: RLE, 3 channels
        let (w, h) = (self.width as usize, self.height as usize);
        let merged = self.merged.clone().unwrap_or_else(|| vec![255; w * h * 4]);
        if merged.len() != w * h * 4 {
            return Err("merged image has the wrong size".into());
        }
        let p = planes(&merged, w * h);
        be16(&mut f, 1);
        let mut rows = Vec::new();
        for plane in p.iter().take(3) {
            for y in 0..h {
                rows.push(packbits_encode(&plane[y * w..(y + 1) * w]));
            }
        }
        for r in &rows {
            be16(&mut f, r.len() as u16);
        }
        for r in rows {
            f.extend(r);
        }
        Ok(f)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<(), String> {
        let b = self.to_bytes()?;
        std::fs::write(path, b).map_err(|e| e.to_string())
    }

    pub fn load(path: &std::path::Path) -> Result<Self, String> {
        let b = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::parse(&b)
    }
}

// ---- reader ---------------------------------------------------------------------

struct Rd<'a> {
    b: &'a [u8],
    p: usize,
}

impl<'a> Rd<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let s = self
            .b
            .get(self.p..self.p + n)
            .ok_or_else(|| format!("file truncated at byte {}", self.p))?;
        self.p += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        let s = self.take(2)?;
        Ok(u16::from_be_bytes([s[0], s[1]]))
    }
    fn u32(&mut self) -> Result<u32, String> {
        let s = self.take(4)?;
        Ok(u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
    }
    fn i32(&mut self) -> Result<i32, String> {
        Ok(self.u32()? as i32)
    }
}

fn read_channel(raw: &[u8], w: usize, h: usize) -> Result<Vec<u8>, String> {
    if raw.len() < 2 {
        return Err("channel shorter than its compression tag".into());
    }
    let comp = u16::from_be_bytes([raw[0], raw[1]]);
    let body = &raw[2..];
    match comp {
        0 => {
            let s = body.get(..w * h).ok_or("raw channel truncated")?;
            Ok(s.to_vec())
        }
        1 => {
            let counts_len = h * 2;
            let counts = body.get(..counts_len).ok_or("RLE row table truncated")?;
            let mut p = counts_len;
            let mut out = Vec::with_capacity(w * h);
            for y in 0..h {
                let n = u16::from_be_bytes([counts[y * 2], counts[y * 2 + 1]]) as usize;
                let row = body.get(p..p + n).ok_or("RLE row truncated")?;
                out.extend(packbits_decode(row, w)?);
                p += n;
            }
            Ok(out)
        }
        2 | 3 => Err("ZIP-compressed channels aren't supported yet".into()),
        c => Err(format!("unknown channel compression {}", c)),
    }
}

/// Largest side Photoshop allows in a PSD (PSB goes beyond; unsupported).
pub const MAX_SIDE: u32 = 30_000;
/// Largest pixel count we allocate for one image or layer (2²⁸ px = 1 GiB RGBA).
pub const MAX_PIXELS: u64 = 1 << 28;

/// Most pixels `len` bytes of channel data can decode to: PackBits needs
/// at least 2 bytes per 128-pixel run, raw needs 1 byte per pixel.
fn max_decodable(len: usize) -> u64 {
    (len as u64).saturating_mul(64)
}

fn check_dims(what: &str, w: u64, h: u64) -> Result<(), String> {
    if w > MAX_SIDE as u64 || h > MAX_SIDE as u64 {
        return Err(format!(
            "{} is {} × {} px; the PSD limit is {} px per side",
            what, w, h, MAX_SIDE
        ));
    }
    if w.saturating_mul(h) > MAX_PIXELS {
        return Err(format!(
            "{} is {} × {} px, more than the {} px this app will allocate",
            what, w, h, MAX_PIXELS
        ));
    }
    Ok(())
}

impl PsdPixels {
    pub fn parse(b: &[u8]) -> Result<Self, String> {
        let mut r = Rd { b, p: 0 };
        if r.take(4)? != b"8BPS" {
            return Err("not a PSD (missing 8BPS signature)".into());
        }
        match r.u16()? {
            1 => {}
            2 => return Err("PSB (large document) files aren't supported".into()),
            v => return Err(format!("unknown PSD version {}", v)),
        }
        r.take(6)?;
        let channels = r.u16()? as usize;
        let height = r.u32()?;
        let width = r.u32()?;
        let depth = r.u16()?;
        let mode = r.u16()?;
        if depth != 8 {
            return Err(format!("{}-bit PSDs aren't supported (8-bit only)", depth));
        }
        if mode != 3 {
            return Err(format!("colour mode {} isn't supported (RGB only)", mode));
        }
        if width == 0 || height == 0 {
            return Err("the document has zero width or height".into());
        }
        check_dims("the document", width as u64, height as u64)?;
        if channels == 0 || channels > 56 {
            return Err(format!("{} channels (a PSD has 1-56)", channels));
        }
        let cm = r.u32()? as usize;
        r.take(cm)?;
        let ir = r.u32()? as usize;
        r.take(ir)?;
        let lmi_len = r.u32()? as usize;
        let lmi_end = r.p.saturating_add(lmi_len);
        let mut layers = Vec::new();
        if lmi_len >= 4 {
            let li_len = r.u32()? as usize;
            let li_end = r.p.saturating_add(li_len);
            if li_len >= 2 {
                let count = (r.u16()? as i16).unsigned_abs() as usize;
                let mut recs = Vec::with_capacity(count);
                for _ in 0..count {
                    let (top, left, bottom, right) = (r.i32()?, r.i32()?, r.i32()?, r.i32()?);
                    let nch = r.u16()? as usize;
                    let mut chans = Vec::with_capacity(nch);
                    for _ in 0..nch {
                        let id = r.u16()? as i16;
                        let len = r.u32()? as usize;
                        chans.push((id, len));
                    }
                    if r.take(4)? != b"8BIM" {
                        return Err("bad blend-mode signature in a layer record".into());
                    }
                    let mut blend = [0u8; 4];
                    blend.copy_from_slice(r.take(4)?);
                    let opacity = r.u8()?;
                    let _clip = r.u8()?;
                    let flags = r.u8()?;
                    let _filler = r.u8()?;
                    let extra_len = r.u32()? as usize;
                    let extra_end = r.p.saturating_add(extra_len);
                    let mask_len = r.u32()? as usize;
                    r.take(mask_len)?;
                    let br_len = r.u32()? as usize;
                    r.take(br_len)?;
                    let nlen = r.u8()? as usize;
                    let name_bytes = r.take(nlen)?;
                    let mut name: String = name_bytes.iter().map(|&c| c as char).collect();
                    let consumed = 1 + nlen;
                    let pad = (4 - consumed % 4) % 4;
                    r.take(pad)?;
                    let mut kind = LayerKind::Pixels;
                    while r.p + 12 <= extra_end {
                        let sig = r.take(4)?;
                        if sig != b"8BIM" && sig != b"8B64" {
                            break;
                        }
                        let mut key = [0u8; 4];
                        key.copy_from_slice(r.take(4)?);
                        let len = r.u32()? as usize;
                        let data = r.take(len)?;
                        match &key {
                            b"luni" if data.len() >= 4 => {
                                let n = u32::from_be_bytes([data[0], data[1], data[2], data[3]])
                                    as usize;
                                let units: Vec<u16> = data[4..]
                                    .as_chunks::<2>()
                                    .0
                                    .iter()
                                    .take(n)
                                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                                    .collect();
                                if let Ok(s) = String::from_utf16(&units) {
                                    name = s.trim_end_matches('\0').to_string();
                                }
                            }
                            b"lsct" | b"lsdk" if data.len() >= 4 => {
                                kind = match u32::from_be_bytes([
                                    data[0], data[1], data[2], data[3],
                                ]) {
                                    1 => LayerKind::GroupFolder { open: true },
                                    2 => LayerKind::GroupFolder { open: false },
                                    3 => LayerKind::GroupEnd,
                                    _ => LayerKind::Pixels,
                                };
                            }
                            _ => {}
                        }
                    }
                    r.p = extra_end;
                    recs.push((
                        PsdLayerPixels {
                            name,
                            visible: flags & 0x02 == 0,
                            opacity,
                            blend,
                            top,
                            left,
                            bottom,
                            right,
                            rgba: Vec::new(),
                            kind,
                        },
                        chans,
                    ));
                }
                for (mut l, chans) in recs {
                    let (w, h) = (l.width() as usize, l.height() as usize);
                    check_dims(&format!("layer '{}'", l.name), w as u64, h as u64)?;
                    // allocate only what the file's channel data could fill
                    let data: usize = chans
                        .iter()
                        .map(|c| c.1)
                        .fold(0usize, |a, b| a.saturating_add(b));
                    if (w * h) as u64 > max_decodable(data) || data > r.b.len().saturating_sub(r.p)
                    {
                        return Err(format!(
                            "layer '{}' claims {} × {} px but carries only {} bytes of pixel data",
                            l.name, w, h, data
                        ));
                    }
                    let mut rgba = vec![255u8; w * h * 4];
                    for (id, len) in chans {
                        let raw = r.take(len)?;
                        let c = match id {
                            0 => 0,
                            1 => 1,
                            2 => 2,
                            -1 => 3,
                            _ => continue, // masks: skipped
                        };
                        if w * h == 0 {
                            continue;
                        }
                        let plane = read_channel(raw, w, h)
                            .map_err(|e| format!("layer '{}': {}", l.name, e))?;
                        for (i, v) in plane.into_iter().enumerate() {
                            rgba[i * 4 + c] = v;
                        }
                    }
                    l.rgba = rgba;
                    layers.push(l);
                }
            }
            r.p = li_end;
        }
        r.p = lmi_end;
        // merged composite
        let merged = (|| -> Result<Vec<u8>, String> {
            let comp = r.u16()?;
            let (w, h) = (width as usize, height as usize);
            if (w * h) as u64 > max_decodable(r.b.len().saturating_sub(r.p)) {
                return Err("merged image is truncated".into());
            }
            let mut rgba = vec![255u8; w * h * 4];
            let nch = channels.min(4);
            match comp {
                0 => {
                    for c in 0..nch {
                        let s = r.take(w * h)?;
                        for (i, v) in s.iter().enumerate() {
                            rgba[i * 4 + c] = *v;
                        }
                    }
                }
                1 => {
                    let mut counts = Vec::with_capacity(channels * h);
                    for _ in 0..channels * h {
                        counts.push(r.u16()? as usize);
                    }
                    for c in 0..channels {
                        for y in 0..h {
                            let row = packbits_decode(r.take(counts[c * h + y])?, w)?;
                            if c < 4 {
                                for (x, v) in row.into_iter().enumerate() {
                                    rgba[(y * w + x) * 4 + c] = v;
                                }
                            }
                        }
                    }
                }
                _ => return Err("merged image compression not supported".into()),
            }
            if channels < 4 {
                for px in rgba.as_chunks_mut::<4>().0 {
                    px[3] = 255;
                }
            }
            Ok(rgba)
        })()
        .ok();
        Ok(Self {
            width,
            height,
            layers,
            merged,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 26-byte header + empty sections claiming `w` × `h` px (the audit's
    /// probe: this used to allocate w·h·4 bytes before reading any pixels).
    fn crafted_header(w: u32, h: u32, channels: u16) -> Vec<u8> {
        let mut b = b"8BPS".to_vec();
        b.extend(1u16.to_be_bytes());
        b.extend([0u8; 6]);
        b.extend(channels.to_be_bytes());
        b.extend(h.to_be_bytes());
        b.extend(w.to_be_bytes());
        b.extend(8u16.to_be_bytes());
        b.extend(3u16.to_be_bytes());
        b.extend(0u32.to_be_bytes()); // colour mode data
        b.extend(0u32.to_be_bytes()); // image resources
        b.extend(0u32.to_be_bytes()); // layer and mask info
        b.extend(0u16.to_be_bytes()); // merged: raw
        b
    }

    #[test]
    fn crafted_psd_dimensions_are_rejected_without_allocating() {
        // 70 000 × 70 000 (19.6 GB of RGBA): over the side limit
        let e = PsdPixels::parse(&crafted_header(70_000, 70_000, 3)).unwrap_err();
        assert!(e.contains("limit"), "{}", e);
        // within the side limit but over the pixel budget
        let e = PsdPixels::parse(&crafted_header(30_000, 30_000, 3)).unwrap_err();
        assert!(e.contains("allocate"), "{}", e);
        // plausible size, but the file has no pixel data: no 400 MB allocation
        let e = PsdPixels::parse(&crafted_header(10_000, 10_000, 3)).map(|p| p.merged.is_none());
        assert_eq!(e, Ok(true));
        assert!(PsdPixels::parse(&crafted_header(0, 10, 3)).is_err());
        assert!(PsdPixels::parse(&crafted_header(10, 10, 0)).is_err());
    }

    #[test]
    fn crafted_layer_rect_is_rejected() {
        // one layer record whose rect spans i32::MIN..i32::MAX with 0 bytes of data
        let mut b = crafted_header(4, 4, 3);
        b.truncate(b.len() - 6); // drop the empty layer section + merged
        let mut rec = Vec::new();
        rec.extend(1u16.to_be_bytes()); // layer count
        for v in [i32::MIN, i32::MIN, i32::MAX, i32::MAX] {
            rec.extend(v.to_be_bytes());
        }
        rec.extend(1u16.to_be_bytes()); // one channel
        rec.extend(0u16.to_be_bytes());
        rec.extend(0u32.to_be_bytes());
        rec.extend(b"8BIMnorm");
        rec.extend([255u8, 0, 0, 0]);
        rec.extend(12u32.to_be_bytes()); // extra data
        rec.extend(0u32.to_be_bytes()); // mask
        rec.extend(0u32.to_be_bytes()); // blending ranges
        rec.extend([0u8, 0, 0, 0]); // empty Pascal name, padded to 4
        let li = rec.len() as u32;
        b.extend((li + 4).to_be_bytes());
        b.extend(li.to_be_bytes());
        b.extend(rec);
        let e = PsdPixels::parse(&b).unwrap_err();
        assert!(e.contains("limit") || e.contains("allocate"), "{}", e);
    }

    #[test]
    fn packbits_roundtrip() {
        let cases: Vec<Vec<u8>> = vec![
            vec![],
            vec![7],
            vec![1, 1, 1, 1],
            (0..300).map(|i| (i % 7) as u8).collect(),
            vec![0; 1000],
            (0..255u8).chain([9; 200]).chain(0..40).collect(),
        ];
        for c in cases {
            let e = packbits_encode(&c);
            assert_eq!(packbits_decode(&e, c.len()).unwrap(), c);
        }
        assert!(packbits_encode(&[0; 1000]).len() < 20);
    }

    fn sample() -> PsdPixels {
        let (w, h) = (40u32, 30u32);
        let mut bg = vec![0u8; (w * h * 4) as usize];
        for (i, px) in bg.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            px.copy_from_slice(&[(i % 251) as u8, 100, 200, 255]);
        }
        let mut dot = vec![0u8; (w * h * 4) as usize];
        for y in 10..15 {
            for x in 20..28 {
                let i = ((y * w + x) * 4) as usize;
                dot[i..i + 4].copy_from_slice(&[250, 10, 10, 200]);
            }
        }
        let mut a = PsdLayerPixels::from_canvas("Background", w, h, &bg, (0, 0));
        a.blend = *b"norm";
        let mut b = PsdLayerPixels::from_canvas("Dot · ünïcode", w, h, &dot, (0, 0));
        b.opacity = 128;
        b.blend = *b"mul ";
        b.visible = false;
        let empty =
            PsdLayerPixels::from_canvas("Empty", w, h, &vec![0; (w * h * 4) as usize], (0, 0));
        PsdPixels {
            width: w,
            height: h,
            layers: vec![
                a,
                PsdLayerPixels::group_end(),
                b,
                empty,
                PsdLayerPixels::group_folder("Group 1", true, true),
            ],
            merged: Some(bg),
        }
    }

    #[test]
    fn layers_roundtrip_with_pixels_and_properties() {
        let d = sample();
        let bytes = d.to_bytes().unwrap();
        let back = PsdPixels::parse(&bytes).unwrap();
        assert_eq!(back, d);
        let dot = &back.layers[2];
        assert_eq!(
            (dot.left, dot.top, dot.width(), dot.height()),
            (20, 10, 8, 5)
        );
        assert_eq!(&dot.rgba[0..4], &[250, 10, 10, 200]);
        assert!(!dot.visible && dot.opacity == 128 && &dot.blend == b"mul ");
        assert_eq!(dot.name, "Dot · ünïcode");
        assert_eq!(back.layers[4].kind, LayerKind::GroupFolder { open: true });
        let canvas = dot.to_canvas(40, 30);
        assert_eq!(canvas[((12 * 40 + 22) * 4) as usize], 250);
        assert_eq!(canvas[3], 0);
    }

    #[test]
    fn rejects_bad_input_without_panicking() {
        assert!(PsdPixels::parse(b"nope").is_err());
        let mut bytes = sample().to_bytes().unwrap();
        bytes.truncate(200);
        assert!(PsdPixels::parse(&bytes).is_err());
        let mut b16 = sample().to_bytes().unwrap();
        b16[22] = 0;
        b16[23] = 16;
        assert!(PsdPixels::parse(&b16).unwrap_err().contains("16-bit"));
        for n in [0, 10, 26, 40, 90, 150] {
            let mut t = sample().to_bytes().unwrap();
            t.truncate(n);
            let _ = PsdPixels::parse(&t);
        }
    }
}

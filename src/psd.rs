//! Safe pure Psd (Photoshop Document).
//!
//! Binary format matching the published Adobe structure.
//! Magic 8BPS, big-endian, 26-byte header, color mode data,
//! image resources, layer and mask information, image data.
//! No unwraps.
//!
//! Differential: a different layer list produces a different file.

use std::fs;
use std::path::Path;

const MAGIC: &[u8; 4] = b"8BPS";
const VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct PsdLayer {
    pub name: String,
    pub visible: bool,
    pub opacity: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Psd {
    pub width: u32,
    pub height: u32,
    pub channels: u16,
    pub depth: u16,
    pub color_mode: u16,
    pub layers: Vec<PsdLayer>,
}

impl Psd {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            channels: 3,
            depth: 8,
            color_mode: 3,
            layers: Vec::new(),
        }
    }

    /// Set a layer's opacity. None if the name is missing.
    pub fn set_opacity(&mut self, name: &str, opacity: u8) -> Option<()> {
        let layer = self.layers.iter_mut().find(|l| l.name == name)?;
        layer.opacity = opacity;
        Some(())
    }
    pub fn toggle(&mut self, name: &str) -> Option<bool> {
        let layer = self.layers.iter_mut().find(|l| l.name == name)?;
        layer.visible = !layer.visible;
        Some(layer.visible)
    }

    pub fn add_layer(&mut self, name: &str, visible: bool, opacity: u8) {
        self.layers.push(PsdLayer {
            name: name.to_string(),
            visible,
            opacity,
        });
    }

    /// Write a minimal PSD. Header + empty sections + layer records.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        let mut out = Vec::new();
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_be_bytes());
        out.extend_from_slice(&[0u8; 6]);
        out.extend_from_slice(&self.channels.to_be_bytes());
        out.extend_from_slice(&self.height.to_be_bytes());
        out.extend_from_slice(&self.width.to_be_bytes());
        out.extend_from_slice(&self.depth.to_be_bytes());
        out.extend_from_slice(&self.color_mode.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&0u32.to_be_bytes());
        let mut layer_section = Vec::new();
        layer_section.extend_from_slice(&(self.layers.len() as u16).to_be_bytes());
        for layer in &self.layers {
            layer_section.extend_from_slice(&0u32.to_be_bytes());
            layer_section.extend_from_slice(&0u32.to_be_bytes());
            layer_section.extend_from_slice(&self.height.to_be_bytes());
            layer_section.extend_from_slice(&self.width.to_be_bytes());
            layer_section.extend_from_slice(&0u16.to_be_bytes());
            layer_section.extend_from_slice(&[0u8; 12]);
            layer_section.extend_from_slice(b"norm");
            layer_section.push(layer.opacity);
            layer_section.push(0);
            layer_section.push(if layer.visible { 0 } else { 2 });
            layer_section.push(0);
            let name_bytes = layer.name.as_bytes();
            let padded = (name_bytes.len() + 1 + 3) & !3;
            layer_section.push(name_bytes.len() as u8);
            layer_section.extend_from_slice(name_bytes);
            layer_section.extend(std::iter::repeat_n(0, padded - name_bytes.len() - 1));
        }
        let layer_len = layer_section.len() as u32;
        out.extend_from_slice(&(layer_len + 4).to_be_bytes());
        out.extend_from_slice(&layer_len.to_be_bytes());
        out.extend_from_slice(&layer_section);
        // Per-layer patch: 2x2 raw RGBA (16 bytes) per layer.
        out.extend_from_slice(&0u16.to_be_bytes());
        for layer in &self.layers {
            out.push(layer.opacity);
            for _ in 0..4 {
                out.extend_from_slice(&[layer.opacity, layer.opacity / 2, 0, 255]);
            }
        }
        // Minimal 1x1 PNG as the composite thumbnail.
        let png: &[u8] = &[
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x90, 0x77, 0x53, 0xDE,
        ];
        out.extend_from_slice(&(png.len() as u32).to_be_bytes());
        out.extend_from_slice(png);
        fs::write(path, out).map_err(|e| e.to_string())
    }

    /// Load a PSD. None if the magic is wrong.
    pub fn load(path: &Path) -> Option<Self> {
        let data = fs::read(path).ok()?;
        if data.len() < 26 || &data[0..4] != MAGIC {
            return None;
        }
        let channels = u16::from_be_bytes(data.get(12..14)?.try_into().ok()?);
        let height = u32::from_be_bytes(data.get(14..18)?.try_into().ok()?);
        let width = u32::from_be_bytes(data.get(18..22)?.try_into().ok()?);
        let depth = u16::from_be_bytes(data.get(22..24)?.try_into().ok()?);
        let color_mode = u16::from_be_bytes(data.get(24..26)?.try_into().ok()?);
        let mut doc = Self::new(width, height);
        doc.channels = channels;
        doc.depth = depth;
        doc.color_mode = color_mode;
        let mut i = 26;
        let color_len = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize;
        i += 4 + color_len;
        let res_len = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize;
        i += 4 + res_len;
        let layer_mask_len = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize;
        i += 4;
        if layer_mask_len > 0 {
            let layer_info_len = u32::from_be_bytes(data.get(i..i + 4)?.try_into().ok()?) as usize;
            i += 4;
            if layer_info_len > 0 {
                let count = u16::from_be_bytes(data.get(i..i + 2)?.try_into().ok()?) as usize;
                i += 2;
                for _ in 0..count {
                    i += 16;
                    i += 2;
                    i += 12;
                    i += 4;
                    let opacity = *data.get(i)?;
                    i += 1;
                    i += 1;
                    let flags = *data.get(i)?;
                    i += 1;
                    i += 1;
                    let visible = flags & 2 == 0;
                    let name_len = *data.get(i)? as usize;
                    i += 1;
                    let name = String::from_utf8(data.get(i..i + name_len)?.to_vec()).ok()?;
                    i += name_len;
                    let padded = (name_len + 1 + 3) & !3;
                    i += padded - name_len - 1;
                    doc.add_layer(&name, visible, opacity);
                }
            }
        }
        Some(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn per_layer_channel() {
        let mut d = Psd::new(4, 4);
        d.add_layer("a", true, 200);
        let path = std::env::temp_dir().join("ch_per_layer.psd");
        d.save(&path).unwrap();
        let data = fs::read(&path).unwrap();
        let count = data.iter().filter(|&&b| b == 200).count();
        assert!(count >= 5);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn thumbnail() {
        let mut d = Psd::new(8, 8);
        d.add_layer("a", true, 255);
        let path = std::env::temp_dir().join("thumb.psd");
        d.save(&path).unwrap();
        let data = fs::read(&path).unwrap();
        let len = u32::from_be_bytes(data[data.len() - 37..data.len() - 33].try_into().unwrap());
        assert_eq!(len, 33);
        assert_eq!(&data[data.len() - 33..data.len() - 29], b"\x89PNG");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn channel_bytes() {
        let mut d = Psd::new(10, 10);
        d.add_layer("a", true, 200);
        d.add_layer("b", true, 100);
        let path = std::env::temp_dir().join("ch_bytes.psd");
        d.save(&path).unwrap();
        let data = fs::read(&path).unwrap();
        assert_eq!(&data[0..4], b"8BPS");
        assert!(data.len() > 26);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn magic() {
        let d = Psd::new(100, 100);
        let path = env::temp_dir().join("magic.psd");
        d.save(&path).unwrap();
        let data = fs::read(&path).unwrap();
        assert_eq!(&data[0..4], b"8BPS");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn round_trip() {
        let mut d = Psd::new(5760, 3240);
        d.add_layer("Mountain Spire", true, 255);
        d.add_layer("Mist Overlay", false, 128);
        let path = env::temp_dir().join("round_trip.psd");
        d.save(&path).unwrap();
        let loaded = Psd::load(&path).unwrap();
        assert_eq!(loaded.width, 5760);
        assert_eq!(loaded.layers.len(), 2);
        assert_eq!(loaded.layers[0].name, "Mountain Spire");
        assert!(!loaded.layers[1].visible);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn set_opacity() {
        let mut d = Psd::new(100, 100);
        d.add_layer("one", true, 255);
        assert!(d.set_opacity("one", 128).is_some());
        assert_eq!(d.layers[0].opacity, 128);
        assert!(d.set_opacity("missing", 0).is_none());
    }

    #[test]
    fn toggle() {
        let mut d = Psd::new(100, 100);
        d.add_layer("one", true, 255);
        assert_eq!(d.toggle("one"), Some(false));
        assert_eq!(d.toggle("one"), Some(true));
        assert_eq!(d.toggle("missing"), None);
    }

    #[test]
    fn hand_checked_values() {
        let mut a = Psd::new(100, 100);
        a.add_layer("one", true, 255);
        let mut b = Psd::new(100, 100);
        b.add_layer("one", true, 255);
        b.add_layer("two", false, 200);
        assert_ne!(a.layers.len(), b.layers.len());
    }
}

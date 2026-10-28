//! App settings that must be known before the window opens (M3): the
//! renderer choice (D2) and the UI scale (H3). Stored as `~/.seraphina/studio/settings.json`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RendererPref {
    /// wgpu (DirectX 12 / Vulkan) on Windows, OpenGL elsewhere; the other
    /// one is tried when the first can't start.
    #[default]
    Auto,
    Wgpu,
    Glow,
}

impl RendererPref {
    pub const ALL: [RendererPref; 3] = [Self::Auto, Self::Wgpu, Self::Glow];
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::Wgpu => "DirectX 12 / Vulkan (wgpu)",
            Self::Glow => "OpenGL (glow)",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "wgpu" | "dx12" | "vulkan" | "directx" => Some(Self::Wgpu),
            "glow" | "gl" | "opengl" => Some(Self::Glow),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub renderer: RendererPref,
    /// Set when Automatic had to fall back; Automatic then starts with it.
    pub auto_fallback: Option<RendererPref>,
    /// Whole-UI scale on top of the monitor's DPI scale (H3), 0.8..=2.0.
    pub ui_scale: f32,
    /// Most xAI spend allowed in any 24 hours, in USD; 0 = no cap (C3).
    pub spend_cap_usd: f64,
    /// B3: the user agreed to send chat to an off-device model.
    pub ai_consent: bool,
    /// B3: include app state (mode, layer and object names) in the prompt.
    pub share_app_state: bool,
    /// B3: include recent Seraphina.AGI memory notes in the prompt.
    pub share_memory: bool,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            renderer: RendererPref::Auto,
            auto_fallback: None,
            ui_scale: 1.0,
            spend_cap_usd: 0.0,
            ai_consent: false,
            share_app_state: true,
            share_memory: true,
        }
    }
}

pub const MIN_SCALE: f32 = 0.8;
pub const MAX_SCALE: f32 = 2.0;

pub fn path() -> PathBuf {
    crate::output::studio_dir().join("settings.json")
}

impl AppSettings {
    pub fn load() -> (Self, Option<String>) {
        crate::project::load_json(&path(), "settings file")
    }
    pub fn save(&self) -> Result<(), String> {
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::project::write_atomic(&path(), s.as_bytes()).map_err(|e| e.to_string())
    }

    /// `ui_scale` made safe: a hand-edited 0, NaN or 50 must not leave the
    /// window unusable.
    pub fn scale(&self) -> f32 {
        if self.ui_scale.is_finite() {
            self.ui_scale.clamp(MIN_SCALE, MAX_SCALE)
        } else {
            1.0
        }
    }

    /// Renderers to try, in order. `env` is `SERAPHINA_RENDERER`.
    pub fn renderer_order(&self, env: Option<&str>, windows: bool) -> Vec<RendererPref> {
        let platform = if windows {
            [RendererPref::Wgpu, RendererPref::Glow]
        } else {
            [RendererPref::Glow, RendererPref::Wgpu]
        };
        let first = env
            .and_then(RendererPref::parse)
            .filter(|r| *r != RendererPref::Auto)
            .or(match self.renderer {
                RendererPref::Auto => self.auto_fallback,
                r => Some(r),
            })
            .unwrap_or(platform[0]);
        let mut v = vec![first];
        v.extend(platform.iter().copied().filter(|r| *r != first));
        v
    }
}

pub fn to_eframe(r: RendererPref) -> eframe::Renderer {
    match r {
        RendererPref::Glow => eframe::Renderer::Glow,
        _ => eframe::Renderer::Wgpu,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use RendererPref::*;

    #[test]
    fn renderer_order_has_a_fallback() {
        let s = AppSettings::default();
        assert_eq!(s.renderer_order(None, true), vec![Wgpu, Glow]);
        assert_eq!(s.renderer_order(None, false), vec![Glow, Wgpu]);
        assert_eq!(s.renderer_order(Some("opengl"), true), vec![Glow, Wgpu]);
        assert_eq!(s.renderer_order(Some("bogus"), true), vec![Wgpu, Glow]);
        let fb = AppSettings {
            auto_fallback: Some(Glow),
            ..Default::default()
        };
        assert_eq!(fb.renderer_order(None, true), vec![Glow, Wgpu]);
        let forced = AppSettings {
            renderer: Wgpu,
            auto_fallback: Some(Glow),
            ..Default::default()
        };
        assert_eq!(forced.renderer_order(None, false), vec![Wgpu, Glow]);
    }

    #[test]
    fn old_or_partial_files_load() {
        let s: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(s, AppSettings::default());
        let s: AppSettings = serde_json::from_str(r#"{"renderer":"glow"}"#).unwrap();
        assert_eq!(s.renderer, Glow);
        assert_eq!(s.scale(), 1.0);
    }

    #[test]
    fn ui_scale_is_clamped() {
        for (v, want) in [(1.5, 1.5), (0.0, 0.8), (9.0, 2.0), (f32::NAN, 1.0)] {
            let s = AppSettings {
                ui_scale: v,
                ..Default::default()
            };
            assert_eq!(s.scale(), want, "{v}");
        }
    }
}

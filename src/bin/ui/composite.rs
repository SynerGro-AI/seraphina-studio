//! Composite mode: layers with keyframed opacity over time.

use crate::commands::AppCommand;
use crate::resolve_asset;
use crate::theme::{self, Palette, R_MD, R_SM, SP_SM, SP_XS};
use crate::timeline::{self, Clock, Track};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Stroke, Vec2};
use egui_phosphor::regular as ic;
use obs_rust_bitstream::ui_canvas::{
    composite, format_clock, BlendMode, ImageLayer, KeyTrack, Raster, ViewTransform,
};

pub const PW: u32 = 584;
pub const PH: u32 = 392;

#[derive(Clone)]
pub struct CompLayer {
    pub layer: ImageLayer,
    pub opacity: KeyTrack,
}

/// What the cached preview was rendered from: time, opacities, visibility, blends.
type PreviewKey = (f32, Vec<f32>, Vec<bool>, Vec<BlendMode>);

pub struct CompState {
    pub layers: Vec<CompLayer>,
    pub selected: usize,
    pub clock: Clock,
    tex: Option<egui::TextureHandle>,
    last: Option<PreviewKey>,
}

fn load_preview(path: &str) -> Option<Raster> {
    let img = image::open(resolve_asset(path)).ok()?;
    let img = img
        .resize_exact(PW, PH, image::imageops::FilterType::Triangle)
        .to_rgba8();
    Raster::from_rgba(PW, PH, img.into_raw())
}

impl CompState {
    pub fn new() -> Self {
        let mut layers = Vec::new();
        let base = load_preview("assets/mountain_spire.jpg")
            .unwrap_or_else(|| Raster::filled(PW, PH, [40, 60, 110, 255]));
        layers.push(CompLayer {
            layer: ImageLayer::new("Mountain Spire", base),
            opacity: KeyTrack::new(),
        });
        if let Some(r) = load_preview("assets/mist_overlay.jpg") {
            let mut l = ImageLayer::new("Mist Overlay", r);
            l.blend = BlendMode::Screen;
            let mut k = KeyTrack::new();
            k.set(0.0, 0.0);
            k.set(2.5, 0.85);
            k.set(5.0, 0.15);
            layers.push(CompLayer {
                layer: l,
                opacity: k,
            });
        }
        let mut grade = ImageLayer::new("Gold Grade", Raster::filled(PW, PH, [214, 180, 92, 255]));
        grade.blend = BlendMode::Multiply;
        let mut k = KeyTrack::new();
        k.set(0.0, 0.0);
        k.set(4.0, 0.45);
        layers.push(CompLayer {
            layer: grade,
            opacity: k,
        });
        Self {
            selected: layers.len() - 1,
            layers,
            clock: Clock::new(5.0),
            tex: None,
            last: None,
        }
    }

    pub fn opacity_at(&self, i: usize, t: f32) -> f32 {
        let l = &self.layers[i];
        l.opacity
            .sample(t)
            .unwrap_or(l.layer.opacity)
            .clamp(0.0, 1.0)
    }

    pub fn frame(&self, t: f32) -> Raster {
        let ls: Vec<ImageLayer> = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let mut x = l.layer.clone();
                x.opacity = self.opacity_at(i, t);
                x
            })
            .collect();
        composite(&ls, PW, PH, [0, 0, 0])
    }

    /// Layer by name or 1-based index from the top; `None` = selected.
    pub fn find(&self, key: Option<&str>) -> Result<usize, String> {
        let n = self.layers.len();
        let Some(k) = key.map(|k| k.trim()).filter(|k| !k.is_empty()) else {
            return Ok(self.selected.min(n.saturating_sub(1)));
        };
        if let Ok(i) = k.parse::<usize>() {
            return if i >= 1 && i <= n {
                Ok(n - i)
            } else {
                Err(format!("layer index {} out of range 1-{}", i, n))
            };
        }
        let lk = k.to_lowercase();
        self.layers
            .iter()
            .position(|l| l.layer.name.to_lowercase() == lk)
            .or_else(|| {
                self.layers
                    .iter()
                    .position(|l| l.layer.name.to_lowercase().contains(&lk))
            })
            .ok_or_else(|| {
                format!(
                    "no composite layer named '{}' (layers: {})",
                    k,
                    self.layers
                        .iter()
                        .rev()
                        .map(|l| l.layer.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })
    }

    /// Execute a Composite command. `None` = not handled here.
    pub fn apply(&mut self, cmd: &AppCommand) -> Option<Result<String, String>> {
        use AppCommand as C;
        let t = self.clock.time;
        Some(match cmd {
            C::SelectLayer { layer } => self.find(Some(layer)).map(|i| {
                self.selected = i;
                format!("selected {}", self.layers[i].layer.name)
            }),
            C::SetLayerOpacity { layer, opacity } => self.find(layer.as_deref()).map(|i| {
                let v = (opacity / 100.0).clamp(0.0, 1.0);
                let l = &mut self.layers[i];
                if l.opacity.is_empty() {
                    l.layer.opacity = v;
                    format!("set {} opacity {:.0}%", l.layer.name, v * 100.0)
                } else {
                    l.opacity.set(t, v);
                    format!(
                        "keyed {} opacity {:.0}% at {:.2}s",
                        l.layer.name,
                        v * 100.0,
                        t
                    )
                }
            }),
            C::SetLayerBlend { layer, mode } => self.find(layer.as_deref()).and_then(|i| {
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
                self.layers[i].layer.blend = m;
                Ok(format!(
                    "set {} blend to {}",
                    self.layers[i].layer.name,
                    m.label()
                ))
            }),
            C::SetLayerVisibility { layer, visible } => self.find(layer.as_deref()).map(|i| {
                self.layers[i].layer.visible = *visible;
                format!(
                    "{} {}",
                    if *visible { "showed" } else { "hid" },
                    self.layers[i].layer.name
                )
            }),
            C::SetKeyframe => self.find(None).map(|i| {
                let v = self.opacity_at(i, t);
                self.layers[i].opacity.set(t, v);
                format!(
                    "keyed {} opacity {:.0}% at {:.2}s",
                    self.layers[i].layer.name,
                    v * 100.0,
                    t
                )
            }),
            C::DeleteKeyframe => self.find(None).and_then(|i| {
                if self.layers[i].opacity.remove_near(t, 0.08) {
                    Ok(format!(
                        "deleted {} key at {:.2}s",
                        self.layers[i].layer.name, t
                    ))
                } else {
                    Err(format!(
                        "no key on {} near {:.2}s",
                        self.layers[i].layer.name, t
                    ))
                }
            }),
            C::TimelinePlay { .. } | C::TimelineScrub { .. } => {
                return timeline::apply(&mut self.clock, cmd)
            }
            _ => return None,
        })
    }

    pub fn context(&self) -> String {
        let t = self.clock.time;
        let ls: Vec<String> = (0..self.layers.len())
            .rev()
            .map(|i| {
                let l = &self.layers[i];
                format!(
                    "{}{} ({}, opacity {:.0}% now, {} keys{})",
                    if i == self.selected { "*" } else { "" },
                    l.layer.name,
                    l.layer.blend.label(),
                    self.opacity_at(i, t) * 100.0,
                    l.opacity.keys().len(),
                    if l.layer.visible { "" } else { ", hidden" }
                )
            })
            .collect();
        format!("Composite: playhead {:.2}s of {:.1}s, playing {}, layers top→bottom (* = selected): {}", t, self.clock.duration, self.clock.playing, ls.join("; "))
    }

    fn refresh(&mut self, ctx: &egui::Context) {
        let t = self.clock.time;
        let ops: Vec<f32> = (0..self.layers.len())
            .map(|i| self.opacity_at(i, t))
            .collect();
        let vis: Vec<bool> = self.layers.iter().map(|l| l.layer.visible).collect();
        let blends: Vec<BlendMode> = self.layers.iter().map(|l| l.layer.blend).collect();
        let key = (0.0, ops, vis, blends);
        if self.tex.is_some() && self.last.as_ref() == Some(&key) {
            return;
        }
        let r = self.frame(t);
        let img = egui::ColorImage::from_rgba_unmultiplied([PW as usize, PH as usize], &r.data);
        match &mut self.tex {
            Some(tx) => tx.set(img, egui::TextureOptions::LINEAR),
            None => self.tex = Some(ctx.load_texture("comp", img, egui::TextureOptions::LINEAR)),
        }
        self.last = Some(key);
    }

    pub fn tools(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        theme::card(ui, p, "Layers", |ui| {
            for i in (0..self.layers.len()).rev() {
                let on = i == self.selected;
                let l = &self.layers[i];
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
                            if theme::icon_button(
                                ui,
                                p,
                                if l.layer.visible {
                                    ic::EYE
                                } else {
                                    ic::EYE_SLASH
                                },
                                "Show / hide",
                                false,
                            )
                            .clicked()
                            {
                                bus.push(AppCommand::SetLayerVisibility {
                                    layer: Some(l.layer.name.clone()),
                                    visible: !l.layer.visible,
                                });
                            }
                            if ui
                                .add(
                                    egui::Button::new(
                                        egui::RichText::new(&l.layer.name).color(if on {
                                            p.accent
                                        } else {
                                            p.text
                                        }),
                                    )
                                    .frame(false),
                                )
                                .clicked()
                            {
                                bus.push(AppCommand::SelectLayer {
                                    layer: l.layer.name.clone(),
                                });
                            }
                        });
                    });
            }
        });
        theme::card(ui, p, "", |ui| {
            ui.label(theme::muted(p, "Opacity is keyframed per layer and interpolated linearly. Scrub the timeline to preview."));
        });
    }

    pub fn center(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        self.refresh(&ctx);
        let (resp, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::hover());
        let rect = resp.rect;
        painter.rect_filled(rect, CornerRadius::ZERO, p.canvas);
        let zoom = ViewTransform::fit_zoom(rect.width(), rect.height() - 24.0, (PW, PH), 32.0);
        let view = ViewTransform {
            viewport: (rect.min.x, rect.min.y, rect.width(), rect.height()),
            image: (PW, PH),
            zoom,
            pan: (0.0, 0.0),
        };
        let (x, y, w, h) = view.image_rect();
        let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
        painter.rect_filled(
            r.translate(Vec2::new(0.0, 6.0)),
            CornerRadius::same(R_MD as u8),
            Color32::from_black_alpha(if p.dark { 120 } else { 40 }),
        );
        if let Some(t) = &self.tex {
            let shape =
                egui::epaint::RectShape::filled(r, CornerRadius::same(R_MD as u8), Color32::WHITE)
                    .with_texture(t.id(), Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)));
            painter.add(shape);
        }
        painter.rect_stroke(
            r,
            CornerRadius::same(R_MD as u8),
            Stroke::new(1.0_f32, p.border),
            egui::StrokeKind::Middle,
        );
        let tc = format!(
            "PROGRAM  ·  {}:{:02}",
            format_clock(self.clock.time as u64),
            ((self.clock.time.fract()) * 30.0) as u32
        );
        painter.text(
            r.left_top() + Vec2::new(0.0, -SP_SM),
            egui::Align2::LEFT_BOTTOM,
            tc,
            egui::FontId::monospace(11.5),
            p.muted,
        );
    }

    pub fn inspector(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        let t = self.clock.time;
        let i = self.selected.min(self.layers.len() - 1);
        let cur = self.opacity_at(i, t);
        theme::card(ui, p, "Layer", |ui| {
            let l = &self.layers[i];
            let name = l.layer.name.clone();
            ui.label(egui::RichText::new(&name).strong().color(p.text));
            let mut pct = cur * 100.0;
            let resp = ui.add(
                egui::Slider::new(&mut pct, 0.0..=100.0)
                    .suffix("%")
                    .text("Opacity"),
            );
            if resp.changed() {
                bus.push(AppCommand::SetLayerOpacity {
                    layer: Some(name.clone()),
                    opacity: pct,
                });
            }
            let mut blend = l.layer.blend;
            egui::ComboBox::from_label("Blend")
                .selected_text(blend.label())
                .show_ui(ui, |ui| {
                    for m in BlendMode::ALL {
                        if ui.selectable_value(&mut blend, m, m.label()).clicked() {
                            bus.push(AppCommand::SetLayerBlend {
                                layer: Some(name.clone()),
                                mode: m.label().into(),
                            });
                        }
                    }
                });
            if !l.opacity.is_empty() {
                ui.label(theme::muted(
                    p,
                    "Animated: editing opacity sets a key at the playhead.",
                ));
            }
        });
        theme::card(ui, p, "Keyframes", |ui| {
            let l = &self.layers[i];
            ui.horizontal(|ui| {
                if ui
                    .add(theme::primary_button(
                        p,
                        &format!("{} Add key", ic::DIAMOND),
                    ))
                    .on_hover_text("Key opacity at the playhead  (K)")
                    .clicked()
                {
                    bus.push(AppCommand::SetKeyframe);
                }
                let near = l.opacity.has_key_near(t, 0.08);
                if ui
                    .add_enabled(near, theme::toggle_button(p, "Delete key", false))
                    .clicked()
                {
                    bus.push(AppCommand::DeleteKeyframe);
                }
            });
            if l.opacity.is_empty() {
                ui.label(theme::muted(p, "No keys — opacity is constant."));
            }
            let mut jump = None;
            for (kt, kv) in l.opacity.keys() {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new(
                                egui::RichText::new(format!("{}  {:.2}s", ic::DIAMOND, kt))
                                    .monospace()
                                    .color(p.text),
                            )
                            .frame(false),
                        )
                        .on_hover_text("Jump to key")
                        .clicked()
                    {
                        jump = Some(*kt);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("{:.0}%", kv * 100.0))
                                .monospace()
                                .color(p.muted),
                        );
                    });
                });
            }
            if let Some(kt) = jump {
                bus.push(AppCommand::TimelinePlay {
                    playing: Some(false),
                });
                bus.push(AppCommand::TimelineScrub { time: kt });
            }
        });
    }

    pub fn timeline(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        ui.horizontal(|ui| timeline::transport(ui, p, &mut self.clock, bus));
        ui.add_space(SP_XS);
        let tracks: Vec<Track> = self
            .layers
            .iter()
            .enumerate()
            .rev()
            .map(|(i, l)| Track {
                label: &l.layer.name,
                keys: &l.opacity,
                selected: i == self.selected,
            })
            .collect();
        let n = self.layers.len();
        let out = timeline::ruler(ui, p, "comp-ruler", &mut self.clock, &tracks);
        if let Some(k) = out.picked_track {
            self.selected = n - 1 - k;
        }
    }
}

// ---- project files ------------------------------------------------------------

use crate::project::{Assets, RasterRef};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompLayerFile {
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub raster: RasterRef,
    pub track: KeyTrack,
}

/// Composite timeline in a `.seraphina` project.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CompFile {
    pub layers: Vec<CompLayerFile>,
    pub selected: usize,
    pub clock: Clock,
}

impl CompState {
    pub fn to_file(&self, a: &mut Assets) -> CompFile {
        CompFile {
            layers: self
                .layers
                .iter()
                .map(|l| CompLayerFile {
                    name: l.layer.name.clone(),
                    visible: l.layer.visible,
                    opacity: l.layer.opacity,
                    blend: l.layer.blend,
                    raster: a.put_raster(&format!("comp {}", l.layer.name), &l.layer.raster),
                    track: l.opacity.clone(),
                })
                .collect(),
            selected: self.selected,
            clock: self.clock.clone(),
        }
    }

    pub fn load_file(&mut self, f: CompFile, a: &Assets) -> Result<(), String> {
        let mut layers = Vec::with_capacity(f.layers.len());
        for l in f.layers {
            let r = a.raster(&l.raster)?;
            if (r.width, r.height) != (PW, PH) {
                return Err(format!(
                    "composite layer is {}×{}, expected {}×{}",
                    r.width, r.height, PW, PH
                ));
            }
            let mut il = ImageLayer::new(&l.name, r);
            il.visible = l.visible;
            il.opacity = l.opacity;
            il.blend = l.blend;
            layers.push(CompLayer {
                layer: il,
                opacity: l.track,
            });
        }
        self.selected = f.selected.min(layers.len().saturating_sub(1));
        self.layers = layers;
        self.clock = f.clock;
        self.tex = None;
        self.last = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn two_layers() -> CompState {
        let base = ImageLayer::new("Base", Raster::filled(PW, PH, [200, 100, 40, 255]));
        let mut grade = ImageLayer::new("Grade", Raster::filled(PW, PH, [128, 255, 0, 255]));
        grade.blend = BlendMode::Multiply;
        let mut k = KeyTrack::new();
        k.set(0.0, 0.0);
        k.set(2.0, 1.0);
        CompState {
            layers: vec![
                CompLayer {
                    layer: base,
                    opacity: KeyTrack::new(),
                },
                CompLayer {
                    layer: grade,
                    opacity: k,
                },
            ],
            selected: 1,
            clock: Clock::new(4.0),
            tex: None,
            last: None,
        }
    }

    fn px(s: &CompState, t: f32) -> [u8; 4] {
        let f = s.frame(t);
        let p = f.data.as_chunks::<4>().0;
        assert!(
            p.iter().all(|q| *q == p[0]),
            "flat layers give a flat frame"
        );
        p[0]
    }

    // multiply: 200·128/255 = 100.4, 100·255/255 = 100, 40·0 = 0
    #[test]
    fn frames_blend_keyed_opacity_to_hand_computed_pixels() {
        let mut s = two_layers();
        assert_eq!(px(&s, 0.0), [200, 100, 40, 255]);
        // halfway between the keys: opacity 0.5
        assert_eq!(px(&s, 1.0), [150, 100, 20, 255]);
        assert_eq!(px(&s, 2.0), [100, 100, 0, 255]);
        assert_eq!(px(&s, 3.5), [100, 100, 0, 255], "holds after the last key");
        // opacity on a keyed layer keys it at the playhead (25% at 1 s)
        s.clock.time = 1.0;
        let r = s.apply(&AppCommand::SetLayerOpacity {
            layer: Some("Grade".into()),
            opacity: 25.0,
        });
        assert!(matches!(r, Some(Ok(_))), "{:?}", r);
        assert_eq!(px(&s, 1.0), [175, 100, 30, 255]);
        assert_eq!(px(&s, 2.0), [100, 100, 0, 255], "other keys untouched");
        // hidden layers do not render
        s.apply(&AppCommand::SetLayerVisibility {
            layer: Some("Grade".into()),
            visible: false,
        });
        assert_eq!(px(&s, 2.0), [200, 100, 40, 255]);
    }
}

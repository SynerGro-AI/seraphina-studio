//! Animate mode: frame-by-frame drawing with onion skin.

use crate::commands::AppCommand;
use crate::theme::{self, Palette, R_SM, SP_MD, SP_SM, SP_XS};
use crate::timeline::Clock;
use crate::{ToastKind, Toasts};
use eframe::egui::{self, Color32, CornerRadius, Pos2, Rect, Stroke, Vec2};
use obs_rust_bitstream::tile_undo::{budget_from_env, DeltaHistory};
use obs_rust_bitstream::ui_canvas::{Brush, Raster, ViewTransform};

pub const W: u32 = 640;
pub const H: u32 = 360;
const MAX_FRAMES: usize = 48;
/// Undo steps (was 6 whole copies; now tile deltas under the byte budget).
pub const UNDO_STEPS: usize = 100;

pub struct AnimState {
    pub frames: Vec<Raster>,
    ink: Vec<bool>,
    texs: Vec<Option<egui::TextureHandle>>,
    dirty: Vec<bool>,
    pub fps: f32,
    pub clock: Clock,
    pub brush: Brush,
    pub erase: bool,
    pub onion: bool,
    pub onion_range: usize,
    history: DeltaHistory<Vec<Raster>>,
    last_dab: Option<(f32, f32)>,
    /// Bumped by every frame edit, undo and redo (dirty tracking).
    pub edits: u64,
}

impl AnimState {
    pub fn new() -> Self {
        let n = 12;
        let fps = 12.0;
        Self {
            frames: vec![Raster::new(W, H); n],
            ink: vec![false; n],
            texs: vec![None; n],
            dirty: vec![true; n],
            fps,
            clock: Clock::new(n as f32 / fps),
            brush: Brush {
                size: 6.0,
                hardness: 0.9,
                flow: 1.0,
                spacing: 0.2,
                color: [28, 28, 30],
            },
            erase: false,
            onion: true,
            onion_range: 1,
            history: DeltaHistory::new(UNDO_STEPS, budget_from_env()),
            last_dab: None,
            edits: 0,
        }
    }

    pub fn current(&self) -> usize {
        ((self.clock.time * self.fps).floor() as usize).min(self.frames.len().saturating_sub(1))
    }

    pub fn goto(&mut self, i: usize) {
        let i = i.min(self.frames.len().saturating_sub(1));
        self.clock.time = (i as f32 + 0.01) / self.fps;
        self.clock.playing = false;
    }

    fn resize_clock(&mut self) {
        self.clock.duration = (self.frames.len() as f32 / self.fps).max(0.1);
        if self.clock.time > self.clock.duration {
            self.clock.time = 0.0;
        }
    }

    pub fn set_fps(&mut self, fps: f32) {
        let cur = self.current();
        self.fps = fps.clamp(1.0, 60.0);
        self.resize_clock();
        self.goto(cur);
    }

    fn snapshot(&mut self) {
        self.edits += 1;
        self.history.push(&self.frames);
    }

    pub fn restore(&mut self, frames: Vec<Raster>) {
        let n = frames.len();
        self.edits += 1;
        self.frames = frames;
        self.ink = self
            .frames
            .iter()
            .map(|f| f.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0))
            .collect();
        self.texs = vec![None; n];
        self.dirty = vec![true; n];
        self.resize_clock();
    }

    /// Execute an Animate command. `None` = not handled here.
    pub fn apply(
        &mut self,
        cmd: &AppCommand,
        toasts: &mut Toasts,
    ) -> Option<Result<String, String>> {
        match cmd {
            AppCommand::SetKeyframe => {
                if self.frames.len() >= MAX_FRAMES {
                    return Some(Err(format!("frame limit reached ({})", MAX_FRAMES)));
                }
                self.duplicate(toasts);
                Some(Ok(format!(
                    "new key drawing at frame {} of {}",
                    self.current() + 1,
                    self.frames.len()
                )))
            }
            AppCommand::DeleteKeyframe => {
                if self.frames.len() <= 1 {
                    return Some(Err("an animation needs at least one frame".into()));
                }
                let cur = self.current();
                self.delete(toasts);
                Some(Ok(format!(
                    "deleted frame {}, {} left",
                    cur + 1,
                    self.frames.len()
                )))
            }
            AppCommand::TimelineScrub { time } => {
                let r = crate::timeline::apply(&mut self.clock, cmd);
                if time.is_finite() {
                    return Some(Ok(format!(
                        "frame {} of {}",
                        self.current() + 1,
                        self.frames.len()
                    )));
                }
                r
            }
            AppCommand::TimelinePlay { .. } => crate::timeline::apply(&mut self.clock, cmd),
            _ => None,
        }
    }

    pub fn context(&self) -> String {
        format!(
            "Animate: {} frames at {} fps, current frame {}, onion skin {}, drawn frames {}",
            self.frames.len(),
            self.fps,
            self.current() + 1,
            self.onion,
            self.ink.iter().filter(|x| **x).count()
        )
    }

    /// False when there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.history.undo(self.frames.clone()) else {
            return false;
        };
        self.restore(prev);
        true
    }

    /// False when there is nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.history.redo(self.frames.clone()) else {
            return false;
        };
        self.restore(next);
        true
    }

    pub fn can_undo(&self) -> bool {
        self.history.can_undo()
    }

    pub fn insert(&mut self, at: usize, r: Raster, toasts: &mut Toasts) {
        if self.frames.len() >= MAX_FRAMES {
            toasts.push(
                ToastKind::Error,
                &format!("Frame limit reached ({}).", MAX_FRAMES),
            );
            return;
        }
        self.snapshot();
        let ink = r.data.as_chunks::<4>().0.iter().any(|p| p[3] > 0);
        self.frames.insert(at, r);
        self.ink.insert(at, ink);
        self.texs.insert(at, None);
        self.dirty.insert(at, true);
        self.resize_clock();
        self.goto(at);
    }

    pub fn add_frame(&mut self, toasts: &mut Toasts) {
        let at = self.current() + 1;
        self.insert(at, Raster::new(W, H), toasts);
    }

    pub fn duplicate(&mut self, toasts: &mut Toasts) {
        let cur = self.current();
        let r = self.frames[cur].clone();
        self.insert(cur + 1, r, toasts);
    }

    pub fn delete(&mut self, toasts: &mut Toasts) {
        if self.frames.len() <= 1 {
            toasts.push(ToastKind::Error, "An animation needs at least one frame.");
            return;
        }
        self.snapshot();
        let cur = self.current();
        self.frames.remove(cur);
        self.ink.remove(cur);
        self.texs.remove(cur);
        self.dirty.remove(cur);
        self.resize_clock();
        self.goto(cur.min(self.frames.len() - 1));
    }

    pub fn clear(&mut self) {
        self.snapshot();
        let cur = self.current();
        self.frames[cur] = Raster::new(W, H);
        self.ink[cur] = false;
        self.dirty[cur] = true;
    }

    pub fn draw_to(&mut self, pt: (f32, f32)) {
        let cur = self.current();
        let b = self.brush;
        match self.last_dab {
            Some(from) => {
                b.stroke(&mut self.frames[cur], from, pt, self.erase, None);
            }
            None => b.dab(&mut self.frames[cur], pt.0, pt.1, self.erase, None),
        }
        self.last_dab = Some(pt);
        self.dirty[cur] = true;
        if !self.erase {
            self.ink[cur] = true;
        }
    }

    fn tex(&mut self, ctx: &egui::Context, i: usize) -> Option<egui::TextureId> {
        if i >= self.frames.len() {
            return None;
        }
        if self.dirty[i] || self.texs[i].is_none() {
            let r = &self.frames[i];
            let img = egui::ColorImage::from_rgba_unmultiplied(
                [r.width as usize, r.height as usize],
                &r.data,
            );
            match &mut self.texs[i] {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => {
                    self.texs[i] = Some(ctx.load_texture(
                        format!("anim-{}", i),
                        img,
                        egui::TextureOptions::LINEAR,
                    ))
                }
            }
            self.dirty[i] = false;
            if self.erase {
                self.ink[i] = self.frames[i]
                    .data
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] > 0);
            }
        }
        self.texs[i].as_ref().map(|t| t.id())
    }

    pub fn tools(&mut self, ui: &mut egui::Ui, p: &Palette) {
        theme::card(ui, p, "Tools", |ui| {
            if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    theme::toggle_button(
                        p,
                        &format!("{}   Pencil", egui_phosphor::regular::PENCIL_SIMPLE),
                        !self.erase,
                    ),
                )
                .on_hover_text("B")
                .clicked()
            {
                self.erase = false;
            }
            if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    theme::toggle_button(
                        p,
                        &format!("{}   Eraser", egui_phosphor::regular::ERASER),
                        self.erase,
                    ),
                )
                .on_hover_text("E")
                .clicked()
            {
                self.erase = true;
            }
        });
        theme::card(ui, p, "Pencil", |ui| {
            ui.horizontal(|ui| {
                ui.label(theme::muted(p, "Colour"));
                egui::color_picker::color_edit_button_srgb(ui, &mut self.brush.color);
            });
            ui.add(
                egui::Slider::new(&mut self.brush.size, 1.0..=60.0)
                    .suffix(" px")
                    .text("Size"),
            );
        });
        theme::card(ui, p, "Onion skin", |ui| {
            ui.checkbox(&mut self.onion, "Show neighbouring frames");
            ui.add_enabled(
                self.onion,
                egui::Slider::new(&mut self.onion_range, 1..=3).text("Frames"),
            );
            ui.label(theme::muted(p, "Red = before, green = after."));
        });
    }

    pub fn center(&mut self, ui: &mut egui::Ui, p: &Palette) {
        let ctx = ui.ctx().clone();
        let (resp, painter) =
            ui.allocate_painter(ui.available_size(), egui::Sense::click_and_drag());
        let rect = resp.rect;
        painter.rect_filled(rect, CornerRadius::ZERO, p.canvas);
        let zoom = ViewTransform::fit_zoom(rect.width(), rect.height(), (W, H), 32.0);
        let view = ViewTransform {
            viewport: (rect.min.x, rect.min.y, rect.width(), rect.height()),
            image: (W, H),
            zoom,
            pan: (0.0, 0.0),
        };
        let (x, y, w, h) = view.image_rect();
        let paper = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, h));
        painter.rect_filled(
            paper.translate(Vec2::new(0.0, 6.0)),
            CornerRadius::same(4),
            Color32::from_black_alpha(if p.dark { 120 } else { 40 }),
        );
        painter.rect_filled(
            paper,
            CornerRadius::same(2),
            Color32::from_rgb(250, 249, 245),
        );
        let uv = Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0));
        let cur = self.current();
        if self.onion && !self.clock.playing {
            for d in (1..=self.onion_range).rev() {
                let a = (110 / d) as u8;
                if cur >= d {
                    if let Some(t) = self.tex(&ctx, cur - d) {
                        painter.image(
                            t,
                            paper,
                            uv,
                            Color32::from_rgba_unmultiplied(255, 70, 70, a),
                        );
                    }
                }
                if let Some(t) = self.tex(&ctx, cur + d) {
                    painter.image(
                        t,
                        paper,
                        uv,
                        Color32::from_rgba_unmultiplied(60, 200, 90, a),
                    );
                }
            }
        }
        if let Some(t) = self.tex(&ctx, cur) {
            painter.image(t, paper, uv, Color32::WHITE);
        }
        painter.rect_stroke(
            paper,
            CornerRadius::same(2),
            Stroke::new(1.0_f32, p.border),
            egui::StrokeKind::Middle,
        );
        painter.text(
            paper.left_top() + Vec2::new(0.0, -SP_SM),
            egui::Align2::LEFT_BOTTOM,
            format!("Frame {} / {}", cur + 1, self.frames.len()),
            egui::FontId::proportional(12.0),
            p.muted,
        );

        if resp.drag_started_by(egui::PointerButton::Primary) || resp.clicked() {
            self.snapshot();
            self.last_dab = None;
        }
        if resp.dragged_by(egui::PointerButton::Primary) || resp.clicked() {
            if let Some(pp) = resp.interact_pointer_pos() {
                self.clock.playing = false;
                self.draw_to(view.to_image(pp.x, pp.y));
            }
        }
        if resp.drag_stopped() || resp.clicked() {
            self.last_dab = None;
        }
        if let Some(hp) = resp.hover_pos() {
            painter.circle_stroke(
                hp,
                (self.brush.size / 2.0 * zoom).max(1.5),
                Stroke::new(1.0_f32, p.accent),
            );
            ctx.set_cursor_icon(egui::CursorIcon::None);
        }
    }

    pub fn inspector(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        toasts: &mut Toasts,
        bus: &mut Vec<AppCommand>,
    ) {
        theme::card(ui, p, "Sequence", |ui| {
            theme::kv(
                ui,
                p,
                "Frame",
                format!("{} of {}", self.current() + 1, self.frames.len()),
            );
            theme::kv(ui, p, "Canvas", format!("{} × {}", W, H));
            theme::kv(ui, p, "Duration", format!("{:.2} s", self.clock.duration));
            let mut fps = self.fps;
            if ui
                .add(
                    egui::Slider::new(&mut fps, 1.0..=30.0)
                        .step_by(1.0)
                        .suffix(" fps")
                        .text("Rate"),
                )
                .changed()
            {
                self.set_fps(fps);
            }
        });
        theme::card(ui, p, "Frame", |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.add(theme::toggle_button(p, "+ New", false)).clicked() {
                    self.add_frame(toasts);
                }
                if ui
                    .add(theme::toggle_button(p, "Duplicate", false))
                    .on_hover_text("New key drawing from this frame  (K)")
                    .clicked()
                {
                    bus.push(AppCommand::SetKeyframe);
                }
                if ui.add(theme::toggle_button(p, "Clear", false)).clicked() {
                    self.clear();
                }
                if ui
                    .add_enabled(
                        self.frames.len() > 1,
                        theme::toggle_button(p, "Delete", false),
                    )
                    .clicked()
                {
                    bus.push(AppCommand::DeleteKeyframe);
                }
            });
            ui.add_space(SP_XS);
            ui.label(theme::muted(
                p,
                ", and . step frames · Space plays · Ctrl+Z undoes",
            ));
        });
    }

    pub fn timeline(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        toasts: &mut Toasts,
        bus: &mut Vec<AppCommand>,
    ) {
        ui.horizontal(|ui| {
            crate::timeline::transport(ui, p, &mut self.clock, bus);
            ui.add_space(SP_MD);
            if ui.add(theme::toggle_button(p, "+ Frame", false)).clicked() {
                self.add_frame(toasts);
            }
        });
        ui.add_space(SP_XS);
        let cur = self.current();
        egui::ScrollArea::horizontal()
            .id_salt("frames")
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = SP_XS;
                    let mut go = None;
                    for i in 0..self.frames.len() {
                        let (r, resp) =
                            ui.allocate_exact_size(Vec2::new(34.0, 44.0), egui::Sense::click());
                        let on = i == cur;
                        let hov = resp.hovered();
                        let fill = if on {
                            p.accent
                        } else if hov {
                            p.border
                        } else {
                            p.surface_hi
                        };
                        ui.painter()
                            .rect_filled(r, CornerRadius::same(R_SM as u8), fill);
                        let fg = if on { p.accent_text } else { p.muted };
                        ui.painter().text(
                            r.center_top() + Vec2::new(0.0, 8.0),
                            egui::Align2::CENTER_TOP,
                            format!("{}", i + 1),
                            egui::FontId::monospace(11.0),
                            fg,
                        );
                        let dot = r.center_bottom() - Vec2::new(0.0, 10.0);
                        if self.ink[i] {
                            ui.painter().circle_filled(
                                dot,
                                4.0,
                                if on { p.accent_text } else { p.text },
                            );
                        } else {
                            ui.painter()
                                .circle_stroke(dot, 3.5, Stroke::new(1.0_f32, fg));
                        }
                        if resp
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            go = Some(i);
                        }
                    }
                    if let Some(i) = go {
                        bus.push(AppCommand::TimelinePlay {
                            playing: Some(false),
                        });
                        bus.push(AppCommand::TimelineScrub {
                            time: (i as f32 + 0.01) / self.fps,
                        });
                    }
                });
            });
    }
}

// ---- project files ------------------------------------------------------------

use crate::project::{Assets, RasterRef};
use serde::{Deserialize, Serialize};

/// Animate mode in a `.seraphina` project.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnimFile {
    pub frames: Vec<RasterRef>,
    pub fps: f32,
    pub clock: Clock,
    pub brush: Brush,
    pub erase: bool,
    pub onion: bool,
    pub onion_range: usize,
}

impl AnimState {
    pub fn to_file(&self, a: &mut Assets) -> AnimFile {
        AnimFile {
            frames: self
                .frames
                .iter()
                .enumerate()
                .map(|(i, f)| a.put_raster(&format!("frame {}", i + 1), f))
                .collect(),
            fps: self.fps,
            clock: self.clock.clone(),
            brush: self.brush,
            erase: self.erase,
            onion: self.onion,
            onion_range: self.onion_range,
        }
    }

    pub fn load_file(&mut self, f: AnimFile, a: &Assets) -> Result<(), String> {
        let mut frames = Vec::with_capacity(f.frames.len());
        for r in &f.frames {
            let r = a.raster(r)?;
            if (r.width, r.height) != (W, H) {
                return Err(format!(
                    "animation frame is {}×{}, expected {}×{}",
                    r.width, r.height, W, H
                ));
            }
            frames.push(r);
        }
        if frames.is_empty() || frames.len() > MAX_FRAMES {
            return Err(format!(
                "animation has {} frames (1-{})",
                frames.len(),
                MAX_FRAMES
            ));
        }
        self.history.clear();
        self.last_dab = None;
        self.fps = f.fps.clamp(1.0, 60.0);
        self.restore(frames);
        self.clock = f.clock;
        self.brush = f.brush;
        self.erase = f.erase;
        self.onion = f.onion;
        self.onion_range = f.onion_range;
        Ok(())
    }
}

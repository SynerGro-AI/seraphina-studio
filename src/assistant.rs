//! Safe pure Assistant (Seraphina side-panel plumbing).
//!
//! Backend trait for chat replies plus an online-learning hook for
//! small, incremental weight updates. The shipped backend is an
//! offline help table: it does not call any model and says so. The shipped
//! learner nudges `LiveWeights` by bounded deltas (no full rewrites).
//! No unwraps.
//!
//! Differential: feedback on a reply changes exactly one weight.

use crate::live_weights::LiveWeights;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    pub role: Role,
    pub text: String,
}

impl ChatMessage {
    pub fn new(role: Role, text: &str) -> Self {
        Self {
            role,
            text: text.to_string(),
        }
    }
}

/// What the workspace tells the backend about the current state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorkspaceContext {
    pub mode: String,
    pub tool: String,
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AssistantError {
    EmptyPrompt,
    NotConnected(String),
}

/// A chat backend. Implement this to hook up a real model.
pub trait AssistantBackend {
    /// Short name shown in the panel header.
    fn name(&self) -> &str;
    /// True when replies come from a real model.
    fn is_connected(&self) -> bool;
    /// Produce a reply to the latest user message.
    fn respond(
        &mut self,
        history: &[ChatMessage],
        ctx: &WorkspaceContext,
    ) -> Result<String, AssistantError>;
}

/// Offline help. Answers from a fixed help table and states that no
/// model is connected.
#[derive(Clone, Debug, Default)]
pub struct OfflineBackend;

impl AssistantBackend for OfflineBackend {
    fn name(&self) -> &str {
        "Seraphina (offline help)"
    }

    fn is_connected(&self) -> bool {
        false
    }

    fn respond(
        &mut self,
        history: &[ChatMessage],
        ctx: &WorkspaceContext,
    ) -> Result<String, AssistantError> {
        let prompt = history
            .iter()
            .rev()
            .find(|m| m.role == Role::User)
            .map(|m| m.text.trim().to_string())
            .unwrap_or_default();
        if prompt.is_empty() {
            return Err(AssistantError::EmptyPrompt);
        }
        let tip = match ctx.mode.as_str() {
            "Stream" => "Stream mode steps the real bitstream reader. Pick a read width, press Step, and compare No-wrap with Wrap past byte 255.",
            "Paint" => "Paint mode: V move, M marquee, L lasso, W quick select, C crop, J healing, S clone (Alt-click source), B brush, E eraser, G gradient, O dodge / Shift+O burn, P pen, T type, H hand, Z zoom, I eyedropper. Ctrl+Z undoes.",
            "3D" => "3D mode renders with a z-buffered software rasterizer: Display offers Shaded (5), Wireframe (4) and Shaded + wire (6). Pick Select/Move/Rotate/Scale on the shelf and drag in the viewport (Alt-drag orbits). Extrude and Bevel edit the selected cube's top face.",
            "Animate" => "Animate mode draws frame by frame. Turn on onion skin to see neighbouring frames; Space plays.",
            "Composite" => "Composite mode keys layer opacity over time. Move the playhead, set opacity, press Add Key.",
            _ => "Pick a mode at the top to get started.",
        };
        Ok(format!(
            "Offline: no language model is configured, so this is a built-in tip rather than a real answer.\n\n{}\n\nSet XAI_API_KEY (or SERAPHINA_BASE_URL for another OpenAI-compatible server) and restart to let Seraphina act on the app.",
            tip
        ))
    }
}

/// One observation the learner may use.
#[derive(Clone, Debug, PartialEq)]
pub enum LearningEvent {
    /// User rated a reply.
    Feedback { helpful: bool },
    /// User picked a tool in a mode.
    ToolUsed { mode: String, tool: String },
}

/// Online-learning hook. Implementations must apply small bounded
/// updates per event, never retrain or replace all weights.
pub trait OnlineLearner {
    fn observe(&mut self, event: &LearningEvent) -> Option<(String, f32)>;
    fn updates(&self) -> usize;
}

/// Learner that nudges `LiveWeights` by a fixed step, clamped.
#[derive(Clone, Debug)]
pub struct WeightNudgeLearner {
    pub weights: LiveWeights,
    pub step: f32,
    updates: usize,
}

impl WeightNudgeLearner {
    pub fn new(step: f32) -> Self {
        Self {
            weights: LiveWeights::new(),
            step,
            updates: 0,
        }
    }

    fn nudge(&mut self, name: &str, delta: f32, lo: f32, hi: f32) -> Option<(String, f32)> {
        let cur = self.weights.get(name).unwrap_or(0.0);
        let next = (cur + delta).clamp(lo, hi);
        self.weights.set(name, next).ok()?;
        self.updates += 1;
        Some((name.to_string(), next))
    }
}

impl OnlineLearner for WeightNudgeLearner {
    fn observe(&mut self, event: &LearningEvent) -> Option<(String, f32)> {
        match event {
            LearningEvent::Feedback { helpful } => {
                let d = if *helpful { self.step } else { -self.step };
                self.nudge("assistant.helpfulness", d, -1.0, 1.0)
            }
            LearningEvent::ToolUsed { mode, tool } => {
                let name = format!(
                    "tool.{}.{}",
                    mode.to_lowercase(),
                    tool.to_lowercase().replace(' ', "_")
                );
                let step = self.step;
                self.nudge(&name, step, 0.0, 100.0)
            }
        }
    }

    fn updates(&self) -> usize {
        self.updates
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(mode: &str) -> WorkspaceContext {
        WorkspaceContext {
            mode: mode.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn offline_help_says_it_is_offline() {
        let mut b = OfflineBackend;
        assert!(!b.is_connected());
        let r = b
            .respond(&[ChatMessage::new(Role::User, "help")], &ctx("Stream"))
            .unwrap_or_default();
        assert!(r.contains("no language model is configured"));
        assert!(r.contains("bitstream"));
    }

    #[test]
    fn empty_prompt_errors() {
        let mut b = OfflineBackend;
        assert_eq!(
            b.respond(&[ChatMessage::new(Role::User, "  ")], &ctx("Paint")),
            Err(AssistantError::EmptyPrompt)
        );
        assert_eq!(
            b.respond(&[], &ctx("Paint")),
            Err(AssistantError::EmptyPrompt)
        );
    }

    #[test]
    fn every_mode_has_tip() {
        let mut b = OfflineBackend;
        let mut seen = Vec::new();
        for m in ["Stream", "Paint", "3D", "Animate", "Composite"] {
            seen.push(
                b.respond(&[ChatMessage::new(Role::User, "?")], &ctx(m))
                    .unwrap_or_default(),
            );
        }
        seen.dedup();
        assert_eq!(seen.len(), 5);
    }

    #[test]
    fn feedback_nudges_one_weight() {
        let mut l = WeightNudgeLearner::new(0.1);
        let (name, v) = l
            .observe(&LearningEvent::Feedback { helpful: true })
            .unwrap_or_default();
        assert_eq!(name, "assistant.helpfulness");
        assert!((v - 0.1).abs() < 1e-6);
        l.observe(&LearningEvent::Feedback { helpful: false });
        assert!(l.weights.get("assistant.helpfulness").unwrap_or(9.0).abs() < 1e-6);
        assert_eq!(l.weights.len(), 1);
        assert_eq!(l.updates(), 2);
    }

    #[test]
    fn feedback_is_clamped() {
        let mut l = WeightNudgeLearner::new(0.6);
        for _ in 0..5 {
            l.observe(&LearningEvent::Feedback { helpful: true });
        }
        assert_eq!(l.weights.get("assistant.helpfulness").unwrap_or(0.0), 1.0);
    }

    #[test]
    fn tool_use_counts() {
        let mut l = WeightNudgeLearner::new(1.0);
        l.observe(&LearningEvent::ToolUsed {
            mode: "Paint".into(),
            tool: "Brush".into(),
        });
        let (n, v) = l
            .observe(&LearningEvent::ToolUsed {
                mode: "Paint".into(),
                tool: "Brush".into(),
            })
            .unwrap_or_default();
        assert_eq!(n, "tool.paint.brush");
        assert_eq!(v, 2.0);
    }
}

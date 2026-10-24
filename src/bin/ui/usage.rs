//! Token and cost meter (M4/C3).
//!
//! xAI reports `usage.cost_in_usd_ticks` (1 USD = 1e10 ticks) on chat,
//! image and video responses. When a response has no cost (a stopped
//! stream, a job whose final status we never saw, a provider that does
//! not report it) we estimate from the public price list and mark the
//! entry as estimated. The cap counts the last 24 hours across sessions,
//! kept in `~/.seraphina/studio/usage.json`. If that file can't be
//! saved, the app warns once (toast and a `usage-*.txt` log report) and the
//! cap keeps counting this session in memory.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

pub const TICKS_PER_USD: f64 = 1e10;
const DAY_SECS: u64 = 24 * 3600;

/// One billed call.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub ticks: u64,
    /// The cost came from our price table, not from xAI.
    pub estimated: bool,
}

impl Usage {
    pub fn usd(&self) -> f64 {
        self.ticks as f64 / TICKS_PER_USD
    }
}

fn ticks(usd: f64) -> u64 {
    (usd.max(0.0) * TICKS_PER_USD).round() as u64
}

fn price_env(name: &str) -> Option<f64> {
    std::env::var(name)
        .ok()
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
}

/// USD per 1M (input, output) tokens. Unknown models use the flagship
/// price so an estimate errs high. Override with SERAPHINA_PRICE_IN /
/// SERAPHINA_PRICE_OUT.
pub fn chat_price(model: &str) -> (f64, f64) {
    let m = model.to_ascii_lowercase();
    let table = if m.starts_with("grok-4.3") || m.starts_with("grok-4.20") {
        (1.25, 2.5)
    } else {
        (2.0, 6.0)
    };
    (
        price_env("SERAPHINA_PRICE_IN").unwrap_or(table.0),
        price_env("SERAPHINA_PRICE_OUT").unwrap_or(table.1),
    )
}

/// Read a `usage` object (or a response that contains one). Accepts the
/// Chat Completions (`prompt_tokens`) and Responses (`input_tokens`)
/// spellings.
pub fn parse_usage(v: &Value) -> Option<Usage> {
    let u = match v.get("usage") {
        Some(u) if u.is_object() => u,
        _ if v.get("prompt_tokens").is_some() || v.get("cost_in_usd_ticks").is_some() => v,
        _ => return None,
    };
    let n = |a: &str, b: &str| {
        u.get(a)
            .or_else(|| u.get(b))
            .and_then(|x| x.as_u64())
            .unwrap_or(0)
    };
    let cost = u.get("cost_in_usd_ticks").and_then(|x| x.as_u64());
    Some(Usage {
        prompt_tokens: n("prompt_tokens", "input_tokens"),
        completion_tokens: n("completion_tokens", "output_tokens"),
        ticks: cost.unwrap_or(0),
        estimated: cost.is_none(),
    })
}

/// Fill in a missing cost from the price table.
pub fn priced(mut u: Usage, model: &str) -> Usage {
    if u.estimated || u.ticks == 0 {
        let (i, o) = chat_price(model);
        let est =
            ticks((u.prompt_tokens as f64 * i + u.completion_tokens as f64 * o) / 1_000_000.0);
        if u.ticks == 0 {
            u.ticks = est;
            u.estimated = true;
        }
    }
    u
}

/// Rough tokens for text we never got a count for (about 4 chars each).
pub fn chat_estimate(model: &str, prompt_chars: usize, completion_chars: usize) -> Usage {
    priced(
        Usage {
            prompt_tokens: prompt_chars.div_ceil(4) as u64,
            completion_tokens: completion_chars.div_ceil(4) as u64,
            ticks: 0,
            estimated: true,
        },
        model,
    )
}

/// Price of one Grok Imagine video second (USD).
pub fn video_per_second(model: &str, resolution: &str) -> f64 {
    if let Some(p) = price_env("SERAPHINA_PRICE_VIDEO_S") {
        return p;
    }
    let m = model.to_ascii_lowercase();
    let r = resolution.to_ascii_lowercase();
    let tier = if r.contains("1080") {
        2
    } else if r.contains("720") {
        1
    } else {
        0
    };
    let row: [f64; 3] = if m.contains("1.5-lite") {
        [0.02, 0.03, 0.14]
    } else if m.contains("1.5") {
        [0.08, 0.14, 0.25]
    } else {
        // grok-imagine-video has no 1080p tier; price it like 1.5.
        [0.05, 0.07, 0.25]
    };
    row[tier]
}

pub fn video_estimate(model: &str, resolution: &str, seconds: u32) -> Usage {
    Usage {
        ticks: ticks(video_per_second(model, resolution) * seconds.max(1) as f64),
        estimated: true,
        ..Default::default()
    }
}

/// Price of one generated image (USD); errs high for 2.0.
pub fn image_price(model: &str) -> f64 {
    if let Some(p) = price_env("SERAPHINA_PRICE_IMAGE") {
        return p;
    }
    if model.to_ascii_lowercase().contains("2.0") {
        0.08
    } else {
        0.02
    }
}

pub fn image_estimate(model: &str) -> Usage {
    Usage {
        ticks: ticks(image_price(model)),
        estimated: true,
        ..Default::default()
    }
}

/// Running totals (saved per project in the manifest).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Totals {
    pub calls: u64,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub usd_ticks: u64,
    /// Calls whose cost was estimated.
    pub estimated_calls: u64,
}

impl Totals {
    pub fn add(&mut self, u: &Usage) {
        self.calls += 1;
        self.prompt_tokens += u.prompt_tokens;
        self.completion_tokens += u.completion_tokens;
        self.usd_ticks += u.ticks;
        self.estimated_calls += u.estimated as u64;
    }
    pub fn usd(&self) -> f64 {
        self.usd_ticks as f64 / TICKS_PER_USD
    }
    pub fn tokens(&self) -> u64 {
        self.prompt_tokens + self.completion_tokens
    }
    pub fn summary(&self) -> String {
        format!(
            "${:.4} · {} tokens · {} call{}{}",
            self.usd(),
            self.tokens(),
            self.calls,
            if self.calls == 1 { "" } else { "s" },
            if self.estimated_calls > 0 {
                format!(" ({} estimated)", self.estimated_calls)
            } else {
                String::new()
            }
        )
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Ledger {
    /// (unix seconds, ticks) for the last 24 hours.
    entries: Vec<(u64, u64)>,
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The app's meter: this session, the open project, and a rolling 24 h
/// ledger the spend cap is checked against.
pub struct Meter {
    pub session: Totals,
    pub project: Totals,
    ledger: Ledger,
    path: Option<PathBuf>,
    /// Where a failed save is logged (a `usage-*.txt` report).
    log_dir: Option<PathBuf>,
    /// The last save failure, until a save works again.
    save_error: Option<String>,
    /// A save failure not yet shown to the user.
    warning: Option<String>,
    /// USD per 24 h; 0 turns the cap off.
    pub cap_usd: f64,
}

impl Meter {
    pub fn path() -> PathBuf {
        crate::output::studio_dir().join("usage.json")
    }

    /// Load the 24 h ledger from disk (a damaged file starts fresh).
    pub fn load(cap_usd: f64) -> Self {
        Self::load_from(Self::path(), crate::crash::logs_dir(), cap_usd)
    }

    /// `load` with the ledger file and the log folder given.
    pub fn load_from(path: PathBuf, log_dir: PathBuf, cap_usd: f64) -> Self {
        let ledger = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut m = Self::with(Some(path), ledger, cap_usd);
        m.log_dir = Some(log_dir);
        m.prune(now_secs());
        m
    }

    /// In-memory only (tests).
    pub fn ephemeral(cap_usd: f64) -> Self {
        Self::with(None, Ledger::default(), cap_usd)
    }

    fn with(path: Option<PathBuf>, ledger: Ledger, cap_usd: f64) -> Self {
        Self {
            session: Totals::default(),
            project: Totals::default(),
            ledger,
            path,
            log_dir: None,
            save_error: None,
            warning: None,
            cap_usd,
        }
    }

    fn prune(&mut self, now: u64) {
        self.ledger
            .entries
            .retain(|(t, _)| now.saturating_sub(*t) < DAY_SECS);
    }

    pub fn record(&mut self, u: &Usage) {
        self.record_at(u, now_secs());
    }

    fn record_at(&mut self, u: &Usage, now: u64) {
        self.session.add(u);
        self.project.add(u);
        self.prune(now);
        if u.ticks > 0 {
            self.ledger.entries.push((now, u.ticks));
        }
        if let Some(p) = self.path.clone() {
            let saved = serde_json::to_vec(&self.ledger)
                .map_err(|e| e.to_string())
                .and_then(|b| crate::project::write_atomic(&p, &b).map_err(|e| e.to_string()));
            match saved {
                Ok(()) => self.save_error = None,
                Err(e) => self.save_failed(&p, &e),
            }
        }
    }

    /// Warn once per run of failures: a toast (via `take_warning`), stderr
    /// and a log report.
    fn save_failed(&mut self, path: &std::path::Path, e: &str) {
        let msg = format!(
            "Couldn't save the xAI spending history to {}: {}. The spend cap still counts this session, but after a restart the last 24 h will start from zero.",
            path.display(),
            e
        );
        if self.save_error.is_none() {
            eprintln!("warning: {msg}");
            if let Some(dir) = &self.log_dir {
                if let Err(le) = crate::crash::write_report_in(dir, "usage", &msg, "") {
                    eprintln!("warning: couldn't write a log in {}: {le}", dir.display());
                }
            }
            self.warning = Some(msg.clone());
        }
        self.save_error = Some(msg);
    }

    /// A save failure to show the user, once.
    pub fn take_warning(&mut self) -> Option<String> {
        self.warning.take()
    }

    pub fn last_24h_usd(&self) -> f64 {
        let now = now_secs();
        self.ledger
            .entries
            .iter()
            .filter(|(t, _)| now.saturating_sub(*t) < DAY_SECS)
            .map(|(_, k)| *k as f64)
            .sum::<f64>()
            / TICKS_PER_USD
    }

    /// May a call costing about `next_usd` go out? Err is the toast.
    pub fn check(&self, next_usd: f64) -> Result<(), String> {
        if self.cap_usd <= 0.0 {
            return Ok(());
        }
        let spent = self.last_24h_usd();
        if spent >= self.cap_usd {
            return Err(format!(
                "Spend cap reached (${:.2} of ${:.2} in the last 24 h), so nothing was sent to xAI. Raise or turn off the cap in Settings → xAI spending.",
                spent, self.cap_usd
            ));
        }
        if spent + next_usd > self.cap_usd {
            return Err(format!(
                "This would pass your spend cap (${:.2} of ${:.2} used in the last 24 h; this call costs about ${:.2}), so nothing was sent to xAI. Raise or turn off the cap in Settings → xAI spending.",
                spent, self.cap_usd, next_usd
            ));
        }
        Ok(())
    }

    /// Status-bar label.
    pub fn label(&self) -> String {
        format!(
            "${:.2} · {}",
            self.session.usd(),
            compact(self.session.tokens())
        )
    }

    pub fn tooltip(&self) -> String {
        let cap = if self.cap_usd > 0.0 {
            format!("${:.2} per 24 h", self.cap_usd)
        } else {
            "off".into()
        };
        format!(
            "xAI usage\nThis session: {}\nThis project: {}\nLast 24 h: ${:.4}\nSpend cap: {}\nCosts are xAI's own figures where reported; others are estimates from the public price list.",
            self.session.summary(),
            self.project.summary(),
            self.last_24h_usd(),
            cap
        )
    }
}

fn compact(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M tok", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}k tok", n as f64 / 1e3)
    } else {
        format!("{} tok", n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_xai_usage_and_prefers_its_cost() {
        let v = json!({"usage":{"prompt_tokens":1200,"completion_tokens":300,"cost_in_usd_ticks":42_000_000u64}});
        let u = parse_usage(&v).unwrap();
        assert_eq!((u.prompt_tokens, u.completion_tokens), (1200, 300));
        assert_eq!(priced(u, "grok-4.7"), u, "xAI's cost is kept");
        assert!(!u.estimated);
        assert!((u.usd() - 0.0042).abs() < 1e-12);
        let r = json!({"usage":{"input_tokens":1000000,"output_tokens":0}});
        let u = priced(parse_usage(&r).unwrap(), "grok-4.7");
        assert!(u.estimated && (u.usd() - 2.0).abs() < 1e-9);
        assert!(parse_usage(&json!({"choices":[]})).is_none());
        assert!(parse_usage(&json!({"usage":null})).is_none());
    }

    #[test]
    fn estimates_err_high() {
        assert_eq!(chat_price("grok-4.3"), (1.25, 2.5));
        assert_eq!(chat_price("some-new-model"), (2.0, 6.0));
        let u = chat_estimate("grok-4.7", 4000, 400);
        assert_eq!((u.prompt_tokens, u.completion_tokens), (1000, 100));
        assert!((u.usd() - 0.0026).abs() < 1e-9);
        assert!((video_estimate("grok-imagine-video-1.5", "720p", 6).usd() - 0.84).abs() < 1e-9);
        assert!((video_estimate("grok-imagine-video", "480p", 1).usd() - 0.05).abs() < 1e-9);
        assert!((image_estimate("grok-imagine-image-2.0").usd() - 0.08).abs() < 1e-9);
    }

    #[test]
    fn cap_blocks_once_reached_and_counts_only_24h() {
        let mut m = Meter::ephemeral(1.0);
        assert!(m.check(0.5).is_ok());
        let now = now_secs();
        let dollar = Usage {
            ticks: ticks(0.9),
            ..Default::default()
        };
        m.record_at(&dollar, now - DAY_SECS - 5); // yesterday
        assert!(m.check(0.5).is_ok(), "older than 24 h doesn't count");
        m.record_at(&dollar, now);
        let e = m.check(0.5).unwrap_err();
        assert!(e.contains("would pass your spend cap"), "{e}");
        assert!(m.check(0.05).is_ok());
        m.record_at(&dollar, now);
        let e = m.check(0.0).unwrap_err();
        assert!(e.starts_with("Spend cap reached ($1.80 of $1.00"), "{e}");
        assert_eq!(m.session.calls, 3);
        m.cap_usd = 0.0;
        assert!(m.check(100.0).is_ok(), "0 turns the cap off");
        assert!(m.tooltip().contains("Spend cap: off"));
    }

    #[test]
    fn ledger_survives_a_restart() {
        let tmp = crate::test_dir::TestDir::new("usage");
        let path = tmp.path().join("usage.json");
        let mut m = Meter::with(Some(path.clone()), Ledger::default(), 1.0);
        m.record(&Usage {
            ticks: ticks(1.5),
            ..Default::default()
        });
        let b = std::fs::read(&path).unwrap();
        let again = Meter::with(Some(path), serde_json::from_slice(&b).unwrap(), 1.0);
        assert!((again.last_24h_usd() - 1.5).abs() < 1e-9);
        assert!(again.check(0.0).is_err());
        assert_eq!(again.session, Totals::default(), "session starts at zero");
        crate::test_dir::fail_late();
    }
}

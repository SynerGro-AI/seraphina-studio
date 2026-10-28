//! Grok Imagine video panel over a `VideoGenProvider`.
//!
//! `GrokImagine` follows the documented xAI REST flow: POST
//! `/v1/videos/generations` (or `/v1/videos/extensions`) returns a
//! `request_id`; GET `/v1/videos/{id}` until `status` is `done` (with
//! `video.url`) or `failed`. Finished clips are downloaded to
//! `~/.seraphina/studio/videos/`. Stitching writes an ffmpeg concat list
//! and runs ffmpeg (bundled or on PATH) on a worker thread into a new
//! `stitched_<UTC time>.mp4`. `generate_image` covers
//! POST `/v1/images/generations` for Paint's "Generate layer".

use crate::commands::AppCommand;
use crate::net::{self, Cancel};
use crate::theme::{self, Palette, SP_SM};
use crate::usage::{self, Usage};
use eframe::egui;
use egui_phosphor::regular as ic;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

/// Read timeout for Imagine API calls and downloads.
const API_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(120);
/// Seconds between status polls, and how many polls before giving up.
const POLL_EVERY: Duration = Duration::from_secs(5);
const POLLS: u32 = 180;
/// Consecutive failed polls tolerated before a clip is marked failed.
const POLL_FAILURES: u32 = 3;
/// Length of an extension.
const EXTEND_SECONDS: u32 = 6;

/// Shown when the user stops a job.
pub const STOPPED_NOTE: &str =
    "stopped by you (a job xAI had already accepted may still finish and be billed)";

#[derive(Clone, Debug, PartialEq)]
pub enum JobStatus {
    Pending,
    Done { url: String, duration: f32 },
    Failed(String),
}

pub trait VideoGenProvider: Send + Sync {
    fn name(&self) -> String;
    /// Submit a job. Only "not accepted" failures (429/503, no
    /// connection) are retried, so a retry can't start a second paid job.
    fn start_generate(
        &self,
        prompt: &str,
        duration: u32,
        cancel: &AtomicBool,
    ) -> Result<String, String>;
    fn start_extend(
        &self,
        video_url: &str,
        prompt: &str,
        cancel: &AtomicBool,
    ) -> Result<String, String>;
    fn poll(&self, request_id: &str, cancel: &AtomicBool) -> Result<JobStatus, String>;
    /// About what a job of this many seconds costs.
    fn estimate(&self, _seconds: u32) -> Usage {
        Usage::default()
    }
}

pub struct GrokImagine {
    base: String,
    key: String,
    pub model: String,
    pub resolution: String,
    pub image_model: String,
}

/// Minimal standard base64 decoder (no extra dependency).
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let s = s.split(',').next_back()?.trim();
    let val = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => (c - b'A') as u32,
            b'a'..=b'z' => (c - b'a' + 26) as u32,
            b'0'..=b'9' => (c - b'0' + 52) as u32,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        })
    };
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|c| !c.is_ascii_whitespace() && *c != b'=')
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    for chunk in bytes.chunks(4) {
        let mut n = 0u32;
        for (i, &c) in chunk.iter().enumerate() {
            n |= val(c)? << (18 - 6 * i);
        }
        let k = chunk.len();
        if k < 2 {
            return None;
        }
        out.push((n >> 16) as u8);
        if k > 2 {
            out.push((n >> 8) as u8);
        }
        if k > 3 {
            out.push(n as u8);
        }
    }
    Some(out)
}

impl GrokImagine {
    pub fn from_env() -> Option<Self> {
        let (key, _) = crate::secrets::xai_key()?;
        let base = std::env::var("SERAPHINA_VIDEO_BASE_URL")
            .unwrap_or_else(|_| "https://api.x.ai/v1".into());
        let model =
            std::env::var("SERAPHINA_VIDEO_MODEL").unwrap_or_else(|_| Self::DEFAULT_MODEL.into());
        let resolution =
            std::env::var("SERAPHINA_VIDEO_RESOLUTION").unwrap_or_else(|_| "480p".into());
        let image_model = std::env::var("SERAPHINA_IMAGE_MODEL")
            .unwrap_or_else(|_| Self::DEFAULT_IMAGE_MODEL.into());
        Some(Self {
            base: base.trim_end_matches('/').to_string(),
            key: key.trim().to_string(),
            model,
            resolution,
            image_model,
        })
    }

    pub const DEFAULT_MODEL: &'static str = "grok-imagine-video-1.5";
    pub const DEFAULT_IMAGE_MODEL: &'static str = "grok-imagine-image-2.0";

    /// Text-to-image; returns encoded image bytes (PNG/JPEG) and what
    /// the call cost.
    pub fn generate_image(
        &self,
        prompt: &str,
        cancel: &AtomicBool,
    ) -> Result<(Vec<u8>, Usage), String> {
        let v = self.post(
            "/images/generations",
            json!({ "model": self.image_model, "prompt": prompt, "n": 1, "response_format": "b64_json" }),
            cancel,
        )?;
        let cost = usage::parse_usage(&v)
            .filter(|u| !u.estimated)
            .unwrap_or_else(|| usage::image_estimate(&self.image_model));
        self.image_bytes(&v, cancel).map(|b| (b, cost))
    }

    fn image_bytes(&self, v: &Value, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
        let d = v.pointer("/data/0").ok_or_else(|| {
            format!(
                "no image in response: {}",
                v.to_string().chars().take(200).collect::<String>()
            )
        })?;
        if let Some(b) = d.get("b64_json").and_then(|b| b.as_str()) {
            return b64_decode(b).ok_or_else(|| "bad base64 image".to_string());
        }
        let url = d
            .get("url")
            .and_then(|u| u.as_str())
            .ok_or("image response had neither b64_json nor url")?;
        let a = net::agent(DOWNLOAD_TIMEOUT);
        let r = net::with_retry(
            cancel,
            net::Policy::Transient,
            || a.get(url),
            None,
            &mut |_, _, _| {},
        )
        .map_err(fail_text)?;
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut r.into_reader(), &mut buf).map_err(|e| e.to_string())?;
        Ok(buf)
    }

    fn post(&self, path: &str, body: Value, cancel: &AtomicBool) -> Result<Value, String> {
        let url = format!("{}{}", self.base, path);
        net::key_allowed(&url)?;
        let a = net::agent(API_TIMEOUT);
        let payload = body.to_string();
        net::with_retry(
            cancel,
            net::Policy::NotAccepted,
            || {
                a.post(&url)
                    .set("Authorization", &format!("Bearer {}", self.key))
                    .set("Content-Type", "application/json")
            },
            Some(&payload),
            &mut |_, _, _| {},
        )
        .map_err(fail_text)?
        .into_json()
        .map_err(|e| e.to_string())
    }
}

fn fail_text(f: net::Failure) -> String {
    match f {
        net::Failure::Stopped => net::STOPPED.into(),
        net::Failure::Http(e) => http_err(*e),
    }
}

fn http_err(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, r) => {
            let body = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|v| {
                    v.pointer("/error/message")
                        .or_else(|| v.get("error"))
                        .and_then(|m| m.as_str().map(String::from))
                })
                .unwrap_or(body);
            format!(
                "HTTP {}: {}",
                code,
                msg.chars().take(240).collect::<String>()
            )
        }
        other => other.to_string(),
    }
}

pub fn parse_status(v: &Value) -> JobStatus {
    let err = || {
        v.pointer("/error/message")
            .and_then(|m| m.as_str())
            .map(String::from)
    };
    match v
        .get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("pending")
    {
        "done" => match v
            .pointer("/video/url")
            .and_then(|u| u.as_str())
            .filter(|u| !u.is_empty())
        {
            Some(u) => JobStatus::Done {
                url: u.to_string(),
                duration: v
                    .pointer("/video/duration")
                    .and_then(|d| d.as_f64())
                    .unwrap_or(0.0) as f32,
            },
            None if v
                .pointer("/video/respect_moderation")
                .and_then(|m| m.as_bool())
                == Some(false) =>
            {
                JobStatus::Failed("blocked by content moderation".into())
            }
            None => JobStatus::Failed("done without a video URL".into()),
        },
        "failed" => JobStatus::Failed(err().unwrap_or_else(|| "generation failed".into())),
        "expired" => JobStatus::Failed("request expired".into()),
        _ => JobStatus::Pending,
    }
}

impl VideoGenProvider for GrokImagine {
    fn name(&self) -> String {
        format!("Grok Imagine · {} · {}", self.model, self.resolution)
    }

    fn start_generate(
        &self,
        prompt: &str,
        duration: u32,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let v = self.post(
            "/videos/generations",
            json!({ "model": self.model, "prompt": prompt, "duration": duration.clamp(1, 15), "aspect_ratio": "16:9", "resolution": self.resolution }),
            cancel,
        )?;
        v.get("request_id")
            .and_then(|r| r.as_str())
            .map(String::from)
            .ok_or_else(|| format!("no request_id in {}", v))
    }

    fn start_extend(
        &self,
        video_url: &str,
        prompt: &str,
        cancel: &AtomicBool,
    ) -> Result<String, String> {
        let v = self.post(
            "/videos/extensions",
            json!({ "model": self.model, "prompt": prompt, "video": { "url": video_url }, "duration": EXTEND_SECONDS }),
            cancel,
        )?;
        v.get("request_id")
            .and_then(|r| r.as_str())
            .map(String::from)
            .ok_or_else(|| format!("no request_id in {}", v))
    }

    fn estimate(&self, seconds: u32) -> Usage {
        usage::video_estimate(&self.model, &self.resolution, seconds)
    }

    fn poll(&self, id: &str, cancel: &AtomicBool) -> Result<JobStatus, String> {
        let url = format!("{}/videos/{}", self.base, id);
        net::key_allowed(&url)?;
        let a = net::agent(API_TIMEOUT);
        let v: Value = net::with_retry(
            cancel,
            net::Policy::Transient,
            || {
                a.get(&url)
                    .set("Authorization", &format!("Bearer {}", self.key))
            },
            None,
            &mut |_, _, _| {},
        )
        .map_err(fail_text)?
        .into_json()
        .map_err(|e| e.to_string())?;
        Ok(parse_status(&v))
    }
}

pub struct Clip {
    pub prompt: String,
    pub status: JobStatus,
    pub local: Option<PathBuf>,
    pub selected: bool,
    rx: Option<Receiver<(JobStatus, Option<PathBuf>)>>,
    cancel: Cancel,
}

/// A finished image job: (prompt, encoded bytes) or an error.
type ImageResult = Result<(String, Vec<u8>), String>;
/// Submits a job and returns its request id.
type StartJob =
    Box<dyn FnOnce(&dyn VideoGenProvider, &AtomicBool) -> Result<String, String> + Send>;

pub struct VideoGenPanel {
    pub provider: Option<std::sync::Arc<dyn VideoGenProvider>>,
    pub prompt: String,
    pub duration: u32,
    pub clips: Vec<Clip>,
    pub extend_prompt: String,
    pub log: Vec<String>,
    pub imager: Option<std::sync::Arc<GrokImagine>>,
    image_rx: Option<Receiver<ImageResult>>,
    image_cancel: Cancel,
    /// Running ffmpeg concat (D5: off the UI thread).
    stitch_rx: Option<Receiver<Result<String, String>>>,
    /// Time between status polls (tests shorten it).
    pub poll_every: Duration,
    /// Costs of accepted jobs; the app drains `spent` into its meter.
    spent_tx: std::sync::mpsc::Sender<Usage>,
    spent_rx: Receiver<Usage>,
    pub spent: Vec<Usage>,
}

pub fn videos_dir() -> PathBuf {
    crate::prefs::seraphina_home().join("studio").join("videos")
}

/// `stitched_<stamp>.mp4` in `dir`, never an existing file.
pub fn stitch_target(dir: &std::path::Path, stamp: &str) -> PathBuf {
    let mut out = dir.join(format!("stitched_{}.mp4", stamp));
    let mut n = 2;
    while out.exists() || out.with_extension("txt").exists() {
        out = dir.join(format!("stitched_{}_{}.mp4", stamp, n));
        n += 1;
    }
    out
}

/// ffmpeg concat demuxer, stream copy. Runs on a worker thread.
fn run_concat(list: &std::path::Path, out: &std::path::Path, n: usize) -> Result<String, String> {
    let ffmpeg = crate::output::find_ffmpeg().map_err(|e| {
        format!(
            "couldn't stitch: {} The concat list is at {}.",
            e,
            list.display()
        )
    })?;
    let mut cmd = std::process::Command::new(&ffmpeg);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-n",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
    ])
    .arg(list)
    .args(["-c", "copy"])
    .arg(out)
    .stdin(std::process::Stdio::null());
    crate::output::no_window(&mut cmd);
    match cmd.output() {
        Ok(o) if o.status.success() => {
            let _ = std::fs::remove_file(list);
            Ok(format!("stitched {} clips → {}", n, out.display()))
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            let last = err
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("ffmpeg failed");
            Err(format!("stitching failed: {}", last.trim()))
        }
        Err(e) => Err(format!("couldn't run ffmpeg ({}): {}", ffmpeg.display(), e)),
    }
}

impl VideoGenPanel {
    pub fn new() -> Self {
        let mut s = Self::unconfigured();
        s.reload_provider();
        s
    }

    /// Pick up a key saved in Settings without losing clips.
    pub fn reload_provider(&mut self) {
        self.imager = GrokImagine::from_env().map(std::sync::Arc::new);
        self.provider = self
            .imager
            .clone()
            .map(|g| g as std::sync::Arc<dyn VideoGenProvider>);
    }

    pub fn unconfigured() -> Self {
        let (spent_tx, spent_rx) = channel();
        Self {
            provider: None,
            prompt: String::new(),
            duration: 6,
            clips: Vec::new(),
            extend_prompt: String::new(),
            log: Vec::new(),
            imager: None,
            image_rx: None,
            image_cancel: net::cancel_flag(),
            stitch_rx: None,
            poll_every: POLL_EVERY,
            spent_tx,
            spent_rx,
            spent: Vec::new(),
        }
    }

    pub fn image_busy(&self) -> bool {
        self.image_rx.is_some()
    }

    /// Start a text-to-image job; the result arrives via `take_image`.
    pub fn generate_image(&mut self, prompt: &str) -> Result<String, String> {
        let g = self.imager.clone().ok_or(
            "Grok Imagine isn't configured: add an xAI API key in Settings (or set XAI_API_KEY)",
        )?;
        if prompt.trim().is_empty() {
            return Err("empty prompt".into());
        }
        if self.image_rx.is_some() {
            return Err("an image is already generating".into());
        }
        let (tx, rx) = channel();
        let pr = prompt.trim().to_string();
        let p2 = pr.clone();
        let cancel = net::cancel_flag();
        self.image_cancel = cancel.clone();
        let spent = self.spent_tx.clone();
        std::thread::spawn(move || {
            let r = g.generate_image(&p2, &cancel).map(|(b, u)| {
                let _ = spent.send(u);
                (p2, b)
            });
            let _ = tx.send(r);
        });
        self.image_rx = Some(rx);
        Ok(format!(
            "generating image “{}” with {}",
            pr.chars().take(60).collect::<String>(),
            self.imager
                .as_ref()
                .map(|g| g.image_model.clone())
                .unwrap_or_default()
        ))
    }

    /// About what generating a clip would cost (USD), for the spend cap.
    pub fn video_cost(&self, duration: Option<u32>) -> f64 {
        let d = duration.unwrap_or(self.duration).clamp(1, 15);
        self.provider
            .as_ref()
            .map(|p| p.estimate(d).usd())
            .unwrap_or(0.0)
    }

    pub fn extend_cost(&self) -> f64 {
        self.provider
            .as_ref()
            .map(|p| p.estimate(EXTEND_SECONDS).usd())
            .unwrap_or(0.0)
    }

    pub fn image_cost(&self) -> f64 {
        self.imager
            .as_ref()
            .map(|g| usage::image_estimate(&g.image_model).usd())
            .unwrap_or(0.0)
    }

    /// Stop button for the image job.
    pub fn stop_image(&mut self) -> Result<String, String> {
        if self.image_rx.take().is_none() {
            return Err("no image is generating".into());
        }
        self.image_cancel.store(true, Ordering::Relaxed);
        self.log.push(format!("image {}", STOPPED_NOTE));
        Ok(format!("image {}", STOPPED_NOTE))
    }

    /// Stop button for a clip (1-based).
    pub fn stop_clip(&mut self, clip: usize) -> Result<String, String> {
        let c = self
            .clips
            .get_mut(clip.saturating_sub(1))
            .filter(|c| c.rx.is_some())
            .ok_or_else(|| format!("clip {} isn't generating", clip))?;
        c.cancel.store(true, Ordering::Relaxed);
        c.rx = None;
        c.status = JobStatus::Failed(STOPPED_NOTE.into());
        Ok(format!("clip {} {}", clip, STOPPED_NOTE))
    }

    /// Stop every running Imagine job; returns how many were stopped.
    pub fn stop_all(&mut self) -> usize {
        let running: Vec<usize> = (1..=self.clips.len())
            .filter(|i| self.clips[i - 1].rx.is_some())
            .collect();
        let mut n = 0;
        for i in running {
            n += self.stop_clip(i).is_ok() as usize;
        }
        n + self.stop_image().is_ok() as usize
    }

    pub fn take_image(&mut self) -> Option<ImageResult> {
        let r = self.image_rx.as_ref()?.try_recv().ok()?;
        self.image_rx = None;
        Some(r)
    }

    fn spawn(&mut self, prompt: String, seconds: u32, start: StartJob) -> Result<String, String> {
        let p = self.provider.clone().ok_or(
            "Grok Imagine isn't configured: add an xAI API key in Settings (or set XAI_API_KEY)",
        )?;
        let (tx, rx) = channel();
        let idx = self.clips.len();
        let cancel = net::cancel_flag();
        let stop = cancel.clone();
        let every = self.poll_every;
        let spent = self.spent_tx.clone();
        std::thread::spawn(move || {
            let id = match start(p.as_ref(), &stop) {
                Ok(id) => {
                    // accepted: xAI bills it from here, Stop or not
                    let _ = spent.send(p.estimate(seconds));
                    id
                }
                Err(e) => {
                    let _ = tx.send((JobStatus::Failed(e), None));
                    return;
                }
            };
            let mut failures = 0;
            for _ in 0..POLLS {
                if !net::sleep_unless(&stop, every) {
                    return; // Stop: the UI already shows it
                }
                match p.poll(&id, &stop) {
                    Ok(JobStatus::Pending) => {
                        failures = 0;
                        continue;
                    }
                    Ok(JobStatus::Done { url, duration }) => {
                        let dir = videos_dir();
                        let path = dir.join(format!(
                            "clip_{}_{}.mp4",
                            idx + 1,
                            id.chars().take(8).collect::<String>()
                        ));
                        let a = net::agent(DOWNLOAD_TIMEOUT);
                        let saved = std::fs::create_dir_all(&dir)
                            .ok()
                            .and_then(|_| {
                                net::with_retry(
                                    &stop,
                                    net::Policy::Transient,
                                    || a.get(&url),
                                    None,
                                    &mut |_, _, _| {},
                                )
                                .ok()
                            })
                            .and_then(|r| {
                                let mut f = std::fs::File::create(&path).ok()?;
                                std::io::copy(&mut r.into_reader(), &mut f).ok()?;
                                Some(path.clone())
                            });
                        let _ = tx.send((JobStatus::Done { url, duration }, saved));
                        return;
                    }
                    Ok(s) => {
                        let _ = tx.send((s, None));
                        return;
                    }
                    Err(_) if stop.load(Ordering::Relaxed) => return,
                    // The poll already retried transient errors; give a
                    // flaky network a few more chances before failing.
                    Err(e) => {
                        failures += 1;
                        if failures >= POLL_FAILURES {
                            let _ = tx.send((JobStatus::Failed(e), None));
                            return;
                        }
                    }
                }
            }
            let _ = tx.send((
                JobStatus::Failed(format!(
                    "timed out after {} minutes",
                    (every * POLLS).as_secs() / 60
                )),
                None,
            ));
        });
        self.clips.push(Clip {
            prompt: prompt.clone(),
            status: JobStatus::Pending,
            local: None,
            selected: true,
            rx: Some(rx),
            cancel,
        });
        Ok(format!(
            "queued clip {}: “{}”",
            idx + 1,
            prompt.chars().take(60).collect::<String>()
        ))
    }

    pub fn generate(&mut self, prompt: &str, duration: Option<u32>) -> Result<String, String> {
        if prompt.trim().is_empty() {
            return Err("empty prompt".into());
        }
        let d = duration.unwrap_or(self.duration).clamp(1, 15);
        let pr = prompt.trim().to_string();
        let p2 = pr.clone();
        self.spawn(pr, d, Box::new(move |p, c| p.start_generate(&p2, d, c)))
    }

    pub fn extend(&mut self, clip: usize, prompt: &str) -> Result<String, String> {
        let url = match self.clips.get(clip.saturating_sub(1)).map(|c| &c.status) {
            Some(JobStatus::Done { url, .. }) => url.clone(),
            Some(_) => return Err(format!("clip {} isn't finished", clip)),
            None => return Err(format!("no clip {}", clip)),
        };
        let p2 = prompt.to_string();
        self.spawn(
            format!("extend #{}: {}", clip, prompt),
            EXTEND_SECONDS,
            Box::new(move |p, c| p.start_extend(&url, &p2, c)),
        )
    }

    /// Write a concat list (and run ffmpeg if available). Clip numbers are 1-based.
    pub fn stitch(&mut self, clips: Option<Vec<usize>>) -> Result<String, String> {
        let picks: Vec<usize> = clips.unwrap_or_else(|| {
            self.clips
                .iter()
                .enumerate()
                .filter(|(_, c)| c.selected)
                .map(|(i, _)| i + 1)
                .collect()
        });
        let files: Vec<PathBuf> = picks
            .iter()
            .filter_map(|i| {
                self.clips
                    .get(i.saturating_sub(1))
                    .and_then(|c| c.local.clone())
            })
            .collect();
        if files.len() < 2 {
            return Err("need at least 2 downloaded clips to stitch".into());
        }
        if self.stitch_rx.is_some() {
            return Err("already stitching; wait for it to finish".into());
        }
        let dir = videos_dir();
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let out = stitch_target(&dir, &crate::output::utc_stamp(secs));
        let list = out.with_extension("txt");
        let body: String = files
            .iter()
            .map(|f| {
                format!(
                    "file '{}'\n",
                    f.display().to_string().replace('\'', "'\\''")
                )
            })
            .collect();
        std::fs::write(&list, body).map_err(|e| e.to_string())?;
        let n = files.len();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(run_concat(&list, &out, n));
        });
        self.stitch_rx = Some(rx);
        Ok(format!("stitching {} clips…", n))
    }

    pub fn stitch_busy(&self) -> bool {
        self.stitch_rx.is_some()
    }

    /// The finished stitch, once (for a toast).
    pub fn take_stitch(&mut self) -> Option<Result<String, String>> {
        let r = self.stitch_rx.as_ref()?.try_recv();
        match r {
            Ok(res) => {
                self.stitch_rx = None;
                self.log.push(match &res {
                    Ok(m) | Err(m) => m.clone(),
                });
                Some(res)
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.stitch_rx = None;
                Some(Err("stitching stopped unexpectedly".into()))
            }
        }
    }

    /// Collect costs reported by workers into `spent`.
    pub fn tick_spent(&mut self) {
        self.spent.extend(self.spent_rx.try_iter());
    }

    pub fn tick(&mut self) -> bool {
        self.tick_spent();
        let mut busy = false;
        for c in &mut self.clips {
            if let Some(rx) = &c.rx {
                busy = true;
                if let Ok((s, local)) = rx.try_recv() {
                    c.status = s;
                    c.local = local;
                    c.rx = None;
                }
            }
        }
        busy || self.stitch_rx.is_some()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        let mut stop_image = false;
        let mut stop_clip = None;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(ic::FILM_SLATE)
                    .size(16.0)
                    .color(p.accent),
            );
            ui.label(
                egui::RichText::new("Grok Imagine")
                    .size(14.0)
                    .strong()
                    .color(p.text),
            );
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match &self.provider {
                    Some(pr) => {
                        theme::pill(ui, p, p.success, "Configured").on_hover_text(pr.name());
                    }
                    None => {
                        theme::pill(ui, p, p.warn, "Not configured")
                            .on_hover_text("Add an xAI API key in Settings (or set XAI_API_KEY)");
                    }
                },
            );
        });
        ui.label(theme::muted(p, "Text-to-video and text-to-image via the xAI Imagine API (billed per second / per image). Clips are saved to ~/.seraphina/studio/videos."));
        ui.add_space(SP_SM);
        ui.add(
            egui::TextEdit::multiline(&mut self.prompt)
                .hint_text("Describe a shot: “slow aerial push over the golden valley at dusk”")
                .desired_rows(3)
                .desired_width(f32::INFINITY),
        );
        ui.horizontal(|ui| {
            ui.label(theme::muted(p, "Duration"));
            ui.add(
                egui::DragValue::new(&mut self.duration)
                    .range(1..=15)
                    .suffix(" s"),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let ok = self.provider.is_some() && !self.prompt.trim().is_empty();
                if ui
                    .add_enabled(ok, theme::primary_button(p, "Generate video"))
                    .on_disabled_hover_text("Needs XAI_API_KEY and a prompt")
                    .clicked()
                {
                    bus.push(AppCommand::VideoGenerate {
                        prompt: self.prompt.clone(),
                        duration: Some(self.duration),
                    });
                }
                let label = if self.image_rx.is_some() {
                    "Imaging…"
                } else {
                    "Image → layer"
                };
                if self.image_rx.is_some()
                    && ui
                        .add(theme::stop_button(p, &format!("{} Stop", ic::STOP)))
                        .on_hover_text("Stop waiting for this image")
                        .clicked()
                {
                    stop_image = true;
                }
                if ui
                    .add_enabled(
                        ok && self.image_rx.is_none(),
                        theme::toggle_button(p, label, false),
                    )
                    .on_hover_text(
                        "Generate a still with Grok Imagine and add it to Paint as a new layer",
                    )
                    .clicked()
                {
                    bus.push(AppCommand::GenerateImage {
                        prompt: self.prompt.clone(),
                    });
                }
            });
        });
        ui.add_space(SP_SM);
        ui.label(theme::section(p, &format!("Clips · {}", self.clips.len())));
        if self.clips.is_empty() {
            ui.label(theme::muted(p, "No clips yet."));
        }
        let mut open = None;
        for (i, c) in self.clips.iter_mut().enumerate() {
            theme::card(ui, p, "", |ui| {
                ui.horizontal(|ui| {
                    ui.checkbox(&mut c.selected, "");
                    ui.label(
                        egui::RichText::new(format!("#{}", i + 1))
                            .strong()
                            .color(p.accent),
                    );
                    let (col, txt) = match &c.status {
                        JobStatus::Pending => (p.warn, "Generating…".to_string()),
                        JobStatus::Done { duration, .. } => (
                            p.success,
                            format!(
                                "Done · {:.0}s{}",
                                duration,
                                if c.local.is_some() { " · saved" } else { "" }
                            ),
                        ),
                        JobStatus::Failed(e) => (
                            p.danger,
                            format!("Failed: {}", e.chars().take(60).collect::<String>()),
                        ),
                    };
                    theme::pill(ui, p, col, &txt);
                    if c.rx.is_some()
                        && ui
                            .add(theme::stop_button(p, &format!("{} Stop", ic::STOP)))
                            .on_hover_text("Stop waiting for this clip")
                            .clicked()
                    {
                        stop_clip = Some(i + 1);
                    }
                });
                ui.add(egui::Label::new(theme::muted(p, &c.prompt)).wrap());
                if let JobStatus::Done { url, .. } = &c.status {
                    if ui
                        .add(theme::toggle_button(p, "Open in browser", false))
                        .clicked()
                    {
                        open = Some(url.clone());
                    }
                }
            });
        }
        if let Some(u) = open {
            ui.ctx().open_url(egui::OpenUrl::new_tab(u));
        }
        if stop_image {
            let _ = self.stop_image();
        }
        if let Some(i) = stop_clip {
            let _ = self.stop_clip(i);
        }
        let done: Vec<usize> = self
            .clips
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c.status, JobStatus::Done { .. }))
            .map(|(i, _)| i + 1)
            .collect();
        if !done.is_empty() {
            ui.add_space(SP_SM);
            ui.add(
                egui::TextEdit::singleline(&mut self.extend_prompt)
                    .hint_text("Extension prompt for the last finished clip")
                    .desired_width(f32::INFINITY),
            );
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.extend_prompt.trim().is_empty(),
                        theme::toggle_button(
                            p,
                            &format!("Extend #{}", done[done.len() - 1]),
                            false,
                        ),
                    )
                    .clicked()
                {
                    bus.push(AppCommand::VideoExtend {
                        clip: done[done.len() - 1],
                        prompt: self.extend_prompt.clone(),
                    });
                }
                let busy = self.stitch_rx.is_some();
                let label = if busy {
                    "Stitching…"
                } else {
                    "Stitch selected"
                };
                if ui
                    .add_enabled(!busy, theme::toggle_button(p, label, busy))
                    .clicked()
                {
                    bus.push(AppCommand::VideoStitch { clips: None });
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parsing() {
        assert_eq!(
            parse_status(&json!({"status":"pending"})),
            JobStatus::Pending
        );
        assert_eq!(
            parse_status(&json!({"status":"done","video":{"url":"https://v/x.mp4","duration":8}})),
            JobStatus::Done {
                url: "https://v/x.mp4".into(),
                duration: 8.0
            }
        );
        assert!(matches!(
            parse_status(&json!({"status":"failed"})),
            JobStatus::Failed(_)
        ));
        assert!(matches!(
            parse_status(&json!({"status":"done"})),
            JobStatus::Failed(_)
        ));
        assert_eq!(
            parse_status(
                &json!({"status":"failed","error":{"code":"internal_error","message":"boom"}})
            ),
            JobStatus::Failed("boom".into())
        );
        assert_eq!(
            parse_status(&json!({"status":"done","video":{"url":null,"respect_moderation":false}})),
            JobStatus::Failed("blocked by content moderation".into())
        );
        assert_eq!(
            parse_status(&json!({"status":"pending","progress":40})),
            JobStatus::Pending
        );
    }

    /// A provider whose polls follow a script (Err = network failure).
    struct Scripted {
        polls: std::sync::Mutex<Vec<Result<JobStatus, String>>>,
        count: std::sync::atomic::AtomicUsize,
    }

    impl VideoGenProvider for Scripted {
        fn name(&self) -> String {
            "scripted".into()
        }
        fn start_generate(&self, _: &str, _: u32, _: &AtomicBool) -> Result<String, String> {
            Ok("job".into())
        }
        fn start_extend(&self, _: &str, _: &str, _: &AtomicBool) -> Result<String, String> {
            Ok("job".into())
        }
        fn poll(&self, _: &str, _: &AtomicBool) -> Result<JobStatus, String> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.polls
                .lock()
                .unwrap()
                .pop()
                .unwrap_or(Ok(JobStatus::Pending))
        }
    }

    fn scripted(
        mut polls: Vec<Result<JobStatus, String>>,
    ) -> (VideoGenPanel, std::sync::Arc<Scripted>) {
        polls.reverse();
        let s = std::sync::Arc::new(Scripted {
            polls: std::sync::Mutex::new(polls),
            count: Default::default(),
        });
        let mut v = VideoGenPanel::unconfigured();
        v.provider = Some(s.clone());
        v.poll_every = Duration::from_millis(5);
        (v, s)
    }

    fn settle(v: &mut VideoGenPanel) -> JobStatus {
        for _ in 0..400 {
            v.tick();
            if v.clips[0].status != JobStatus::Pending {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        v.clips[0].status.clone()
    }

    #[test]
    fn imagine_refuses_plain_http_to_another_host() {
        let g = GrokImagine {
            base: "http://192.0.2.1/v1".into(),
            key: "xai-NOT-A-REAL-KEY-0123456789".into(),
            model: "m".into(),
            resolution: "480p".into(),
            image_model: "i".into(),
        };
        let cancel = AtomicBool::new(false);
        let e = g.start_generate("a cat", 1, &cancel).unwrap_err();
        assert!(e.contains("unencrypted") && !e.contains("xai-"), "{e}");
        assert!(g.poll("id", &cancel).unwrap_err().contains("unencrypted"));
    }

    #[test]
    fn stop_ends_a_clip_and_its_polling() {
        let (mut v, s) = scripted(vec![]);
        v.generate("a cat", Some(1)).unwrap();
        std::thread::sleep(Duration::from_millis(60));
        assert!(s.count.load(Ordering::SeqCst) > 0);
        assert!(v.stop_clip(1).unwrap().contains("may still"));
        assert!(v.stop_clip(1).is_err(), "already stopped");
        assert_eq!(v.clips[0].status, JobStatus::Failed(STOPPED_NOTE.into()));
        std::thread::sleep(Duration::from_millis(40));
        let n = s.count.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(s.count.load(Ordering::SeqCst), n, "polling stopped");
        v.tick();
        assert_eq!(v.clips[0].status, JobStatus::Failed(STOPPED_NOTE.into()));
        assert_eq!(v.stop_all(), 0);
    }

    #[test]
    fn a_flaky_poll_is_tolerated_but_a_dead_one_fails() {
        let e = || Err::<JobStatus, String>("connection reset".into());
        let (mut v, _) = scripted(vec![
            e(),
            e(),
            Ok(JobStatus::Pending),
            Ok(JobStatus::Failed("moderated".into())),
        ]);
        v.generate("a cat", Some(1)).unwrap();
        assert_eq!(settle(&mut v), JobStatus::Failed("moderated".into()));
        let (mut v, _) = scripted(vec![e(), e(), e(), Ok(JobStatus::Pending)]);
        v.generate("a cat", Some(1)).unwrap();
        assert_eq!(settle(&mut v), JobStatus::Failed("connection reset".into()));
    }

    #[test]
    fn stopping_an_image_frees_the_button() {
        let mut v = VideoGenPanel::unconfigured();
        assert!(v.stop_image().is_err());
        let (tx, rx) = channel();
        v.image_rx = Some(rx);
        assert!(v.stop_image().is_ok());
        assert!(v.image_cancel.load(Ordering::Relaxed));
        let _ = tx.send(Ok(("late".into(), vec![1])));
        assert!(v.take_image().is_none(), "a late image is dropped");
    }

    #[test]
    fn base64_roundtrip() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("data:image/png;base64,aGk=").unwrap(), b"hi");
        assert!(b64_decode("@@@@").is_none());
    }

    #[test]
    fn unconfigured_is_a_clear_error() {
        let mut v = VideoGenPanel::unconfigured();
        assert!(v
            .generate_image("a cat")
            .unwrap_err()
            .contains("XAI_API_KEY"));
        assert!(v
            .generate("a cat", None)
            .unwrap_err()
            .contains("XAI_API_KEY"));
        assert!(v.stitch(None).is_err());
        assert!(v.extend(1, "more").is_err());
    }

    #[test]
    fn concat_runs_off_thread_when_ffmpeg_exists() {
        let Ok(ff) = crate::output::find_ffmpeg() else {
            return;
        };
        let dir = std::env::temp_dir().join(format!("seraphina-concat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut body = String::new();
        for i in 0..2 {
            let f = dir.join(format!("c{}.mp4", i));
            let ok = std::process::Command::new(&ff)
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg("testsrc=size=64x64:rate=10:duration=0.5")
                .args(["-pix_fmt", "yuv420p"])
                .arg(&f)
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                return;
            }
            body.push_str(&format!("file '{}'\n", f.display()));
        }
        let out = stitch_target(&dir, "t");
        let list = out.with_extension("txt");
        std::fs::write(&list, body).unwrap();
        let (o2, l2) = (out.clone(), list.clone());
        let r = std::thread::spawn(move || run_concat(&l2, &o2, 2))
            .join()
            .unwrap();
        assert!(
            r.as_deref().unwrap_or("").contains("stitched 2 clips"),
            "{r:?}"
        );
        assert!(out.exists() && !list.exists());
        // A second run never overwrites.
        assert_ne!(stitch_target(&dir, "t"), out);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stitch_names_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("seraphina-stitch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let a = stitch_target(&dir, "2026-10-24_00-00-00Z");
        assert!(a.ends_with("stitched_2026-10-24_00-00-00Z.mp4"));
        std::fs::write(&a, b"x").unwrap();
        let b = stitch_target(&dir, "2026-10-24_00-00-00Z");
        assert!(b.ends_with("stitched_2026-10-24_00-00-00Z_2.mp4"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Live, billed: run explicitly with `cargo test --features ui --bin ui -- --ignored live_imagine`.
    /// Writes into SERAPHINA_LIVE_OUT (default: temp dir).
    #[test]
    #[ignore]
    fn live_imagine_video_and_image() {
        let Some(g) = GrokImagine::from_env() else {
            eprintln!("SKIPPED live_imagine_video_and_image: no XAI_API_KEY");
            return;
        };
        let out = std::env::var("SERAPHINA_LIVE_OUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::temp_dir());
        std::fs::create_dir_all(&out).unwrap();
        let t0 = std::time::Instant::now();
        let img = g
            .generate_image(
                "A gold gem logo on a near-black background, minimal, crisp",
                &AtomicBool::new(false),
            )
            .expect("image")
            .0;
        let im = image::load_from_memory(&img).expect("decodes");
        std::fs::write(out.join("live_grok_imagine_image.jpg"), &img).unwrap();
        eprintln!(
            "LIVE image: {} bytes, {}x{}, {:.1}s",
            img.len(),
            im.width(),
            im.height(),
            t0.elapsed().as_secs_f32()
        );
        let t0 = std::time::Instant::now();
        let id = g
            .start_generate(
                "Slow golden light sweeping across misty mountain peaks at dusk",
                1,
                &AtomicBool::new(false),
            )
            .expect("start");
        eprintln!(
            "LIVE video request accepted ({} @ {})",
            g.model, g.resolution
        );
        let mut status = JobStatus::Pending;
        for _ in 0..120 {
            std::thread::sleep(std::time::Duration::from_secs(5));
            status = g.poll(&id, &AtomicBool::new(false)).expect("poll");
            if status != JobStatus::Pending {
                break;
            }
        }
        match status {
            JobStatus::Done { url, duration } => {
                let r = ureq::get(&url).call().expect("download");
                let mut buf = Vec::new();
                std::io::Read::read_to_end(&mut r.into_reader(), &mut buf).unwrap();
                std::fs::write(out.join("live_grok_imagine_clip.mp4"), &buf).unwrap();
                eprintln!(
                    "LIVE video: {:.0}s clip, {} bytes, {:.0}s wall",
                    duration,
                    buf.len(),
                    t0.elapsed().as_secs_f32()
                );
                assert!(buf.len() > 1000);
            }
            other => panic!("video did not finish: {:?}", other),
        }
    }
}

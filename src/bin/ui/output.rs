//! Real recording and live streaming through an `ffmpeg` subprocess.
//!
//! * Canvas source: the active mode's composite is letterboxed to the output
//!   size and piped as raw RGBA frames to ffmpeg's stdin at a fixed rate. A
//!   tick that cannot be written in time is counted as a dropped frame.
//! * Screen source: ffmpeg grabs the desktop itself (x11grab; on Windows
//!   ddagrab, the DPI-correct Desktop Duplication grabber, with gdigrab as
//!   the fallback).
//! * Camera source: a webcam read by `crate::camera` (V4L2 on Linux, Media
//!   Foundation on Windows) is piped the same way as the canvas. A camera
//!   that stops sending ends the recording / stream with the reason.
//! * Encode: libx264 (veryfast, zerolatency, yuv420p) + AAC (silent track or a
//!   capture device). Record writes MP4, live pushes FLV to RTMP.
//! * `-progress pipe:1` is parsed for fps / bitrate / encoded time / drops.
//! * Stream delay (Settings ▸ Output, 0 = off): the encoder writes FLV to
//!   its stdout instead of RTMP (progress moves to stderr). Its tags are
//!   split by `obs_rust_bitstream::encoder_packet`, held in an
//!   `OutputDelayQueue` for the delay, and written to a second ffmpeg that
//!   only copies them to the RTMP server. Stopping ends the stream at once:
//!   the delayed tail is dropped, as in OBS. If either process dies, the
//!   other is killed and the stream reconnects as a whole.
//! * Live sessions that die are restarted with exponential backoff
//!   (`obs_rust_bitstream::Reconnect`).
//!
//! The stream key lives in its own file (0600 on Unix), is never put in the
//! AI context, logs or error text (ffmpeg output is scrubbed), and the UI only
//! shows it masked unless the user asks to reveal it.

use crate::prefs::seraphina_home;
use crate::ToastKind;
use obs_rust_bitstream::encoder_packet::{FlvItem, FlvReader};
use obs_rust_bitstream::output_delay_queue::OutputDelayQueue;
use obs_rust_bitstream::ui_canvas::Raster;
use obs_rust_bitstream::Reconnect;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Canvas,
    Screen,
    /// A webcam (V4L2 on Linux, Media Foundation on Windows).
    Camera,
}

impl Source {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "canvas" | "composite" | "app" => Some(Source::Canvas),
            "screen" | "desktop" | "display" => Some(Source::Screen),
            "camera" | "webcam" => Some(Source::Camera),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Source::Canvas => "Canvas",
            Source::Screen => "Screen",
            Source::Camera => "Camera",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Audio {
    None,
    Silent,
    Device,
}

impl Audio {
    pub fn label(self) -> &'static str {
        match self {
            Audio::None => "No audio",
            Audio::Silent => "Silent AAC track",
            Audio::Device => "Capture device",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct OutputConfig {
    /// e.g. `rtmp://live.twitch.tv/app` (the key is stored separately).
    pub rtmp_url: String,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub video_kbps: u32,
    pub audio_kbps: u32,
    pub audio: Audio,
    /// PulseAudio source (Linux) or DirectShow device name (Windows).
    pub audio_device: String,
    pub source: Source,
    /// Camera for the Camera source: its name or device path; empty means
    /// the first camera found.
    pub camera_device: String,
    pub record_dir: String,
    pub retry_sec: u32,
    pub retry_max: u32,
    /// Live only: viewers see the stream this many seconds late (0 = off,
    /// at most `MAX_DELAY_SEC`).
    pub stream_delay_sec: u32,
}

/// The delay is held in memory: 300 s at 4.5 Mb/s is about 170 MB.
pub const MAX_DELAY_SEC: u32 = 300;

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            rtmp_url: String::new(),
            width: 1280,
            height: 720,
            fps: 30,
            video_kbps: 4500,
            audio_kbps: 160,
            audio: Audio::Silent,
            audio_device: String::new(),
            source: Source::Canvas,
            camera_device: String::new(),
            record_dir: studio_dir()
                .join("recordings")
                .to_string_lossy()
                .into_owned(),
            retry_sec: 2,
            retry_max: 10,
            stream_delay_sec: 0,
        }
    }
}

pub fn studio_dir() -> PathBuf {
    seraphina_home().join("studio")
}

impl OutputConfig {
    /// Settings and, if output.json was damaged, a message saying where
    /// the damaged copy was kept.
    pub fn load_from(path: &std::path::Path) -> (Self, Option<String>) {
        crate::project::load_json(path, "output settings file")
    }
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::project::write_atomic(path, s.as_bytes()).map_err(|e| e.to_string())
    }
    /// Even dimensions (yuv420p), sane bounds.
    /// The stream delay for a session of this kind, if one applies.
    pub fn delay(&self, kind: Kind) -> Option<Duration> {
        let d = self.stream_delay_sec.min(MAX_DELAY_SEC);
        (kind == Kind::Live && d > 0).then(|| Duration::from_secs(d as u64))
    }

    pub fn size(&self) -> (u32, u32) {
        let e = |v: u32| (v.clamp(64, 3840) / 2) * 2;
        (e(self.width), e(self.height))
    }
}

/// The saved stream key (Credential Manager on Windows, a 0600 file
/// elsewhere; see `secrets`). Unit tests never read the user's store.
pub fn load_key() -> String {
    if cfg!(test) {
        return String::new();
    }
    crate::secrets::Store::user().load(crate::secrets::Secret::StreamKey)
}

pub fn save_key(key: &str) -> Result<crate::secrets::Where, String> {
    crate::secrets::Store::user().save(crate::secrets::Secret::StreamKey, key)
}

pub fn key_location() -> crate::secrets::Where {
    crate::secrets::Store::user().location(crate::secrets::Secret::StreamKey)
}

/// What the UI shows instead of the key.
pub fn mask(key: &str) -> String {
    if key.is_empty() {
        "not set".into()
    } else {
        format!("{} ({} characters)", "•".repeat(8), key.chars().count())
    }
}

/// Keep a console window from flashing up for a child process on Windows.
pub fn no_window(_cmd: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        _cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
}

const EXE: &str = if cfg!(windows) {
    "ffmpeg.exe"
} else {
    "ffmpeg"
};

pub const FFMPEG_HELP: &str = "ffmpeg was not found. Put ffmpeg next to SeraphinaStudio (ffmpeg.exe in the same folder on Windows), add it to PATH, or set SERAPHINA_FFMPEG to its full path. Windows builds: https://www.gyan.dev/ffmpeg/builds/ or https://github.com/BtbN/FFmpeg-Builds/releases ; Linux: your package manager (apt install ffmpeg).";

/// Candidate locations, in priority order: `SERAPHINA_FFMPEG`, next to the
/// executable (and `./ffmpeg/bin/`), then every `PATH` entry.
pub fn ffmpeg_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("SERAPHINA_FFMPEG") {
        if !p.trim().is_empty() {
            v.push(PathBuf::from(p.trim()));
        }
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|e| e.parent().map(Path::to_path_buf))
    {
        v.push(dir.join(EXE));
        v.push(dir.join("ffmpeg").join("bin").join(EXE));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            v.push(d.join(EXE));
        }
    }
    v
}

/// Find a working ffmpeg (it must answer `-version`).
pub fn find_ffmpeg() -> Result<PathBuf, String> {
    find_ffmpeg_in(ffmpeg_candidates())
}

pub fn find_ffmpeg_in(candidates: Vec<PathBuf>) -> Result<PathBuf, String> {
    for c in candidates {
        if !c.is_file() {
            continue;
        }
        let mut cmd = Command::new(&c);
        cmd.arg("-version")
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        no_window(&mut cmd);
        if let Ok(o) = cmd.output() {
            if o.status.success()
                && String::from_utf8_lossy(&o.stdout).starts_with("ffmpeg version")
            {
                return Ok(c);
            }
        }
    }
    Err(FFMPEG_HELP.into())
}

/// `ffprobe` sitting next to a given ffmpeg (falls back to PATH lookup).
#[cfg(test)]
pub fn ffprobe_near(ffmpeg: &Path) -> PathBuf {
    let name = if cfg!(windows) {
        "ffprobe.exe"
    } else {
        "ffprobe"
    };
    let p = ffmpeg.with_file_name(name);
    if p.is_file() {
        p
    } else {
        PathBuf::from(name)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Record,
    Live,
}

/// Join an RTMP base URL and a stream key.
pub fn rtmp_target(url: &str, key: &str) -> String {
    let u = url.trim().trim_end_matches('/');
    if key.trim().is_empty() {
        u.to_string()
    } else {
        format!("{}/{}", u, key.trim())
    }
}

/// How the Screen source captures the desktop.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Grab {
    /// Windows Desktop Duplication (ffmpeg >= 6, d3d11): native pixels at
    /// any DPI scale, GPU copy, primary monitor.
    Ddagrab,
    /// Windows GDI: works everywhere, slower, whole virtual desktop.
    Gdigrab,
    X11,
}

/// Set once a ddagrab session dies before its first frame (Remote
/// Desktop, old driver, no D3D11): later sessions use gdigrab.
static DDA_BROKEN: AtomicBool = AtomicBool::new(false);
static DDA_PROBE: Mutex<Option<(PathBuf, bool)>> = Mutex::new(None);

/// Whether this ffmpeg build has the ddagrab filter (cached per path).
fn has_ddagrab(ffmpeg: &Path) -> bool {
    if let Ok(g) = DDA_PROBE.lock() {
        if let Some((p, ok)) = g.as_ref() {
            if p == ffmpeg {
                return *ok;
            }
        }
    }
    let mut cmd = Command::new(ffmpeg);
    cmd.args(["-hide_banner", "-filters"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    no_window(&mut cmd);
    let ok = cmd
        .output()
        .map(|o| filters_have_ddagrab(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false);
    if let Ok(mut g) = DDA_PROBE.lock() {
        *g = Some((ffmpeg.to_path_buf(), ok));
    }
    ok
}

/// `ffmpeg -filters` lines look like ` ... ddagrab  |->V  Grab Windows Desktop...`.
pub fn filters_have_ddagrab(list: &str) -> bool {
    list.lines()
        .any(|l| l.split_whitespace().nth(1) == Some("ddagrab"))
}

/// Pick the grabber. `SERAPHINA_SCREEN_GRAB=gdigrab|ddagrab` overrides.
pub fn choose_grab(windows: bool, env: Option<&str>, has_dda: bool, dda_broken: bool) -> Grab {
    if !windows {
        return Grab::X11;
    }
    match env.map(|e| e.trim().to_ascii_lowercase()).as_deref() {
        Some("gdigrab" | "gdi") => Grab::Gdigrab,
        Some("ddagrab" | "dda") => Grab::Ddagrab,
        _ if has_dda && !dda_broken => Grab::Ddagrab,
        _ => Grab::Gdigrab,
    }
}

pub fn screen_grab(ffmpeg: &Path) -> Grab {
    let env = std::env::var("SERAPHINA_SCREEN_GRAB").ok();
    let broken = DDA_BROKEN.load(Ordering::SeqCst);
    // only probe on Windows: elsewhere it's always x11grab
    let has = cfg!(windows) && env.is_none() && !broken && has_ddagrab(ffmpeg);
    choose_grab(cfg!(windows), env.as_deref(), has, broken)
}

/// Full ffmpeg argument list for a session.
/// Which capture API "Capture device" audio goes through on this OS.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AudioBackend {
    /// Linux: PulseAudio (or PipeWire's pulse server); empty name = `default`.
    Pulse,
    /// Windows: DirectShow; there is no default device, a name is required.
    DirectShow,
    /// macOS and others: not implemented; refused with a clear error.
    Unsupported,
}

impl AudioBackend {
    pub fn host() -> Self {
        if cfg!(windows) {
            AudioBackend::DirectShow
        } else if cfg!(target_os = "linux") {
            AudioBackend::Pulse
        } else {
            AudioBackend::Unsupported
        }
    }
}

/// ffmpeg arguments for one session. Errors when the audio settings can't
/// work (no DirectShow device named, or an OS without capture support):
/// the app never swaps in a synthetic or silent track. A device that exists
/// by name but can't be opened makes ffmpeg exit, which surfaces as an
/// error toast.
pub fn build_args(
    cfg: &OutputConfig,
    kind: Kind,
    target: &str,
    grab: Grab,
    backend: AudioBackend,
) -> Result<Vec<String>, String> {
    let (w, h) = cfg.size();
    let fps = cfg.fps.clamp(1, 120);
    // with a stream delay stdout carries the FLV, so progress goes to stderr
    let delayed = cfg.delay(kind).is_some();
    let mut a: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "warning",
        "-nostats",
        "-progress",
        if delayed { "pipe:2" } else { "pipe:1" },
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match cfg.source {
        // camera frames are piped like the canvas, at the output fps
        Source::Canvas | Source::Camera => {
            if cfg.audio == Audio::Device {
                // A capture device stamps audio with wall-clock time; stamp the
                // piped frames the same way, or ffmpeg can't line the two up
                // and the file stops after a fraction of a second.
                push(&["-use_wallclock_as_timestamps", "1"]);
            }
            push(&[
                "-f",
                "rawvideo",
                "-pix_fmt",
                "rgba",
                "-s",
                &format!("{}x{}", w, h),
                "-framerate",
                &fps.to_string(),
                "-i",
                "pipe:0",
            ]);
        }
        Source::Screen => match grab {
            Grab::Ddagrab => push(&[
                "-f",
                "lavfi",
                "-i",
                &format!("ddagrab=output_idx=0:framerate={}:draw_mouse=1", fps),
            ]),
            Grab::Gdigrab => push(&[
                "-f",
                "gdigrab",
                "-framerate",
                &fps.to_string(),
                "-draw_mouse",
                "1",
                "-i",
                "desktop",
            ]),
            Grab::X11 => {
                let display = std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into());
                push(&[
                    "-f",
                    "x11grab",
                    "-framerate",
                    &fps.to_string(),
                    "-draw_mouse",
                    "1",
                    "-i",
                    &display,
                ]);
            }
        },
    }
    let has_audio = cfg.audio != Audio::None;
    match cfg.audio {
        Audio::None => {}
        Audio::Silent => push(&[
            "-f",
            "lavfi",
            "-i",
            "anullsrc=channel_layout=stereo:sample_rate=48000",
        ]),
        Audio::Device => match backend {
            AudioBackend::DirectShow => {
                let dev = cfg.audio_device.trim();
                if dev.is_empty() {
                    return Err("Audio is set to Capture device but no DirectShow device is named. Enter it in Settings ▸ Output (e.g. \"Microphone (Realtek Audio)\"; `ffmpeg -list_devices true -f dshow -i dummy` lists them), or pick Silent AAC track / No audio.".into());
                }
                push(&["-f", "dshow", "-i", &format!("audio={dev}")]);
            }
            AudioBackend::Pulse => {
                let dev = if cfg.audio_device.trim().is_empty() {
                    "default"
                } else {
                    cfg.audio_device.trim()
                };
                push(&["-f", "pulse", "-i", dev]);
            }
            AudioBackend::Unsupported => {
                return Err("Capture-device audio isn't supported on this OS yet (Windows and Linux only). Pick Silent AAC track or No audio in Settings ▸ Output.".into());
            }
        },
    }
    push(&["-map", "0:v"]);
    if has_audio {
        push(&["-map", "1:a"]);
    }
    if cfg.source == Source::Screen {
        // ddagrab hands over D3D11 GPU frames; bring them to system memory first
        let pre = if grab == Grab::Ddagrab {
            "hwdownload,format=bgra,"
        } else {
            ""
        };
        push(&["-vf", &format!("{pre}scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2", pre = pre, w = w, h = h)]);
    }
    let kb = cfg.video_kbps.clamp(200, 50_000);
    push(&[
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-tune",
        "zerolatency",
        "-pix_fmt",
        "yuv420p",
        "-b:v",
        &format!("{}k", kb),
        "-maxrate",
        &format!("{}k", kb),
        "-bufsize",
        &format!("{}k", kb * 2),
        "-g",
        &(fps * 2).to_string(),
    ]);
    if has_audio {
        push(&[
            "-c:a",
            "aac",
            "-b:a",
            &format!("{}k", cfg.audio_kbps.clamp(32, 512)),
            "-ar",
            "48000",
        ]);
        if cfg.source != Source::Screen {
            push(&["-shortest"]);
        }
    }
    match kind {
        Kind::Record => push(&["-movflags", "+faststart", "-f", "mp4", "-y", target]),
        Kind::Live => push(&["-f", "flv", if delayed { "pipe:1" } else { target }]),
    }
    Ok(a)
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Frames written into ffmpeg's stdin.
    pub frames_in: u64,
    /// Ticks filled with the previous frame again because the pipe or
    /// encoder was behind (no tick is skipped: ffmpeg stamps piped frames by
    /// count, so a skipped tick would make the file shorter than real time).
    pub repeated: u64,
    /// Wall-clock seconds from the writer's first tick to the stop request.
    pub span: f64,
    /// ffmpeg's own counters.
    pub enc_frames: u64,
    pub ffmpeg_drops: u64,
    pub fps: f32,
    pub kbps: f32,
    pub out_time: f64,
    pub total_bytes: u64,
    /// Stream delay only: the stream time (s) of the last packet sent on to
    /// the RTMP server, i.e. where viewers are.
    pub sent_time: f64,
    /// Stream delay only: the stream time (s) of the newest packet the
    /// encoder handed to the delay queue.
    pub queued_time: f64,
    pub last_error: Option<String>,
    pub ended: bool,
}

fn parse_progress(line: &str, s: &mut Stats) {
    let Some((k, v)) = line.split_once('=') else {
        return;
    };
    let v = v.trim();
    match k.trim() {
        "frame" => s.enc_frames = v.parse().unwrap_or(s.enc_frames),
        "fps" => s.fps = v.parse().unwrap_or(s.fps),
        "bitrate" => {
            s.kbps = v
                .trim_end_matches("kbits/s")
                .trim()
                .parse()
                .unwrap_or(s.kbps)
        }
        "drop_frames" => s.ffmpeg_drops = v.parse().unwrap_or(s.ffmpeg_drops),
        "total_size" => s.total_bytes = v.parse().unwrap_or(s.total_bytes),
        "out_time_us" | "out_time_ms" => {
            if let Ok(us) = v.parse::<i64>() {
                s.out_time = us.max(0) as f64 / 1e6;
            }
        }
        "progress" if v == "end" => s.ended = true,
        _ => {}
    }
}

/// Scale `src` into a `w`×`h` black frame (aspect kept, nearest sampling,
/// alpha composited over black).
pub fn letterbox(src: Option<&Raster>, w: u32, h: u32) -> Vec<u8> {
    let mut out = vec![0u8; (w * h * 4) as usize];
    for px in out.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    let Some(r) = src.filter(|r| r.width > 0 && r.height > 0) else {
        return out;
    };
    let s = (w as f32 / r.width as f32).min(h as f32 / r.height as f32);
    let (dw, dh) = (
        ((r.width as f32 * s) as u32).clamp(1, w),
        ((r.height as f32 * s) as u32).clamp(1, h),
    );
    let (ox, oy) = ((w - dw) / 2, (h - dh) / 2);
    let xs: Vec<usize> = (0..dw)
        .map(|x| ((x as u64 * r.width as u64 / dw as u64) as usize).min(r.width as usize - 1))
        .collect();
    for y in 0..dh {
        let sy = ((y as u64 * r.height as u64 / dh as u64) as usize).min(r.height as usize - 1);
        let srow = &r.data[sy * r.width as usize * 4..(sy + 1) * r.width as usize * 4];
        let drow =
            &mut out[((oy + y) * w + ox) as usize * 4..((oy + y) * w + ox + dw) as usize * 4];
        for (d, &sx) in drow.as_chunks_mut::<4>().0.iter_mut().zip(&xs) {
            let p = &srow[sx * 4..sx * 4 + 4];
            let a = p[3] as u16;
            d[0] = (p[0] as u16 * a / 255) as u8;
            d[1] = (p[1] as u16 * a / 255) as u8;
            d[2] = (p[2] as u16 * a / 255) as u8;
        }
    }
    out
}

pub type FrameSlot = Arc<Mutex<Option<Arc<Raster>>>>;

/// One running ffmpeg process.
pub struct Session {
    pub kind: Kind,
    /// File path, or the RTMP URL with the key masked.
    pub target_label: String,
    pub path: Option<PathBuf>,
    pub started: Instant,
    child: Child,
    /// Stream delay: the ffmpeg that copies the delayed FLV to RTMP, and the
    /// thread that feeds it from the delay queue.
    relay: Option<(Child, JoinHandle<()>)>,
    /// The stream delay this session runs with.
    pub delay: Option<Duration>,
    stats: Arc<Mutex<Stats>>,
    stop: Arc<AtomicBool>,
    writer: Option<JoinHandle<()>>,
    finished: bool,
    grab: Grab,
}

/// `rtmp://user:pass@host/app/KEY?token=x` → `rtmp://host/app`.
pub fn strip_url_secrets(url: &str, key: &str) -> String {
    let mut u = url.trim().to_string();
    if let Some(q) = u.find(['?', '#']) {
        u.truncate(q);
    }
    if let Some(s) = u.find("://") {
        let rest = &u[s + 3..];
        let host_end = rest.find('/').unwrap_or(rest.len());
        if let Some(at) = rest[..host_end].rfind('@') {
            u = format!("{}{}", &u[..s + 3], &rest[at + 1..]);
        }
    }
    let k = key.trim();
    if !k.is_empty() {
        u = u.replace(k, "");
    }
    while u.ends_with('/') && !u.ends_with("://") {
        u.pop();
    }
    u
}

fn scrub(line: &str, secret: &str) -> String {
    if secret.len() >= 2 {
        line.replace(secret, "****")
    } else {
        line.to_string()
    }
}

impl Session {
    pub fn spawn(
        ffmpeg: &Path,
        cfg: &OutputConfig,
        kind: Kind,
        target: &str,
        secret: &str,
        frames: FrameSlot,
        backend: AudioBackend,
    ) -> Result<Session, String> {
        let grab = if cfg.source == Source::Screen {
            screen_grab(ffmpeg)
        } else {
            Grab::X11
        };
        let args = build_args(cfg, kind, target, grab, backend)?;
        let mut cmd = Command::new(ffmpeg);
        cmd.args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        no_window(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("could not start ffmpeg ({}): {}", ffmpeg.display(), e))?;
        let stats = Arc::new(Mutex::new(Stats::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let delay = cfg.delay(kind);
        if let Some(err) = child.stderr.take() {
            scan_stderr(err, stats.clone(), secret, delay.is_some());
        }
        let out = child.stdout.take();
        let relay = match (delay, out) {
            (None, Some(out)) => {
                let st = stats.clone();
                std::thread::spawn(move || {
                    for line in BufReader::new(out).lines().map_while(Result::ok) {
                        if let Ok(mut s) = st.lock() {
                            parse_progress(&line, &mut s);
                        }
                    }
                });
                None
            }
            (Some(d), Some(out)) => {
                match spawn_relay(ffmpeg, target, secret, out, stats.clone(), stop.clone(), d) {
                    Ok(r) => Some(r),
                    Err(e) => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err(e);
                    }
                }
            }
            (_, None) => None,
        };
        let mut stdin = child.stdin.take().ok_or("ffmpeg stdin unavailable")?;
        let (w, h) = cfg.size();
        let fps = cfg.fps.clamp(1, 120) as f64;
        let source = cfg.source;
        let (st, stp) = (stats.clone(), stop.clone());
        let writer = std::thread::spawn(move || {
            if source == Source::Screen {
                while !stp.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = stdin.write_all(b"q\n");
                return;
            }
            let period = Duration::from_secs_f64(1.0 / fps);
            let start = Instant::now();
            // ticks written so far; tick n is due at start + n * period
            let mut ticks: u64 = 0;
            let mut cache: Option<(usize, Vec<u8>)> = None;
            let black = letterbox(None, w, h);
            loop {
                let stopping = stp.load(Ordering::Relaxed);
                let now = Instant::now();
                // every tick due by now gets a frame: the file's length (frames
                // / fps) follows the wall clock even when the encoder is slow
                let due = (now.duration_since(start).as_secs_f64() * fps) as u64 + 1;
                if stopping {
                    if let Ok(mut s) = st.lock() {
                        s.span = now.duration_since(start).as_secs_f64();
                    }
                    if ticks >= due {
                        break;
                    }
                } else if ticks >= due {
                    let at = start + period.mul_f64(ticks as f64);
                    std::thread::sleep(at.saturating_duration_since(now));
                    continue;
                }
                let frame = frames.lock().ok().and_then(|f| f.clone());
                let buf: &[u8] = match &frame {
                    Some(f) => {
                        let id = Arc::as_ptr(f) as usize;
                        if cache.as_ref().map(|c| c.0) != Some(id) {
                            cache = Some((id, letterbox(Some(f), w, h)));
                        }
                        &cache.as_ref().map(|c| &c.1).unwrap_or(&black)[..]
                    }
                    None => &black,
                };
                let n = due - ticks;
                let mut failed = false;
                for _ in 0..n {
                    if stdin.write_all(buf).is_err() {
                        failed = true;
                        break;
                    }
                    ticks += 1;
                }
                if let Ok(mut s) = st.lock() {
                    s.frames_in = ticks;
                    s.repeated += n.saturating_sub(1);
                    if failed && s.last_error.is_none() {
                        s.last_error = Some("ffmpeg stopped accepting frames".into());
                    }
                }
                if failed {
                    break;
                }
                if stopping {
                    break;
                }
            }
            // dropping stdin = EOF: ffmpeg flushes and finalises the file
        });
        let target_label = match kind {
            Kind::Record => target.to_string(),
            Kind::Live => scrub(target, secret),
        };
        Ok(Session {
            kind,
            target_label,
            path: (kind == Kind::Record).then(|| PathBuf::from(target)),
            started: Instant::now(),
            child,
            relay,
            delay,
            stats,
            stop,
            writer: Some(writer),
            finished: false,
            grab,
        })
    }

    pub fn stats(&self) -> Stats {
        self.stats.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// A ddagrab session that died before producing anything: Desktop
    /// Duplication isn't available here, so switch to gdigrab from now on.
    /// Returns true the first time that happens.
    pub fn note_grab_failure(&self) -> bool {
        self.grab == Grab::Ddagrab
            && self.stats().out_time <= 0.0
            && !DDA_BROKEN.swap(true, Ordering::SeqCst)
    }

    /// Where viewers are (s): the delayed position with a stream delay,
    /// otherwise what ffmpeg has sent.
    pub fn on_air(&self) -> f64 {
        let st = self.stats();
        if self.delay.is_some() {
            st.sent_time
        } else {
            st.out_time
        }
    }

    /// How far the stream has got (s), for telling a healthy stream from a
    /// stuck one. With a stream delay nothing is sent for the whole delay,
    /// so this is the encoder's intake into the delay queue instead.
    pub fn progress(&self) -> f64 {
        let st = self.stats();
        if self.delay.is_some() {
            st.queued_time
        } else {
            st.out_time
        }
    }

    /// `Some(success)` once ffmpeg has exited. With a stream delay both
    /// processes count: if one dies the other is killed.
    pub fn exited(&mut self) -> Option<bool> {
        let enc = exit_status(&mut self.child);
        let Some((relay, _)) = self.relay.as_mut() else {
            self.finished |= enc.is_some();
            return enc;
        };
        let rel = exit_status(relay);
        match (enc, rel) {
            (Some(a), Some(b)) => {
                self.finished = true;
                Some(a && b)
            }
            (None, None) => None,
            _ if self.stop.load(Ordering::Relaxed) => None,
            _ => {
                let _ = relay.kill();
                let _ = relay.wait();
                let _ = self.child.kill();
                let _ = self.child.wait();
                self.finished = true;
                Some(false)
            }
        }
    }

    /// Graceful stop: end the frame pipe (or send `q`), wait for ffmpeg to
    /// finalise, kill it only after `timeout`. A stream delay's unsent tail
    /// is dropped.
    pub fn stop(&mut self, timeout: Duration) -> Result<(), String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        let t0 = Instant::now();
        let result = loop {
            if let Some(ok) = self.exited() {
                break if ok {
                    Ok(())
                } else {
                    Err(self
                        .stats()
                        .last_error
                        .unwrap_or_else(|| "ffmpeg exited with an error".into()))
                };
            }
            if t0.elapsed() > timeout {
                self.kill();
                let _ = self.child.wait();
                if let Some((r, _)) = self.relay.as_mut() {
                    let _ = r.wait();
                }
                self.finished = true;
                break Err("ffmpeg did not finish in time and was killed".into());
            }
            std::thread::sleep(Duration::from_millis(30));
        };
        if let Some((_, pump)) = self.relay.take() {
            let _ = pump.join();
        }
        result
    }

    /// Hard kill (the test "drop connection" button).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
        if let Some((r, _)) = self.relay.as_mut() {
            let _ = r.kill();
        }
    }
}

fn exit_status(c: &mut Child) -> Option<bool> {
    match c.try_wait() {
        Ok(Some(st)) => Some(st.success()),
        Ok(None) => None,
        Err(_) => Some(false),
    }
}

/// Keep the last error-looking stderr line (secret scrubbed); with
/// `progress`, stderr also carries `-progress` lines.
fn scan_stderr(
    err: std::process::ChildStderr,
    st: Arc<Mutex<Stats>>,
    secret: &str,
    progress: bool,
) {
    let secret = secret.to_string();
    std::thread::spawn(move || {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            if progress {
                if let Ok(mut s) = st.lock() {
                    parse_progress(&line, &mut s);
                }
            }
            let l = scrub(line.trim(), &secret);
            let lower = l.to_lowercase();
            let bad = [
                "error",
                "failed",
                "refused",
                "invalid",
                "could not",
                "cannot",
                "no such",
                "broken pipe",
                "timed out",
                "unable",
            ];
            if bad.iter().any(|b| lower.contains(b)) {
                if let Ok(mut s) = st.lock() {
                    s.last_error = Some(l.chars().take(240).collect());
                }
            }
        }
    });
}

pub fn relay_args(target: &str) -> Vec<String> {
    [
        "-hide_banner",
        "-loglevel",
        "warning",
        "-nostats",
        "-f",
        "flv",
        "-i",
        "pipe:0",
        "-c",
        "copy",
        "-f",
        "flv",
        target,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Start the stream delay: a reader thread splits the encoder's FLV into
/// packets, a pump thread holds them in an [`OutputDelayQueue`] and writes
/// each one to the relay ffmpeg once it is due.
fn spawn_relay(
    ffmpeg: &Path,
    target: &str,
    secret: &str,
    mut flv: ChildStdout,
    stats: Arc<Mutex<Stats>>,
    stop: Arc<AtomicBool>,
    delay: Duration,
) -> Result<(Child, JoinHandle<()>), String> {
    let mut cmd = Command::new(ffmpeg);
    cmd.args(relay_args(target))
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    no_window(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| {
        format!(
            "could not start the ffmpeg relay ({}): {}",
            ffmpeg.display(),
            e
        )
    })?;
    if let Some(err) = child.stderr.take() {
        scan_stderr(err, stats.clone(), secret, false);
    }
    let Some(mut sink) = child.stdin.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("ffmpeg relay stdin unavailable".into());
    };
    let (tx, rx) = mpsc::channel::<Result<FlvItem, String>>();
    std::thread::spawn(move || {
        let mut reader = FlvReader::new();
        let mut buf = vec![0u8; 64 * 1024];
        let mut ok = true;
        // keep draining to EOF so the encoder never blocks on its stdout
        while let Ok(n) = flv.read(&mut buf) {
            if n == 0 {
                break;
            }
            if !ok {
                continue;
            }
            reader.push(&buf[..n]);
            loop {
                match reader.next_item() {
                    Ok(Some(item)) => {
                        let _ = tx.send(Ok(item));
                    }
                    Ok(None) => break,
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        ok = false;
                        break;
                    }
                }
            }
        }
    });
    let pump = std::thread::spawn(move || {
        let fail = |why: String| {
            if let Ok(mut s) = stats.lock() {
                s.last_error.get_or_insert(why);
            }
        };
        let mut queue = OutputDelayQueue::new(delay);
        while !stop.load(Ordering::Relaxed) {
            let wait = queue
                .next_due()
                .map(|t| t.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::MAX)
                .min(Duration::from_millis(50));
            match rx.recv_timeout(wait) {
                Ok(Ok(FlvItem::Header(h))) => {
                    if sink.write_all(&h).is_err() {
                        fail("the stream relay stopped accepting data".into());
                        return;
                    }
                }
                Ok(Ok(FlvItem::Packet(p))) => {
                    if let Ok(mut s) = stats.lock() {
                        s.queued_time = p.dts_ms as f64 / 1000.0;
                    }
                    queue.push(p, Instant::now());
                }
                Ok(Err(e)) => {
                    fail(format!("stream delay: {e}"));
                    return;
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
            while let Some(p) = queue.pop_due(Instant::now()) {
                if sink.write_all(&p.bytes).is_err() {
                    fail("the stream relay stopped accepting data".into());
                    return;
                }
                if let Ok(mut s) = stats.lock() {
                    s.sent_time = p.dts_ms as f64 / 1000.0;
                }
            }
        }
        // dropping `sink` ends the relay's input; the unsent tail is dropped
    });
    Ok((child, pump))
}

impl Drop for Session {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.stop(Duration::from_secs(5));
        }
    }
}

/// Status-bar summary.
#[derive(Clone, Debug, Default)]
pub struct Status {
    pub recording: Option<f64>,
    pub live: Option<f64>,
    pub reconnect_in: Option<f64>,
    pub fps: f32,
    pub kbps: f32,
    /// Frames ffmpeg dropped.
    pub dropped: u64,
    /// Ticks the writer filled with the previous frame (encoder behind).
    pub repeated: u64,
    pub error: Option<String>,
}

/// Owns the configuration, the stream key and the record/live sessions.
pub struct Output {
    pub cfg: OutputConfig,
    key: String,
    ffmpeg: Option<PathBuf>,
    pub ffmpeg_error: Option<String>,
    pub rec: Option<Session>,
    pub live: Option<Session>,
    live_since: Option<Instant>,
    pub reconnect: Reconnect,
    reconnect_at: Option<Instant>,
    pub live_error: Option<String>,
    pub rec_error: Option<String>,
    pub last_record: Option<PathBuf>,
    frames: FrameSlot,
    last_push: Option<Instant>,
    events: Vec<(ToastKind, String)>,
    /// UI only: show the key in clear text.
    pub reveal_key: bool,
    pub key_edit: String,
    /// Where the saved key lives (looked up lazily for Settings).
    key_place: Option<crate::secrets::Where>,
    /// Capture API for "Capture device" audio; `AudioBackend::host()`.
    pub audio_backend: AudioBackend,
    /// The open camera while the Camera source is recording, streaming
    /// or previewed; shared by all three.
    pub camera: Option<crate::camera::Feed>,
    /// Settings ▸ Output preview is showing the camera.
    pub camera_preview: bool,
    /// Last camera enumeration (Settings ▸ Output ▸ Refresh).
    /// Where settings are saved: output.json in the studio folder; in unit
    /// tests a temp folder of this Output's own, removed when it drops.
    cfg_path: PathBuf,
    #[cfg(test)]
    _cfg_dir: crate::test_dir::TestDir,
    pub cameras: Option<Result<crate::camera::Listing, crate::camera::CameraError>>,
    /// A camera is there but the system blocks access (why); Settings ▸
    /// Output shows Approve and checks again until it opens.
    pub camera_blocked: Option<String>,
    /// Approve was pressed on Linux: show the video-group fix.
    camera_fix_shown: bool,
    /// When blocked access was last checked, and whether the window had
    /// focus then (regaining focus checks at once).
    camera_checked: Option<Instant>,
    was_focused: bool,
    preview_tex: Option<(usize, egui::TextureHandle)>,
    preview_error: Option<String>,
}

impl Output {
    pub fn new(cfg: OutputConfig, key: String) -> Self {
        crate::crash::register_secret(&key);
        let mut reconnect = Reconnect::new();
        reconnect.set_settings(cfg.retry_max, cfg.retry_sec);
        #[cfg(test)]
        let cfg_dir = crate::test_dir::TestDir::new("output_cfg");
        #[cfg(test)]
        let cfg_path = cfg_dir.path().join("output.json");
        #[cfg(not(test))]
        let cfg_path = studio_dir().join("output.json");
        Self {
            cfg,
            cfg_path,
            #[cfg(test)]
            _cfg_dir: cfg_dir,
            key,
            ffmpeg: None,
            ffmpeg_error: None,
            rec: None,
            live: None,
            live_since: None,
            reconnect,
            reconnect_at: None,
            live_error: None,
            rec_error: None,
            last_record: None,
            frames: Arc::new(Mutex::new(None)),
            last_push: None,
            events: Vec::new(),
            reveal_key: false,
            key_edit: String::new(),
            key_place: None,
            audio_backend: AudioBackend::host(),
            camera: None,
            camera_preview: false,
            cameras: None,
            camera_blocked: None,
            camera_fix_shown: false,
            camera_checked: None,
            was_focused: true,
            preview_tex: None,
            preview_error: None,
        }
    }

    pub fn load() -> Self {
        let mut o = Self::new(OutputConfig::default(), load_key());
        let (cfg, warning) = OutputConfig::load_from(&o.cfg_path);
        o.cfg = cfg;
        o.reconnect.set_settings(o.cfg.retry_max, o.cfg.retry_sec);
        if let Some(w) = warning {
            o.events.push((ToastKind::Warn, w));
        }
        o
    }

    pub fn key_is_set(&self) -> bool {
        !self.key.is_empty()
    }

    pub fn masked_key(&self) -> String {
        mask(&self.key)
    }

    /// Only for the Settings "reveal" toggle.
    pub fn key_for_reveal(&self) -> &str {
        &self.key
    }

    pub fn set_key(&mut self, key: &str, persist: bool) -> Result<String, String> {
        self.key = key.trim().to_string();
        crate::crash::register_secret(&self.key);
        let mut place = crate::secrets::Where::None;
        if persist {
            place = save_key(&self.key)?;
            self.key_place = Some(place);
        }
        Ok(if self.key.is_empty() {
            "stream key cleared".into()
        } else if persist {
            format!(
                "stream key saved ({} characters, in {})",
                self.key.chars().count(),
                place.label()
            )
        } else {
            format!("stream key set ({} characters)", self.key.chars().count())
        })
    }

    /// Output settings for a project file: never the stream key (it stays in
    /// its own file), URL credentials or query tokens.
    pub fn project_cfg(&self) -> OutputConfig {
        let mut c = self.cfg.clone();
        c.rtmp_url = strip_url_secrets(&c.rtmp_url, &self.key);
        c
    }

    /// Settings from an opened project (the local key is kept; nothing is
    /// written to output.json until the user changes a setting).
    pub fn load_project_cfg(&mut self, c: OutputConfig) {
        self.cfg = c;
        self.reconnect
            .set_settings(self.cfg.retry_max, self.cfg.retry_sec);
    }

    pub fn save_config(&mut self) -> Result<(), String> {
        self.reconnect
            .set_settings(self.cfg.retry_max, self.cfg.retry_sec);
        self.cfg.save_to(&self.cfg_path)
    }

    pub fn ffmpeg(&mut self) -> Result<PathBuf, String> {
        if let Some(p) = &self.ffmpeg {
            return Ok(p.clone());
        }
        match find_ffmpeg() {
            Ok(p) => {
                self.ffmpeg = Some(p.clone());
                self.ffmpeg_error = None;
                Ok(p)
            }
            Err(e) => {
                self.ffmpeg_error = Some(e.clone());
                Err(e)
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.rec.is_some()
    }

    /// Live or trying to reconnect.
    pub fn is_live(&self) -> bool {
        self.live.is_some() || self.reconnect_at.is_some()
    }

    pub fn wants_frames(&self) -> bool {
        self.cfg.source == Source::Canvas && (self.rec.is_some() || self.live.is_some())
    }

    /// True when it's time for the app to render and hand over a new frame.
    pub fn frame_due(&self) -> bool {
        self.wants_frames()
            && self
                .last_push
                .map(|t| t.elapsed().as_secs_f64() >= 1.0 / self.cfg.fps.max(1) as f64)
                .unwrap_or(true)
    }

    pub fn push_frame(&mut self, r: Arc<Raster>) {
        self.last_push = Some(Instant::now());
        if let Ok(mut f) = self.frames.lock() {
            *f = Some(r);
        }
    }

    /// Frames for a new session: the canvas slot, or the camera's, opening
    /// the camera if it isn't open yet.
    fn session_frames(&mut self) -> Result<FrameSlot, String> {
        if self.cfg.source == Source::Camera {
            self.camera_frames()
        } else {
            Ok(self.frames.clone())
        }
    }

    /// The configured camera's frame slot. An open, healthy feed of the
    /// same camera is reused; anything else is closed and reopened.
    pub fn camera_frames(&mut self) -> Result<FrameSlot, String> {
        let want = self.cfg.camera_device.trim();
        if let Some(f) = &self.camera {
            let same = want.is_empty()
                || f.device.path == want
                || f.device.name.eq_ignore_ascii_case(want);
            if same && f.error().is_none() {
                return Ok(f.frames.clone());
            }
        }
        // close first: a camera can only be opened once
        self.camera = None;
        let (w, h) = self.cfg.size();
        let feed = match crate::camera::Feed::start(want, w, h) {
            Ok(f) => f,
            Err(e) => {
                if let crate::camera::CameraError::Blocked(m) = &e {
                    self.camera_blocked = Some(m.clone());
                }
                return Err(e.into());
            }
        };
        if self.camera_blocked.take().is_some() {
            self.camera_fix_shown = false;
            self.events.push((
                ToastKind::Success,
                "Camera access approved; the camera opens now.".into(),
            ));
        }
        Ok(self.camera.insert(feed).frames.clone())
    }

    /// Show or hide the camera preview in Settings ▸ Output.
    pub fn set_camera_preview(&mut self, on: bool) -> Result<(), String> {
        if on {
            self.camera_frames()?;
        }
        self.camera_preview = on;
        self.release_camera();
        Ok(())
    }

    /// Close the camera once nothing uses it.
    fn release_camera(&mut self) {
        let used = self.camera_preview
            || self.rec.is_some()
            || self.live.is_some()
            || self.reconnect_at.is_some();
        if !used {
            self.camera = None;
        }
    }

    pub fn refresh_cameras(&mut self) {
        self.set_cameras(crate::camera::list());
    }

    /// Take a scan's result. Blocked cameras keep (or start) the Approve
    /// flow, naming each one; with no camera at all there is nothing to
    /// approve. Otherwise blocked access found at open (Windows) stands
    /// until a test open succeeds.
    fn set_cameras(&mut self, r: Result<crate::camera::Listing, crate::camera::CameraError>) {
        match &r {
            Err(crate::camera::CameraError::Blocked(m)) => self.camera_blocked = Some(m.clone()),
            Ok(l) => {
                if let Some(why) = l.blocked_why() {
                    self.camera_blocked = Some(why);
                } else if l.cameras.is_empty() {
                    self.camera_blocked = None;
                }
            }
            _ => {}
        }
        self.cameras = Some(r);
    }

    /// The camera picker's entries: (label, hover text, selectable). Usable
    /// cameras first, then each blocked or failing node, named, so nothing
    /// is hidden.
    fn camera_rows(&self) -> Vec<(String, String, bool)> {
        let Some(Ok(l)) = &self.cameras else {
            return vec![];
        };
        let cams = l
            .cameras
            .iter()
            .map(|d| (d.name.clone(), d.path.clone(), true));
        let blocked = l
            .blocked
            .iter()
            .map(|u| (format!("{} (access blocked)", u.path), u.why.clone(), false));
        let failed = l
            .failed
            .iter()
            .map(|u| (format!("{} (can't open)", u.path), u.why.clone(), false));
        cams.chain(blocked).chain(failed).collect()
    }

    /// While camera access is blocked: check again every 2 s, and at once
    /// when the window regains focus (back from Settings or a terminal).
    pub fn camera_recheck_due(&mut self, now: Instant, focused: bool) -> bool {
        let regained = focused && !self.was_focused;
        self.was_focused = focused;
        self.camera_blocked.is_some()
            && (regained
                || self
                    .camera_checked
                    .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(2)))
    }

    /// Check blocked camera access again: re-scan, and if a camera is
    /// listed, try opening it (Windows only refuses at open). Closes it
    /// again unless something uses it.
    pub fn recheck_camera_access(&mut self) {
        self.camera_checked = Some(Instant::now());
        self.refresh_cameras();
        if self.camera_blocked.is_some()
            && matches!(&self.cameras, Some(Ok(l)) if !l.cameras.is_empty())
        {
            let _ = self.camera_frames();
            self.release_camera();
        }
    }

    fn default_record_path(&self) -> PathBuf {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        PathBuf::from(&self.cfg.record_dir).join(format!("Seraphina_{}.mp4", utc_stamp(secs)))
    }

    pub fn start_record(&mut self, path: Option<PathBuf>) -> Result<String, String> {
        if self.rec.is_some() {
            return Ok("already recording".into());
        }
        let ff = self.ffmpeg()?;
        let path = path.unwrap_or_else(|| self.default_record_path());
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)
                .map_err(|e| format!("can't create {}: {}", d.display(), e))?;
        }
        let frames = self.session_frames()?;
        let s = Session::spawn(
            &ff,
            &self.cfg,
            Kind::Record,
            &path.to_string_lossy(),
            "",
            frames,
            self.audio_backend,
        );
        if s.is_err() {
            self.release_camera();
        }
        let s = s?;
        self.rec = Some(s);
        self.rec_error = None;
        let (w, h) = self.cfg.size();
        Ok(format!(
            "recording {}×{} @ {} fps ({}, {}) to {}",
            w,
            h,
            self.cfg.fps,
            self.cfg.source.label(),
            self.cfg.audio.label(),
            path.display()
        ))
    }

    pub fn stop_record(&mut self) -> Result<String, String> {
        let Some(mut s) = self.rec.take() else {
            return Ok("already stopped".into());
        };
        let r = s.stop(Duration::from_secs(15));
        let path = s.path.clone().unwrap_or_default();
        let secs = s.elapsed();
        drop(s);
        self.release_camera();
        self.last_record = Some(path.clone());
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match r {
            Ok(()) if size > 0 => Ok(format!(
                "saved {:.1}s recording to {} ({:.1} MB)",
                secs,
                path.display(),
                size as f64 / 1e6
            )),
            Ok(()) => Err(format!("ffmpeg finished but {} is empty", path.display())),
            Err(e) => Err(format!("recording failed: {}", e)),
        }
    }

    fn spawn_live(&mut self) -> Result<(), String> {
        let ff = self.ffmpeg()?;
        let target = rtmp_target(&self.cfg.rtmp_url, &self.key);
        let frames = self.session_frames()?;
        let s = Session::spawn(
            &ff,
            &self.cfg,
            Kind::Live,
            &target,
            &self.key,
            frames,
            self.audio_backend,
        );
        if s.is_err() {
            self.release_camera();
        }
        let s = s?;
        self.live = Some(s);
        Ok(())
    }

    pub fn start_live(&mut self) -> Result<String, String> {
        if self.is_live() {
            return Ok("already live".into());
        }
        let url = self.cfg.rtmp_url.trim();
        if url.is_empty() {
            return Err("set an RTMP URL (and stream key) in Settings ▸ Output first".into());
        }
        if !(url.starts_with("rtmp://") || url.starts_with("rtmps://")) {
            return Err(format!("'{}' is not an rtmp:// or rtmps:// URL", url));
        }
        self.reconnect
            .set_settings(self.cfg.retry_max, self.cfg.retry_sec);
        self.reconnect.success();
        self.live_error = None;
        self.spawn_live()?;
        self.live_since = Some(Instant::now());
        Ok(format!(
            "live to {} (key {})",
            self.cfg.rtmp_url.trim(),
            if self.key_is_set() { "set" } else { "not set" }
        ))
    }

    pub fn stop_live(&mut self) -> Result<String, String> {
        self.reconnect_at = None;
        self.reconnect.success();
        self.live_since = None;
        match self.live.take() {
            Some(mut s) => {
                let _ = s.stop(Duration::from_secs(5));
                self.release_camera();
                Ok("stream ended".into())
            }
            None => {
                self.release_camera();
                Ok("already offline".into())
            }
        }
    }

    /// Whether Retry now has anything to do: a reconnect is pending, the
    /// stream gave up, or it is up but hasn't shown it is flowing yet. Once
    /// the stream is confirmed live (with a stream delay: the encoder is
    /// feeding the delay queue) Retry stays dormant until a real failure.
    pub fn retry_available(&self) -> bool {
        match &self.live {
            Some(s) => self.reconnect_at.is_some() || s.progress() <= 0.0,
            None => self.reconnect_at.is_some() || self.live_error.is_some(),
        }
    }

    /// Retry now: reconnect immediately if a reconnect is pending or the
    /// stream gave up; restart the stream if it is up but not flowing.
    pub fn retry(&mut self) -> Result<String, String> {
        if !self.retry_available() {
            return if self.live.is_some() {
                Ok("stream is live; nothing to retry".into())
            } else {
                Err("not live; use Go live".into())
            };
        }
        if let Some(mut s) = self.live.take() {
            s.kill();
        }
        self.reconnect_at = None;
        self.reconnect.success();
        self.live_error = None;
        self.spawn_live()?;
        self.live_since.get_or_insert_with(Instant::now);
        Ok("reconnecting now".into())
    }

    /// Kill the live ffmpeg process to exercise reconnect.
    pub fn drop_connection(&mut self) -> bool {
        match self.live.as_mut() {
            Some(s) => {
                s.kill();
                true
            }
            None => false,
        }
    }

    fn schedule_reconnect(&mut self, why: String) {
        if self.reconnect.start() {
            let wait = self.reconnect.wait_ms();
            self.reconnect_at = Some(Instant::now() + Duration::from_millis(wait));
            self.events.push((
                ToastKind::Warn,
                format!(
                    "Stream dropped ({}). Reconnecting in {:.1}s (attempt {}/{}).",
                    why,
                    wait as f64 / 1000.0,
                    self.reconnect.attempts(),
                    self.reconnect.retry_max()
                ),
            ));
        } else {
            self.reconnect_at = None;
            self.live_since = None;
            self.live_error = Some(why.clone());
            self.events.push((
                ToastKind::Error,
                format!(
                    "Stream ended after {} reconnect attempts: {}",
                    self.reconnect.retry_max(),
                    why
                ),
            ));
        }
    }

    /// Poll the processes; drive reconnects. Returns toasts to show.
    pub fn tick(&mut self) -> Vec<(ToastKind, String)> {
        if let Some(e) = self.camera.as_ref().and_then(|f| f.error()) {
            // a frozen picture is worse than a stop: end what uses the camera
            self.camera = None;
            self.camera_preview = false;
            let why = format!("camera stopped: {e}");
            if let Some(mut s) = self.rec.take() {
                let _ = s.stop(Duration::from_secs(15));
                self.last_record = s.path.clone();
                self.rec_error = Some(why.clone());
            }
            if self.live.take().is_some() || self.reconnect_at.is_some() {
                self.reconnect_at = None;
                self.live_since = None;
                self.live_error = Some(why.clone());
            }
            self.events.push((ToastKind::Error, why));
        }
        if self.cfg.source != Source::Camera {
            self.camera_preview = false;
        }
        if let Some(s) = self.rec.as_mut() {
            if let Some(ok) = s.exited() {
                let err = s.stats().last_error.unwrap_or_else(|| {
                    if ok {
                        "ffmpeg ended".into()
                    } else {
                        "ffmpeg exited with an error".into()
                    }
                });
                let dda = s.note_grab_failure();
                self.last_record = s.path.clone();
                self.rec = None;
                if dda {
                    self.events.push((
                        ToastKind::Warn,
                        "Desktop Duplication capture isn't available here; switched to GDI capture. Start the recording again.".into(),
                    ));
                }
                self.rec_error = Some(err.clone());
                self.events
                    .push((ToastKind::Error, format!("Recording stopped: {}", err)));
            }
        }
        let mut dead = None;
        if let Some(s) = self.live.as_mut() {
            if s.exited().is_some() {
                if s.note_grab_failure() {
                    self.events.push((
                        ToastKind::Warn,
                        "Desktop Duplication capture isn't available here; retrying with GDI capture.".into(),
                    ));
                }
                dead = Some(
                    s.stats()
                        .last_error
                        .unwrap_or_else(|| "connection closed".into()),
                );
            } else if self.reconnect.attempts() > 0 && s.progress() > 2.0 {
                self.reconnect.success();
                self.events
                    .push((ToastKind::Success, "Stream reconnected.".into()));
            }
        }
        if let Some(why) = dead {
            self.live = None;
            self.schedule_reconnect(why);
        }
        if let Some(at) = self.reconnect_at {
            if Instant::now() >= at {
                self.reconnect_at = None;
                if let Err(e) = self.spawn_live() {
                    self.schedule_reconnect(e);
                }
            }
        }
        self.release_camera();
        std::mem::take(&mut self.events)
    }

    /// The running stream's delay and where its viewers are.
    pub fn live_delay(&self) -> Option<(Duration, f64)> {
        let s = self.live.as_ref()?;
        Some((s.delay?, s.on_air()))
    }

    pub fn status(&self) -> Status {
        let mut st = Status {
            recording: self.rec.as_ref().map(|s| s.elapsed()),
            live: self.live_since.map(|t| t.elapsed().as_secs_f64()),
            reconnect_in: self
                .reconnect_at
                .map(|t| t.saturating_duration_since(Instant::now()).as_secs_f64()),
            ..Default::default()
        };
        for s in self.live.iter().chain(self.rec.iter()) {
            let x = s.stats();
            st.fps = st.fps.max(x.fps);
            st.kbps += x.kbps;
            st.dropped += x.ffmpeg_drops;
            st.repeated += x.repeated;
            if st.error.is_none() {
                st.error = x.last_error;
            }
        }
        if st.error.is_none() {
            st.error = self.live_error.clone().or_else(|| self.rec_error.clone());
        }
        st
    }

    /// Safe for the AI context: never includes the key.
    pub fn context(&self) -> String {
        let st = self.status();
        format!(
            "Output: ffmpeg {}, source {}{}, {}x{} @ {} fps, audio {}; recording {}; live {}; RTMP URL {}, stream key {}",
            if self.ffmpeg.is_some() { "found" } else if self.ffmpeg_error.is_some() { "missing" } else { "not checked" },
            self.cfg.source.label(),
            match (&self.cfg.source, self.cfg.camera_device.trim()) {
                (Source::Camera, "") => " (first camera)".to_string(),
                (Source::Camera, d) => format!(" ({d})"),
                _ => String::new(),
            },
            self.cfg.size().0,
            self.cfg.size().1,
            self.cfg.fps,
            self.cfg.audio.label(),
            st.recording.map(|t| format!("yes ({:.0}s)", t)).unwrap_or_else(|| "no".into()),
            if st.reconnect_in.is_some() { "reconnecting".to_string() } else { st.live.map(|t| format!("yes ({:.0}s)", t)).unwrap_or_else(|| "no".into()) },
            if self.cfg.rtmp_url.trim().is_empty() { "not set".to_string() } else { self.cfg.rtmp_url.trim().to_string() },
            if self.key_is_set() { "set (hidden)" } else { "not set" }
        )
    }
}

/// `YYYY-MM-DD_HH-MM-SSZ` (UTC) without a date library.
pub fn utc_stamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // civil_from_days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        rem / 60 % 60,
        rem % 60
    )
}

// ---- UI ---------------------------------------------------------------------

use crate::commands::AppCommand;
use crate::theme::{self, Palette, SP_XS};
use eframe::egui;
use egui_phosphor::regular as ic;

fn clock(t: f64) -> String {
    obs_rust_bitstream::ui_canvas::format_clock(t.max(0.0) as u64)
}

impl Output {
    /// Stream-mode card: real controls and live stats.
    pub fn panel(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        bus: &mut Vec<AppCommand>,
        open_settings: &mut bool,
    ) {
        let ff = self.ffmpeg();
        theme::card(ui, p, "Output · ffmpeg", |ui| {
            match &ff {
                Ok(path) => {
                    theme::kv(ui, p, "ffmpeg", format!("{}  found", ic::CHECK_CIRCLE));
                    ui.label(theme::muted(p, &path.display().to_string()));
                }
                Err(e) => {
                    ui.label(
                        egui::RichText::new("ffmpeg not found")
                            .color(p.danger)
                            .strong(),
                    );
                    ui.add(egui::Label::new(theme::muted(p, e)).wrap());
                }
            }
            if let Some(i) = theme::segmented(
                ui,
                p,
                "out-src",
                &["Canvas", "Screen", "Camera"],
                match self.cfg.source {
                    Source::Canvas => 0,
                    Source::Screen => 1,
                    Source::Camera => 2,
                },
                70.0,
            ) {
                bus.push(AppCommand::SetOutputSource {
                    source: ["canvas", "screen", "camera"][i].into(),
                    device: None,
                });
            }
            let (w, h) = self.cfg.size();
            ui.label(theme::muted(
                p,
                &format!(
                    "{}×{} @ {} fps · {} kb/s · {}",
                    w,
                    h,
                    self.cfg.fps,
                    self.cfg.video_kbps,
                    self.cfg.audio.label()
                ),
            ));
            ui.add_space(SP_XS);
            let live = self.is_live();
            let t = if live {
                format!("{}  End stream", ic::STOP)
            } else {
                format!("{}  Go live", ic::BROADCAST)
            };
            if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    theme::toggle_button(p, &t, live),
                )
                .clicked()
            {
                bus.push(AppCommand::SetStreaming { live: !live });
            }
            let rec = self.is_recording();
            let t = if rec {
                format!("{}  Stop recording", ic::STOP)
            } else {
                format!("{}  Record", ic::RECORD)
            };
            if ui
                .add_sized(
                    [ui.available_width(), 30.0],
                    theme::toggle_button(p, &t, rec),
                )
                .clicked()
            {
                bus.push(AppCommand::SetRecording { recording: !rec });
            }
            let st = self.status();
            if let Some(t) = st.recording {
                theme::kv(ui, p, "Recording", clock(t));
            }
            if let Some(t) = st.live {
                theme::kv(ui, p, "Live", clock(t));
            }
            if let Some((d, at)) = self.live_delay() {
                theme::kv(
                    ui,
                    p,
                    "Stream delay",
                    format!("{} s · viewers at {}", d.as_secs(), clock(at)),
                );
            }
            if st.recording.is_some() || st.live.is_some() {
                theme::kv(
                    ui,
                    p,
                    "Encoder",
                    format!("{:.0} fps · {:.0} kb/s", st.fps, st.kbps),
                );
                theme::kv(ui, p, "Dropped frames", format!("{}", st.dropped));
                // the encoder fell behind: these ticks got the previous frame
                // again, so the recording keeps real time
                theme::kv(ui, p, "Repeated frames", format!("{}", st.repeated));
                ui.ctx().request_repaint_after(Duration::from_millis(250));
            }
            if let Some(r) = st.reconnect_in {
                theme::kv(
                    ui,
                    p,
                    "Reconnecting in",
                    format!(
                        "{:.1} s (attempt {}/{})",
                        r,
                        self.reconnect.attempts(),
                        self.reconnect.retry_max()
                    ),
                );
            }
            if let Some(e) = &st.error {
                ui.add(egui::Label::new(egui::RichText::new(e).size(11.0).color(p.danger)).wrap());
            }
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.retry_available(),
                        theme::toggle_button(p, "Retry now", false),
                    )
                    .clicked()
                {
                    bus.push(AppCommand::RetryStream);
                }
                if ui
                    .add_enabled(
                        self.live.is_some(),
                        theme::toggle_button(p, "Drop connection (test)", false),
                    )
                    .on_hover_text("Kills the ffmpeg process to exercise reconnect")
                    .clicked()
                {
                    self.drop_connection();
                }
            });
            if let Some(path) = &self.last_record {
                ui.label(theme::muted(
                    p,
                    &format!("Last recording: {}", path.display()),
                ));
            }
            ui.add(
                egui::Label::new(theme::muted(
                    p,
                    &format!(
                        "RTMP URL: {}",
                        if self.cfg.rtmp_url.trim().is_empty() {
                            "not set"
                        } else {
                            self.cfg.rtmp_url.trim()
                        }
                    ),
                ))
                .wrap(),
            );
            theme::kv(ui, p, "Stream key", self.masked_key());
            if ui
                .add(theme::toggle_button(p, "Output settings…", false))
                .clicked()
            {
                *open_settings = true;
            }
        });
    }

    /// Blocked camera access: the reason, Approve, and a check again every
    /// 2 s (and on regaining focus) until the camera opens.
    fn camera_access_ui(&mut self, ui: &mut egui::Ui, p: &Palette, why: &str) {
        ui.add(egui::Label::new(egui::RichText::new(why).size(11.0).color(p.danger)).wrap());
        let approve = crate::camera::approve();
        if let Some(a) = approve {
            if ui.add(theme::toggle_button(p, "Approve", false)).clicked() {
                match a {
                    crate::camera::Approve::OpenSettings(uri) => {
                        if let Err(e) = crate::camera::settings_command(uri).spawn() {
                            self.preview_error = Some(format!("can't open {uri}: {e}"));
                        }
                    }
                    crate::camera::Approve::ShowFix { .. } => self.camera_fix_shown = true,
                }
            }
        }
        if let (true, Some(crate::camera::Approve::ShowFix { command, then })) =
            (self.camera_fix_shown, approve)
        {
            ui.horizontal_wrapped(|ui| {
                ui.label(theme::muted(p, "Run"));
                ui.code(command);
                ui.label(theme::muted(p, then));
            });
            if ui.add(theme::toggle_button(p, "Re-scan", false)).clicked() {
                self.recheck_camera_access();
            }
        }
        let focused = ui.ctx().input(|i| i.viewport().focused.unwrap_or(true));
        if self.camera_recheck_due(Instant::now(), focused) {
            self.recheck_camera_access();
        }
        ui.ctx().request_repaint_after(Duration::from_secs(2));
    }

    /// Camera picker and live preview (Settings ▸ Output, Camera source).
    fn camera_ui(&mut self, ui: &mut egui::Ui, p: &Palette, bus: &mut Vec<AppCommand>) {
        const FIRST: &str = "First camera found";
        ui.label(theme::section(p, "Camera"));
        if self.cameras.is_none() {
            self.refresh_cameras();
        }
        let mut pick = None;
        ui.horizontal(|ui| {
            let cur = self.cfg.camera_device.trim();
            egui::ComboBox::from_id_salt("out-camera")
                .selected_text(if cur.is_empty() { FIRST } else { cur })
                .width(220.0)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(cur.is_empty(), FIRST).clicked() {
                        pick = Some(String::new());
                    }
                    for (label, hover, usable) in self.camera_rows() {
                        let row = egui::Button::selectable(usable && cur == label, &label);
                        if ui.add_enabled(usable, row).on_hover_text(hover).clicked() {
                            pick = Some(label);
                        }
                    }
                });
            if ui.add(theme::toggle_button(p, "Refresh", false)).clicked() {
                self.refresh_cameras();
            }
        });
        if let Some(d) = pick {
            bus.push(AppCommand::SetOutputSource {
                source: "camera".into(),
                device: Some(d),
            });
        }
        if let Some(why) = self.camera_blocked.clone() {
            self.camera_access_ui(ui, p, &why);
        }
        match &self.cameras {
            Some(Err(crate::camera::CameraError::Failed(e))) => {
                ui.add(egui::Label::new(egui::RichText::new(e).size(11.0).color(p.danger)).wrap());
            }
            Some(Ok(l)) => {
                for u in &l.failed {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&u.why).size(11.0).color(p.danger))
                            .wrap(),
                    );
                }
                if l == &crate::camera::Listing::default() {
                    ui.label(theme::muted(
                        p,
                        "No camera found. Plug one in and press Refresh.",
                    ));
                }
            }
            _ => {}
        }
        let mut on = self.camera_preview;
        if ui.checkbox(&mut on, "Preview").changed() {
            self.preview_error = self.set_camera_preview(on).err();
        }
        if let Some(e) = &self.preview_error {
            ui.add(egui::Label::new(egui::RichText::new(e).size(11.0).color(p.danger)).wrap());
        }
        if !self.camera_preview {
            self.preview_tex = None;
            return;
        }
        let Some(feed) = &self.camera else { return };
        if let Some(f) = feed.latest() {
            let id = Arc::as_ptr(&f) as usize;
            if self.preview_tex.as_ref().map(|t| t.0) != Some(id) {
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [f.width as usize, f.height as usize],
                    &f.data,
                );
                match &mut self.preview_tex {
                    Some((old, t)) => {
                        t.set(img, egui::TextureOptions::LINEAR);
                        *old = id;
                    }
                    None => {
                        let t = ui.ctx().load_texture(
                            "camera-preview",
                            img,
                            egui::TextureOptions::LINEAR,
                        );
                        self.preview_tex = Some((id, t));
                    }
                }
            }
        }
        if let Some((_, t)) = &self.preview_tex {
            let [w, h] = t.size();
            let s = (320.0 / w as f32).min(240.0 / h as f32);
            ui.image((t.id(), egui::vec2(w as f32 * s, h as f32 * s)));
        }
        ui.label(theme::muted(
            p,
            &format!(
                "{} · {}×{} · {} frames",
                feed.device.name,
                feed.size.0,
                feed.size.1,
                feed.frames_read()
            ),
        ));
        ui.ctx().request_repaint_after(Duration::from_millis(33));
    }

    /// Settings window section. Returns true when the config changed.
    pub fn settings_ui(
        &mut self,
        ui: &mut egui::Ui,
        p: &Palette,
        bus: &mut Vec<AppCommand>,
    ) -> bool {
        let before = self.cfg.clone();
        ui.label(theme::section(p, "Output (ffmpeg)"));
        match &self.ffmpeg {
            Some(f) => theme::kv(ui, p, "ffmpeg", f.display().to_string()),
            None => {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            self.ffmpeg_error
                                .clone()
                                .unwrap_or_else(|| FFMPEG_HELP.into()),
                        )
                        .size(11.0)
                        .color(p.muted),
                    )
                    .wrap(),
                );
            }
        }
        let mut url = self.cfg.rtmp_url.clone();
        ui.horizontal(|ui| {
            ui.label(theme::muted(p, "RTMP URL"));
            let r = ui.add(
                egui::TextEdit::singleline(&mut url)
                    .hint_text("rtmp://live.example.com/app")
                    .desired_width(260.0),
            );
            if r.lost_focus() && url != self.cfg.rtmp_url {
                bus.push(AppCommand::SetStreamTarget { url: url.clone() });
            }
        });
        ui.horizontal(|ui| {
            ui.label(theme::muted(p, "Stream key"));
            let hint = self.masked_key();
            ui.add(
                egui::TextEdit::singleline(&mut self.key_edit)
                    .password(!self.reveal_key)
                    .hint_text(hint)
                    .desired_width(170.0),
            );
            if ui
                .add_enabled(
                    !self.key_edit.trim().is_empty(),
                    theme::toggle_button(p, "Save", false),
                )
                .clicked()
            {
                bus.push(AppCommand::SetStreamKey {
                    key: std::mem::take(&mut self.key_edit),
                });
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.reveal_key, "Show key");
            if self.reveal_key && self.key_is_set() {
                ui.label(
                    egui::RichText::new(self.key_for_reveal())
                        .monospace()
                        .size(11.0)
                        .color(p.text),
                );
            } else {
                ui.label(theme::muted(p, &self.masked_key()));
            }
        });
        egui::Grid::new("out-grid")
            .num_columns(2)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                ui.label(theme::muted(p, "Size"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.cfg.width)
                            .range(64..=3840)
                            .suffix(" w"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.cfg.height)
                            .range(64..=2160)
                            .suffix(" h"),
                    );
                });
                ui.end_row();
                ui.label(theme::muted(p, "Frame rate"));
                ui.add(
                    egui::DragValue::new(&mut self.cfg.fps)
                        .range(1..=60)
                        .suffix(" fps"),
                );
                ui.end_row();
                ui.label(theme::muted(p, "Video bitrate"));
                ui.add(
                    egui::DragValue::new(&mut self.cfg.video_kbps)
                        .range(200..=20000)
                        .speed(50.0)
                        .suffix(" kb/s"),
                );
                ui.end_row();
                ui.label(theme::muted(p, "Audio"));
                egui::ComboBox::from_id_salt("out-audio")
                    .selected_text(self.cfg.audio.label())
                    .show_ui(ui, |ui| {
                        for a in [Audio::None, Audio::Silent, Audio::Device] {
                            ui.selectable_value(&mut self.cfg.audio, a, a.label());
                        }
                    });
                ui.end_row();
                if self.cfg.audio == Audio::Device {
                    let (label, hint) = match self.audio_backend {
                        AudioBackend::DirectShow => {
                            ("DirectShow device", "Microphone (Realtek Audio)")
                        }
                        AudioBackend::Pulse => ("Pulse source", "default"),
                        AudioBackend::Unsupported => ("Not supported on this OS", ""),
                    };
                    ui.label(theme::muted(p, label));
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.audio_device).hint_text(hint));
                    ui.end_row();
                }
                ui.label(theme::muted(p, "Record folder"));
                ui.add(egui::TextEdit::singleline(&mut self.cfg.record_dir).desired_width(240.0));
                ui.end_row();
                ui.label(theme::muted(p, "Reconnect"));
                ui.horizontal(|ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.cfg.retry_sec)
                            .range(1..=60)
                            .suffix(" s"),
                    );
                    ui.add(
                        egui::DragValue::new(&mut self.cfg.retry_max)
                            .range(1..=50)
                            .suffix(" tries"),
                    );
                });
                ui.end_row();
                ui.label(theme::muted(p, "Stream delay"));
                ui.add(
                    egui::DragValue::new(&mut self.cfg.stream_delay_sec)
                        .range(0..=MAX_DELAY_SEC)
                        .suffix(" s"),
                )
                .on_hover_text(
                    "Viewers see the stream this many seconds late (0 = off). \
                     Stopping ends it at once; the delayed part is not sent.",
                );
                ui.end_row();
            });
        if self.cfg.source == Source::Camera {
            self.camera_ui(ui, p, bus);
        }
        let place = *self.key_place.get_or_insert_with(|| {
            if cfg!(test) {
                crate::secrets::Where::None
            } else {
                key_location()
            }
        });
        let note = match place {
            crate::secrets::Where::CredentialManager => {
                "The key is stored in Windows Credential Manager (SeraphinaStudio/stream_key)"
            }
            crate::secrets::Where::File => {
                "The key is stored in a private file (~/.seraphina/studio/stream_key, owner-only)"
            }
            crate::secrets::Where::None if cfg!(windows) => {
                "A saved key goes to Windows Credential Manager"
            }
            crate::secrets::Where::None => {
                "A saved key goes to a private file (~/.seraphina/studio/stream_key, owner-only)"
            }
        };
        ui.label(theme::muted(
            p,
            &format!(
                "{}. It is never shown to Seraphina or written to logs.",
                note
            ),
        ));
        self.cfg != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_cover_record_live_audio_and_screen() {
        let mut c = OutputConfig {
            width: 641,
            height: 359,
            ..Default::default()
        };
        let a = build_args(
            &c,
            Kind::Record,
            "/tmp/x.mp4",
            Grab::X11,
            AudioBackend::Pulse,
        )
        .expect("args")
        .join(" ");
        assert!(
            a.contains("-f rawvideo -pix_fmt rgba -s 640x358 -framerate 30 -i pipe:0"),
            "{}",
            a
        );
        // ffmpeg master (2026) made thread_queue_size muxer-only; as an input option it aborts startup
        assert!(!a.contains("thread_queue_size"));
        assert!(a.contains("anullsrc") && a.contains("-c:a aac") && a.contains("-shortest"));
        assert!(
            a.contains("-c:v libx264")
                && a.contains("yuv420p")
                && a.ends_with("-movflags +faststart -f mp4 -y /tmp/x.mp4")
        );
        c.audio = Audio::None;
        c.source = Source::Screen;
        let a = build_args(
            &c,
            Kind::Live,
            "rtmp://h/app/k",
            Grab::Gdigrab,
            AudioBackend::DirectShow,
        )
        .expect("args")
        .join(" ");
        assert!(a.contains("-f gdigrab -framerate 30 -draw_mouse 1 -i desktop"));
        assert!(!a.contains("hwdownload"));
        let a = build_args(
            &c,
            Kind::Live,
            "rtmp://h/app/k",
            Grab::X11,
            AudioBackend::Pulse,
        )
        .expect("args")
        .join(" ");
        assert!(a.contains("-f x11grab"));
        assert!(!a.contains("-c:a") && !a.contains("1:a") && a.contains("scale=640:358"));
        assert!(a.ends_with("-f flv rtmp://h/app/k"));
        assert_eq!(rtmp_target("rtmp://h/app/", " k "), "rtmp://h/app/k");
        assert_eq!(rtmp_target("rtmp://h/app", ""), "rtmp://h/app");
    }

    #[test]
    fn stream_delay_sends_flv_to_stdout_and_progress_to_stderr() {
        let mut c = OutputConfig {
            stream_delay_sec: 30,
            ..Default::default()
        };
        assert_eq!(c.delay(Kind::Live), Some(Duration::from_secs(30)));
        assert_eq!(c.delay(Kind::Record), None, "recordings are never delayed");
        let live = |c: &OutputConfig| {
            build_args(
                c,
                Kind::Live,
                "rtmp://h/app/k",
                Grab::X11,
                AudioBackend::Pulse,
            )
            .expect("args")
            .join(" ")
        };
        let a = live(&c);
        assert!(
            a.contains("-progress pipe:2") && a.ends_with("-f flv pipe:1"),
            "{a}"
        );
        assert!(
            !a.contains("rtmp://"),
            "the encoder must not reach the server: {a}"
        );
        let r = build_args(
            &c,
            Kind::Record,
            "/tmp/x.mp4",
            Grab::X11,
            AudioBackend::Pulse,
        )
        .expect("args")
        .join(" ");
        assert!(r.contains("-progress pipe:1") && r.ends_with("/tmp/x.mp4"));
        assert_eq!(
            relay_args("rtmp://h/app/k").join(" "),
            "-hide_banner -loglevel warning -nostats -f flv -i pipe:0 -c copy -f flv rtmp://h/app/k"
        );
        c.stream_delay_sec = 100_000;
        assert_eq!(
            c.delay(Kind::Live),
            Some(Duration::from_secs(MAX_DELAY_SEC as u64))
        );
        c.stream_delay_sec = 0;
        assert_eq!(c.delay(Kind::Live), None);
        let a = live(&c);
        assert!(a.contains("-progress pipe:1") && a.ends_with("-f flv rtmp://h/app/k"));
    }

    /// Settings ▸ Output shows the stream delay with its value.
    #[test]
    fn settings_show_the_stream_delay() {
        let ctx = egui::Context::default();
        let mut o = Output::new(
            OutputConfig {
                stream_delay_sec: 42,
                ..Default::default()
            },
            String::new(),
        );
        let mut bus = Vec::new();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 1400.0),
            )),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                o.settings_ui(ui, &Palette::dark(), &mut bus);
            });
        });
        let mut texts = Vec::new();
        for c in &out.shapes {
            if let egui::epaint::Shape::Text(t) = &c.shape {
                texts.push(t.galley.text().to_string());
            }
        }
        assert!(
            texts.iter().any(|t| t == "Stream delay"),
            "no Stream delay row in {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t == "42 s"),
            "delay value not shown: {texts:?}"
        );
    }

    #[test]
    fn ddagrab_is_preferred_on_windows_with_gdigrab_fallback() {
        assert_eq!(choose_grab(false, None, true, false), Grab::X11);
        assert_eq!(choose_grab(true, None, true, false), Grab::Ddagrab);
        assert_eq!(choose_grab(true, None, false, false), Grab::Gdigrab);
        assert_eq!(choose_grab(true, None, true, true), Grab::Gdigrab);
        assert_eq!(
            choose_grab(true, Some("GDIGRAB"), true, false),
            Grab::Gdigrab
        );
        assert_eq!(
            choose_grab(true, Some("ddagrab"), false, false),
            Grab::Ddagrab
        );
        let list = " ... abuffer           |->A       Buffer audio frames\n ... ddagrab           |->V       Grab Windows Desktop images using Desktop Duplication API.\n";
        assert!(filters_have_ddagrab(list));
        assert!(!filters_have_ddagrab(" ... gdigrabx |->V x\n"));
        let c = OutputConfig {
            source: Source::Screen,
            audio: Audio::None,
            width: 1920,
            height: 1080,
            fps: 60,
            ..Default::default()
        };
        let a = build_args(
            &c,
            Kind::Record,
            "o.mp4",
            Grab::Ddagrab,
            AudioBackend::DirectShow,
        )
        .expect("args")
        .join(" ");
        assert!(
            a.contains("-f lavfi -i ddagrab=output_idx=0:framerate=60:draw_mouse=1"),
            "{}",
            a
        );
        assert!(
            a.contains("-vf hwdownload,format=bgra,scale=1920:1080:"),
            "{}",
            a
        );
        assert!(!a.contains("gdigrab"));
    }

    #[test]
    fn key_is_masked_scrubbed_and_never_in_context() {
        let mut o = Output::new(
            OutputConfig {
                rtmp_url: "rtmp://example/live".into(),
                ..Default::default()
            },
            String::new(),
        );
        o.set_key("live_123456_secretkey", false).unwrap();
        assert!(!o.masked_key().contains("secret") && o.masked_key().contains("21 characters"));
        assert!(!o.context().contains("secret") && o.context().contains("set (hidden)"));
        assert_eq!(
            scrub(
                "Error opening rtmp://example/live/live_123456_secretkey",
                "live_123456_secretkey"
            ),
            "Error opening rtmp://example/live/****"
        );
        assert!(!o.set_key("abc", false).unwrap().contains("abc"));
    }

    #[test]
    fn progress_parsing_and_letterbox() {
        let mut s = Stats::default();
        for l in [
            "frame=150",
            "fps=30.01",
            "bitrate=1234.5kbits/s",
            "out_time_us=5000000",
            "drop_frames=2",
            "progress=end",
        ] {
            parse_progress(l, &mut s);
        }
        assert_eq!((s.enc_frames, s.ffmpeg_drops), (150, 2));
        assert!(
            (s.fps - 30.01).abs() < 1e-3
                && (s.kbps - 1234.5).abs() < 1e-3
                && (s.out_time - 5.0).abs() < 1e-9
                && s.ended
        );
        let r = Raster::filled(100, 50, [200, 100, 50, 255]);
        let b = letterbox(Some(&r), 40, 40);
        assert_eq!(
            &b[(20 * 40 + 20) * 4..(20 * 40 + 20) * 4 + 4],
            &[200, 100, 50, 255]
        );
        assert_eq!(&b[0..4], &[0, 0, 0, 255], "letterbox bars are black");
        assert_eq!(utc_stamp(1_792_540_800), "2026-10-21_00-00-00Z");
    }

    #[test]
    fn missing_ffmpeg_is_a_clear_error() {
        let e = find_ffmpeg_in(vec![
            PathBuf::from("/nonexistent/ffmpeg"),
            PathBuf::from("/etc/hostname"),
        ])
        .unwrap_err();
        assert!(
            e.contains("ffmpeg was not found") && e.contains("SERAPHINA_FFMPEG"),
            "{}",
            e
        );
        let mut o = Output::new(OutputConfig::default(), String::new());
        assert!(o.start_live().unwrap_err().contains("RTMP URL"));
        o.cfg.rtmp_url = "http://nope".into();
        assert!(o.start_live().unwrap_err().contains("rtmp://"));
    }

    fn probe(ffmpeg: &Path, target: &str) -> Option<String> {
        let o = Command::new(ffprobe_near(ffmpeg))
            .args([
                "-v",
                "error",
                "-rw_timeout",
                "8000000",
                "-show_entries",
                "stream=codec_name,codec_type,width,height:format=duration",
                "-of",
                "default=nw=1",
                target,
            ])
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&o.stdout).into_owned() + &String::from_utf8_lossy(&o.stderr))
    }

    fn moving_frame(i: u32) -> Arc<Raster> {
        let mut r = Raster::filled(320, 180, [20, 24, 40, 255]);
        let x0 = (i * 4) % 280;
        for y in 60..120 {
            for x in x0..x0 + 40 {
                let k = ((y * 320 + x) * 4) as usize;
                r.data[k..k + 4].copy_from_slice(&[214, 180, 92, 255]);
            }
        }
        Arc::new(r)
    }

    /// Records 5 s from the frame pipe and verifies the file with ffprobe.
    /// Set `RECORD_TEST_OUT` to keep a copy of the MP4.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn records_five_seconds_to_mp4_verified_by_ffprobe() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let cfg = OutputConfig {
            width: 640,
            height: 360,
            fps: 30,
            video_kbps: 1500,
            ..Default::default()
        };
        let mut o = Output::new(cfg, String::new());
        let tmp = crate::test_dir::TestDir::new("rec_test");
        let path = tmp.path().join("rec.mp4");
        o.start_record(Some(path.clone())).unwrap();
        let stats = o.rec.as_ref().unwrap().stats.clone();
        let t0 = Instant::now();
        let mut i = 0;
        while t0.elapsed() < Duration::from_secs(5) {
            o.push_frame(moving_frame(i));
            i += 1;
            assert!(o.tick().is_empty());
            std::thread::sleep(Duration::from_millis(33));
        }
        let st = o.status();
        assert!(st.recording.unwrap() >= 5.0);
        let msg = o.stop_record().unwrap();
        let w = stats.lock().unwrap().clone();
        // every tick of wall time got a frame, however slow the encoder was
        let frames_secs = w.frames_in as f64 / 30.0;
        assert!(w.span >= 5.0, "{:?}", w);
        assert!(
            (frames_secs - w.span).abs() <= 1.0 / 30.0 + 1e-9,
            "{} frames ({frames_secs:.3} s) for {:.3} s of wall time ({} repeated)",
            w.frames_in,
            w.span,
            w.repeated
        );
        assert!(msg.contains("saved"), "{}", msg);
        let p = probe(&ff, &path.to_string_lossy()).unwrap();
        assert!(
            p.contains("codec_name=h264") && p.contains("codec_name=aac"),
            "{}",
            p
        );
        assert!(p.contains("width=640") && p.contains("height=360"), "{}", p);
        let dur: f64 = p
            .lines()
            .find_map(|l| l.strip_prefix("duration="))
            .and_then(|d| d.parse().ok())
            .unwrap();
        assert!((4.5..=5.8).contains(&dur), "duration {} ({})", dur, p);
        let o2 = Command::new(ffprobe_near(&ff))
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=duration,nb_frames",
                "-of",
                "default=nw=1",
            ])
            .arg(&path)
            .output()
            .unwrap();
        let vp = String::from_utf8_lossy(&o2.stdout).into_owned();
        let field = |k: &str| -> f64 {
            vp.lines()
                .find_map(|l| l.strip_prefix(k))
                .and_then(|d| d.parse().ok())
                .unwrap_or_else(|| panic!("no {k} in {vp}"))
        };
        assert_eq!(field("nb_frames=") as u64, w.frames_in, "{vp}");
        assert!(
            (field("duration=") - frames_secs).abs() <= 1.0 / 30.0 + 1e-6,
            "video {vp} vs {frames_secs:.3} s written"
        );
        if let Ok(out) = std::env::var("RECORD_TEST_OUT") {
            std::fs::copy(&path, out).unwrap();
        }
        crate::test_dir::fail_late();
    }

    fn camera_test_output() -> Output {
        let cfg = OutputConfig {
            width: 320,
            height: 180,
            fps: 30,
            source: Source::Camera,
            camera_device: "Recorded Cam".into(),
            ..Default::default()
        };
        Output::new(cfg, String::new())
    }

    /// A test Output saves settings into its own temp folder, and dropping
    /// it (even in a failing test) removes them.
    #[test]
    fn a_test_output_removes_its_settings_file_when_dropped() {
        let mut o = Output::new(OutputConfig::default(), String::new());
        o.save_config().unwrap();
        let path = o.cfg_path.clone();
        assert!(path.is_file(), "{} saved", path.display());
        assert!(path.starts_with(std::env::temp_dir()), "{}", path.display());
        let other = Output::new(OutputConfig::default(), String::new());
        assert_ne!(other.cfg_path, path, "each test Output has its own file");
        drop(o);
        assert!(!path.exists(), "{} left behind", path.display());
        assert!(!path.parent().unwrap().exists());
    }

    /// Usable and blocked cameras together: the picker lists the usable
    /// camera and names each blocked node, and the panel shows each blocked
    /// node's reason with Approve. Real nodes: a readable one and two the
    /// user can't open (see camera.rs for why the usable camera is built).
    #[cfg(target_os = "linux")]
    #[test]
    fn settings_list_usable_cameras_and_name_each_blocked_one() {
        use crate::camera::tests::{blocked_why, locked_node, mixed_listing, rerun_as_nobody};
        if rerun_as_nobody("output::tests::settings_list_usable_cameras_and_name_each_blocked_one")
        {
            return;
        }
        let dir = crate::test_dir::TestDir::new("output_mixed_cameras");
        let readable = dir.path().join("video1");
        std::fs::write(&readable, b"").unwrap();
        let locked = [locked_node(&dir, "video2"), locked_node(&dir, "video3")];
        let probes = [&readable, &locked[0], &locked[1]]
            .iter()
            .map(|p| {
                let p = p.to_string_lossy().into_owned();
                let r = crate::camera::v4l2::probe(&p);
                (p, r)
            })
            .collect();
        let usable = crate::camera::Device {
            name: "USB Cam".into(),
            path: "/dev/video0".into(),
        };
        let mut o = Output::new(
            OutputConfig {
                source: Source::Camera,
                ..Default::default()
            },
            String::new(),
        );
        o.set_cameras(Ok(mixed_listing(usable, probes)));
        let whys: Vec<String> = locked.iter().map(|l| blocked_why(l)).collect();
        assert_eq!(o.camera_blocked, Some(whys.join("\n")));
        let path = |l: &std::path::PathBuf| l.to_string_lossy().into_owned();
        assert_eq!(
            o.camera_rows(),
            vec![
                ("USB Cam".to_string(), "/dev/video0".to_string(), true),
                (
                    format!("{} (access blocked)", path(&locked[0])),
                    whys[0].clone(),
                    false
                ),
                (
                    format!("{} (access blocked)", path(&locked[1])),
                    whys[1].clone(),
                    false
                ),
            ]
        );
        // the panel: no re-scan of the real /dev during the frame
        o.camera_checked = Some(Instant::now());
        o.was_focused = true;
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 1400.0),
            )),
            ..Default::default()
        };
        let mut bus = Vec::new();
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                o.settings_ui(ui, &Palette::dark(), &mut bus);
            });
        });
        let texts: String = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::epaint::Shape::Text(t) => Some(t.galley.text().to_string()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("|");
        for w in &whys {
            assert!(texts.contains(w.as_str()), "{w} not shown: {texts}");
        }
        assert!(texts.contains("Approve"), "{texts}");
        assert!(!texts.contains("No camera found"), "{texts}");
        // the blocked nodes get approved: the camera list stays, and the
        // Approve flow waits for a test open of the usable camera
        o.set_cameras(Ok(crate::camera::Listing {
            cameras: vec![crate::camera::Device {
                name: "USB Cam".into(),
                path: "/dev/video0".into(),
            }],
            ..Default::default()
        }));
        assert!(o.camera_blocked.is_some());
        // all gone: nothing left to approve
        o.set_cameras(Ok(crate::camera::Listing::default()));
        assert_eq!(o.camera_blocked, None);
    }

    /// Blocked camera access is checked again every 2 s, and at once when
    /// the window regains focus; never while nothing is blocked.
    #[test]
    fn blocked_camera_access_is_rechecked_on_a_timer_and_on_focus() {
        let mut o = Output::new(OutputConfig::default(), String::new());
        let t = Instant::now();
        assert!(!o.camera_recheck_due(t, true), "nothing blocked");
        o.camera_blocked = Some("Permission denied on /dev/video0".into());
        assert!(o.camera_recheck_due(t, true), "never checked yet");
        o.camera_checked = Some(t);
        assert!(!o.camera_recheck_due(t + Duration::from_millis(1900), true));
        assert!(o.camera_recheck_due(t + Duration::from_secs(2), true));
        assert!(!o.camera_recheck_due(t + Duration::from_millis(500), false));
        assert!(
            o.camera_recheck_due(t + Duration::from_millis(600), true),
            "focus came back"
        );
        assert!(!o.camera_recheck_due(t + Duration::from_millis(700), true));
    }

    /// A camera playing tests/fixtures/camera/red_320x180.mjpeg.
    fn recorded_camera(ok: Option<u32>) -> crate::camera::Feed {
        let cam = crate::camera::tests::Recorded::red_mjpeg(ok);
        crate::camera::Feed::from_reader("Recorded Cam", cam).unwrap()
    }

    /// The middle pixel of the file's frame at `t` seconds, as RGB.
    fn pixel_at(ff: &Path, file: &Path, t: &str) -> [u8; 3] {
        let o = Command::new(ff)
            .args(["-v", "error", "-ss", t, "-i"])
            .arg(file)
            .args(["-frames:v", "1", "-vf", "crop=2:2:159:89,scale=1:1"])
            .args(["-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
            .output()
            .unwrap();
        assert_eq!(o.stdout.len(), 3, "{}", String::from_utf8_lossy(&o.stderr));
        [o.stdout[0], o.stdout[1], o.stdout[2]]
    }

    /// The Camera source records what the camera sends, not the canvas:
    /// the canvas slot gets a blue frame, the (recorded) camera a red one, and
    /// the file must be red.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn camera_source_records_the_camera_picture() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let tmp = crate::test_dir::TestDir::new("cam");
        let path = tmp.path().join("cam.mp4");
        let mut o = camera_test_output();
        o.camera = Some(recorded_camera(None));
        o.push_frame(Arc::new(Raster::filled(320, 180, [20, 40, 220, 255])));
        o.start_record(Some(path.clone())).unwrap();
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(1500) {
            assert!(o.tick().is_empty());
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(o.context().contains("(Recorded Cam)"), "{}", o.context());
        o.stop_record().unwrap();
        assert!(o.camera.is_none(), "camera closed once nothing uses it");
        let [r, g, b] = pixel_at(&ff, &path, "0.7");
        assert!(
            r > 170 && g < 80 && b < 80,
            "expected camera red, got {r},{g},{b}"
        );
        crate::test_dir::fail_late();
    }

    /// A camera that stops sending ends the recording with the reason,
    /// rather than freezing on the last frame.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn camera_failure_stops_the_recording_with_a_reason() {
        find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let tmp = crate::test_dir::TestDir::new("cam_fail");
        let path = tmp.path().join("cam.mp4");
        let mut o = camera_test_output();
        o.camera = Some(recorded_camera(Some(40)));
        o.start_record(Some(path.clone())).unwrap();
        let t0 = Instant::now();
        let mut toasts = Vec::new();
        while o.is_recording() && t0.elapsed() < Duration::from_secs(10) {
            toasts.extend(o.tick());
            std::thread::sleep(Duration::from_millis(30));
        }
        assert!(!o.is_recording(), "recording kept going on a dead camera");
        assert_eq!(o.rec_error.as_deref(), Some("camera stopped: unplugged"));
        assert!(
            toasts
                .iter()
                .any(|(k, m)| *k == ToastKind::Error && m.contains("camera stopped: unplugged")),
            "{toasts:?}"
        );
        assert!(o.camera.is_none());
        crate::test_dir::fail_late();
    }

    #[test]
    fn camera_source_pipes_raw_frames_and_parses() {
        assert_eq!(Source::parse("Camera"), Some(Source::Camera));
        assert_eq!(Source::parse("webcam"), Some(Source::Camera));
        let c = OutputConfig {
            source: Source::Camera,
            ..Default::default()
        };
        let a = build_args(
            &c,
            Kind::Record,
            "/tmp/x.mp4",
            Grab::X11,
            AudioBackend::Pulse,
        )
        .unwrap()
        .join(" ");
        assert!(
            a.contains("-f rawvideo -pix_fmt rgba") && a.contains("-i pipe:0"),
            "{a}"
        );
        assert!(!a.contains("x11grab") && a.contains("-shortest"), "{a}");
    }

    /// Whether the socket table (`/proc/net/tcp`) shows `port` listening.
    /// Without the table there is no way to tell, so it fails loudly
    /// instead of guessing.
    fn listening(port: u16, table: std::io::Result<String>) -> bool {
        let t = table.unwrap_or_else(|e| {
            panic!("can't read /proc/net/tcp to see the RTMP listener come up: {e}")
        });
        let hex = format!(":{port:04X} ");
        t.lines().any(|l| l.contains(&hex) && l.contains(" 0A "))
    }

    #[test]
    fn listening_reads_the_socket_table() {
        let table = concat!(
            "  sl  local_address rem_address   st tx_queue rx_queue\n",
            "   0: 0100007F:7A69 00000000:0000 0A 00000000:00000000\n",
            "   1: 0100007F:1F90 0100007F:9C40 01 00000000:00000000\n",
        );
        assert!(listening(31337, Ok(table.into())));
        assert!(
            !listening(8080, Ok(table.into())),
            "connected, not listening"
        );
        assert!(!listening(1935, Ok(table.into())));
    }

    #[test]
    #[should_panic(expected = "can't read /proc/net/tcp")]
    fn listening_without_a_socket_table_fails_loudly() {
        listening(1935, Err(std::io::ErrorKind::NotFound.into()));
    }

    /// A one-shot `ffmpeg -listen 1` RTMP server on 127.0.0.1 that saves
    /// what it receives under `live/rx_test_key`; returns it, its port and
    /// the folder holding the file (removed when dropped) and the file.
    fn rtmp_listener(ff: &Path, tag: &str) -> (Child, u16, crate::test_dir::TestDir, PathBuf) {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
            l.local_addr().expect("local addr").port()
        };
        let tmp = crate::test_dir::TestDir::new(&format!("rtmp_rx_{tag}"));
        let got = tmp.path().join("rx.flv");
        let server = Command::new(ff)
            .args(["-hide_banner", "-loglevel", "error", "-listen", "1", "-i"])
            .arg(format!("rtmp://127.0.0.1:{port}/live/rx_test_key"))
            .args(["-c", "copy", "-y"])
            .arg(&got)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn the ffmpeg RTMP listener");
        // Wait until the one-shot listener is bound. Connecting to check
        // would use up its only accept, so read the socket table instead.
        let t0 = Instant::now();
        while !listening(port, std::fs::read_to_string("/proc/net/tcp")) {
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "listener never came up"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        (server, port, tmp, got)
    }

    /// Goes live against `ffmpeg -listen 1` acting as the RTMP server: the
    /// listener writes what it receives to an FLV, which ffprobe must read
    /// back as H.264 at the configured size. Needs only ffmpeg; no network
    /// beyond 127.0.0.1.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn goes_live_to_an_ffmpeg_rtmp_listener() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let (mut server, port, _tmp, got) = rtmp_listener(&ff, "plain");
        let cfg = OutputConfig {
            rtmp_url: format!("rtmp://127.0.0.1:{port}/live"),
            width: 320,
            height: 180,
            fps: 30,
            video_kbps: 600,
            ..Default::default()
        };
        let mut o = Output::new(cfg, String::new());
        o.set_key("rx_test_key", false).expect("set key");
        o.push_frame(moving_frame(0));
        o.start_live().expect("start live");
        let mut ev = Vec::new();
        let t0 = Instant::now();
        let mut i = 0;
        while t0.elapsed() < Duration::from_secs(3) {
            o.push_frame(moving_frame(i));
            i += 1;
            ev.extend(o.tick());
            std::thread::sleep(Duration::from_millis(33));
        }
        assert!(o.is_live(), "dropped: {:?}", ev);
        o.stop_live().expect("stop live");
        let t0 = Instant::now();
        let status = loop {
            if let Some(st) = server.try_wait().expect("listener status") {
                break st;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "listener did not finish"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        let mut err = String::new();
        if let Some(mut e) = server.stderr.take() {
            use std::io::Read;
            let _ = e.read_to_string(&mut err);
        }
        assert!(status.success(), "listener failed: {err}");
        let p = probe(&ff, &got.to_string_lossy()).expect("ffprobe");
        assert!(
            p.contains("codec_name=h264") && p.contains("width=320") && p.contains("height=180"),
            "listener did not receive the stream: {p}"
        );
        assert!(
            ev.iter().all(|(_, m)| !m.contains("rx_test_key")),
            "key leaked into a message"
        );
        crate::test_dir::fail_late();
    }

    /// A live Output with a 10 s stream delay, aimed at a free port with
    /// no server: inside the delay window the relay has nothing to send, so
    /// it never connects and the stream is healthy but not yet on air.
    fn delayed_output(retry_sec: u32) -> Output {
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
            l.local_addr().expect("local addr").port()
        };
        let cfg = OutputConfig {
            rtmp_url: format!("rtmp://127.0.0.1:{port}/live"),
            width: 320,
            height: 180,
            fps: 30,
            video_kbps: 600,
            stream_delay_sec: 10,
            retry_max: 20,
            retry_sec,
            ..Default::default()
        };
        let mut o = Output::new(cfg, String::new());
        o.set_key("delay_test_key", false).expect("set key");
        o.push_frame(moving_frame(0));
        o
    }

    /// Ticks (pushing frames) until `done`, failing after `limit`.
    fn tick_until(o: &mut Output, limit: Duration, what: &str, done: impl Fn(&Output) -> bool) {
        let t0 = Instant::now();
        let mut i = 0;
        while !done(o) {
            assert!(t0.elapsed() < limit, "{what} within {limit:?}");
            o.push_frame(moving_frame(i));
            i += 1;
            o.tick();
            std::thread::sleep(Duration::from_millis(33));
        }
    }

    /// Retry now inside the delay window must not kill a healthy stream:
    /// nothing is on air yet, but the encoder is feeding the delay queue.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn retry_now_during_the_delay_keeps_a_healthy_stream() {
        find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let mut o = delayed_output(2);
        o.start_live().expect("start live");
        tick_until(
            &mut o,
            Duration::from_secs(8),
            "the encoder fed the delay queue",
            |o| o.live.as_ref().is_some_and(|s| s.progress() > 1.0),
        );
        let s = o.live.as_ref().expect("live");
        assert_eq!(s.on_air(), 0.0, "nothing should be on air inside the delay");
        let pid = s.child.id();
        assert!(!o.retry_available(), "Retry now must be dormant once live");
        let msg = o.retry().expect("retry");
        assert!(msg.contains("nothing to retry"), "{msg}");
        let s = o.live.as_ref().expect("still live");
        assert_eq!(s.child.id(), pid, "Retry now restarted a healthy stream");
        assert!(o.is_live());
        o.stop_live().expect("stop live");
    }

    /// Retry now wakes up again after a real drop: a reconnect is pending
    /// and Retry now makes it happen at once.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn retry_now_reenables_after_a_real_drop() {
        find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let mut o = delayed_output(5);
        o.start_live().expect("start live");
        tick_until(
            &mut o,
            Duration::from_secs(8),
            "the stream is flowing",
            |o| o.live.as_ref().is_some_and(|s| s.progress() > 1.0),
        );
        assert!(!o.retry_available(), "dormant while live");
        assert!(o.drop_connection());
        tick_until(
            &mut o,
            Duration::from_secs(4),
            "the drop was noticed",
            |o| o.reconnect_at.is_some(),
        );
        assert!(o.retry_available(), "Retry now must be active after a drop");
        assert_eq!(o.retry(), Ok("reconnecting now".into()));
        assert!(o.reconnect_at.is_none() && o.live.is_some());
        o.stop_live().expect("stop live");
    }

    /// After a reconnect with a stream delay, the stream counts as back once
    /// the encoder is flowing again, not after delay + 2 s on air. So a
    /// second drop soon after is attempt 1 again, not attempt 2.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn a_reconnect_inside_the_delay_window_resets_the_attempt_count() {
        find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let mut o = delayed_output(1);
        o.start_live().expect("start live");
        let reconnected = |o: &Output| {
            o.reconnect.attempts() == 1 && o.reconnect_at.is_none() && o.live.is_some()
        };
        assert!(o.drop_connection());
        tick_until(
            &mut o,
            Duration::from_secs(8),
            "reconnect attempt 1",
            reconnected,
        );
        tick_until(
            &mut o,
            Duration::from_secs(8),
            "the reconnect counted as a success",
            |o| o.reconnect.attempts() == 0,
        );
        assert_eq!(
            o.live.as_ref().expect("live").on_air(),
            0.0,
            "this must happen inside the delay window"
        );
        assert!(o.drop_connection());
        tick_until(
            &mut o,
            Duration::from_secs(8),
            "reconnect after the second drop",
            |o| o.reconnect_at.is_none() && o.live.is_some() && o.reconnect.attempts() > 0,
        );
        assert_eq!(o.reconnect.attempts(), 1, "the earlier drop still counted");
        o.stop_live().expect("stop live");
    }

    /// Stream delay: the encoder runs at once, but nothing reaches the RTMP
    /// server until the delay has passed, and then the whole stream does.
    /// The relay connects to the server only once it has packets to send,
    /// so the connection showing up in /proc/net/tcp marks the first data.
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "needs ffmpeg and /proc/net/tcp (Windows runners and Wine have no ffmpeg.exe)"
    )]
    fn stream_delay_holds_the_stream_back_by_the_delay() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let (mut server, port, _tmp, got) = rtmp_listener(&ff, "delay");
        let cfg = OutputConfig {
            rtmp_url: format!("rtmp://127.0.0.1:{port}/live"),
            width: 320,
            height: 180,
            fps: 30,
            video_kbps: 600,
            stream_delay_sec: 4,
            ..Default::default()
        };
        let mut o = Output::new(cfg, String::new());
        o.set_key("rx_test_key", false).expect("set key");
        o.push_frame(moving_frame(0));
        o.start_live().expect("start live");
        let t0 = Instant::now();
        let hex = format!(":{port:04X} ");
        let (mut ev, mut i) = (Vec::new(), 0);
        let mut connected = None;
        let mut encoding_while_held = false;
        while t0.elapsed() < Duration::from_secs(10) {
            o.push_frame(moving_frame(i));
            i += 1;
            ev.extend(o.tick());
            let s = o.live.as_ref().expect("still live");
            let held = s.stats().out_time > 0.5 && s.on_air() == 0.0;
            encoding_while_held |= held && t0.elapsed() < Duration::from_millis(3900);
            let tcp = std::fs::read_to_string("/proc/net/tcp").expect("read /proc/net/tcp");
            if connected.is_none() && tcp.lines().any(|l| l.contains(&hex) && l.contains(" 01 ")) {
                connected = Some(t0.elapsed());
            }
            std::thread::sleep(Duration::from_millis(33));
        }
        assert!(o.is_live(), "dropped: {:?}", ev);
        let (delay, viewers_at) = o.live_delay().expect("delay shown while live");
        let enc = o.live.as_ref().expect("live").stats().out_time;
        assert_eq!(delay, Duration::from_secs(4));
        let first = connected.expect("the relay never connected to the RTMP server");
        assert!(
            first >= delay,
            "the server got a connection {first:?} after going live, before the 4 s delay"
        );
        assert!(
            viewers_at > 1.0 && enc - viewers_at > 3.0,
            "viewers at {viewers_at:.2}s, encoder at {enc:.2}s"
        );
        assert!(
            encoding_while_held,
            "the encoder never ran while the delay held its output"
        );
        o.stop_live().expect("stop live");
        let t0 = Instant::now();
        while server.try_wait().expect("listener status").is_none() {
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "listener did not finish"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
        let p = probe(&ff, &got.to_string_lossy()).expect("ffprobe");
        assert!(
            p.contains("codec_name=h264") && p.contains("width=320") && p.contains("height=180"),
            "listener did not receive the stream: {p}"
        );
        crate::test_dir::fail_late();
    }

    /// With a stream delay two ffmpegs run; when the relay dies (here: no
    /// server on the port) the encoder must be killed and the session end
    /// with the relay's error, so the stream can reconnect as a whole.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn a_dead_relay_ends_the_delayed_stream() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let port = free_port();
        let cfg = OutputConfig {
            width: 160,
            height: 90,
            stream_delay_sec: 1,
            ..Default::default()
        };
        let frames: FrameSlot = Arc::new(Mutex::new(Some(moving_frame(0))));
        let target = format!("rtmp://127.0.0.1:{port}/live/secret_key_x");
        let mut s = Session::spawn(
            &ff,
            &cfg,
            Kind::Live,
            &target,
            "secret_key_x",
            frames,
            AudioBackend::host(),
        )
        .expect("spawn");
        let t0 = Instant::now();
        let ok = loop {
            if let Some(ok) = s.exited() {
                break ok;
            }
            assert!(
                t0.elapsed() < Duration::from_secs(20),
                "the session kept running after its relay died"
            );
            std::thread::sleep(Duration::from_millis(50));
        };
        assert!(!ok);
        assert!(
            s.child.try_wait().expect("encoder status").is_some(),
            "the encoder was left running"
        );
        let err = s.stats().last_error.unwrap_or_default();
        assert!(
            !err.is_empty() && !err.contains("secret_key_x"),
            "error: {err:?}"
        );
    }

    fn mediamtx_bin() -> Option<PathBuf> {
        let c = std::env::var_os("MEDIAMTX")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join("opt/mediamtx/mediamtx"))
            })?;
        c.is_file().then_some(c)
    }

    /// A running mediamtx and its config folder. Dropping it (also when the
    /// test fails) stops the server, then removes the folder.
    struct Mtx {
        child: Child,
        _dir: crate::test_dir::TestDir,
    }

    impl Mtx {
        fn kill(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    impl Drop for Mtx {
        fn drop(&mut self) {
            self.kill();
        }
    }

    fn start_mediamtx(bin: &Path, port: u16) -> Mtx {
        let c = spawn_mediamtx(
            bin,
            "reconnect",
            &format!("rtmp: yes\nrtmpAddress: 127.0.0.1:{port}\napi: no\npaths:\n  all_others:\n"),
        );
        std::thread::sleep(Duration::from_millis(800));
        c
    }

    /// mediamtx with only the given settings on top of a quiet base config
    /// (no RTSP/HLS/WebRTC/SRT). `tag` keeps parallel tests' configs apart.
    fn spawn_mediamtx(bin: &Path, tag: &str, settings: &str) -> Mtx {
        let dir = crate::test_dir::TestDir::new(&format!("mtx_{tag}"));
        let yml = dir.path().join("mediamtx.yml");
        std::fs::write(&yml, format!("logLevel: warn\nmetrics: no\npprof: no\nplayback: no\nrtsp: no\nhls: no\nwebrtc: no\nsrt: no\n{settings}")).expect("write mediamtx.yml");
        let child = Command::new(bin)
            .arg(&yml)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mediamtx");
        Mtx { child, _dir: dir }
    }

    fn free_port() -> u16 {
        let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
        l.local_addr().expect("local addr").port()
    }

    /// GET from the mediamtx control API: Ok(body) on 2xx, Err(status) else.
    fn mtx_api(api: u16, path: &str) -> Result<String, u16> {
        match ureq::get(&format!("http://127.0.0.1:{api}{path}"))
            .timeout(Duration::from_secs(2))
            .call()
        {
            Ok(r) => r.into_string().map_err(|_| 0),
            Err(ureq::Error::Status(c, _)) => Err(c),
            Err(_) => Err(0),
        }
    }

    /// The app's real streaming path (Output::start_live → ffmpeg → RTMP)
    /// publishes to mediamtx under the caller's stream key, and a wrong key
    /// is rejected. mediamtx accepts only `live/good_key`; its control API
    /// must report that path ready with an H.264 track and bytes received,
    /// and ffprobe must read the stream back. Publishing with `wrong_key`
    /// must be refused by the server and surface as an error in the app.
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "mediamtx is installed on the Linux runner only"
    )]
    fn publishes_to_mediamtx_under_the_callers_key_and_rejects_a_wrong_key() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let mtx = mediamtx_bin()
            .expect("mediamtx not found: set MEDIAMTX or install it at ~/opt/mediamtx/mediamtx");
        let (rtmp, api) = (free_port(), free_port());
        let mut server = spawn_mediamtx(
            &mtx,
            "keys",
            &format!("rtmp: yes\nrtmpAddress: 127.0.0.1:{rtmp}\napi: yes\napiAddress: 127.0.0.1:{api}\npaths:\n  live/good_key:\n"),
        );
        let t0 = Instant::now();
        while mtx_api(api, "/v3/paths/list").is_err() {
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "mediamtx API never came up"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
        let run = |key: &str, secs: u64| {
            let cfg = OutputConfig {
                rtmp_url: format!("rtmp://127.0.0.1:{rtmp}/live"),
                width: 320,
                height: 180,
                fps: 30,
                video_kbps: 600,
                retry_sec: 1,
                retry_max: 2,
                ..Default::default()
            };
            let mut o = Output::new(cfg, String::new());
            o.set_key(key, false).expect("set key");
            o.push_frame(moving_frame(0));
            o.start_live().expect("start live");
            let mut ev = Vec::new();
            let t0 = Instant::now();
            let mut i = 0;
            while t0.elapsed() < Duration::from_secs(secs) {
                o.push_frame(moving_frame(i));
                i += 1;
                ev.extend(o.tick());
                std::thread::sleep(Duration::from_millis(33));
            }
            (o, ev)
        };

        // Right key: the server has the path, ready, with video.
        let (mut o, ev) = run("good_key", 3);
        let body = mtx_api(api, "/v3/paths/get/live/good_key").unwrap_or_else(|c| {
            panic!("live/good_key not on the server (HTTP {c}); app events: {ev:?}")
        });
        let j: serde_json::Value = serde_json::from_str(&body).expect("API JSON");
        assert_eq!(j["ready"], true, "{body}");
        assert!(
            j["tracks"]
                .as_array()
                .is_some_and(|t| t.iter().any(|t| t == "H264")),
            "no H264 track: {body}"
        );
        assert!(j["bytesReceived"].as_u64().unwrap_or(0) > 0, "{body}");
        let p = probe(&ff, &format!("rtmp://127.0.0.1:{rtmp}/live/good_key")).expect("ffprobe");
        assert!(
            p.contains("codec_name=h264") && p.contains("width=320") && p.contains("height=180"),
            "read-back failed: {p}"
        );
        assert!(
            o.is_live() && o.status().error.is_none(),
            "{:?} {ev:?}",
            o.status()
        );
        assert!(
            ev.iter()
                .all(|(k, _)| *k == ToastKind::Info || *k == ToastKind::Success),
            "{ev:?}"
        );
        o.stop_live().expect("stop live");

        // Wrong key: the server refuses the path and the app says so.
        let (mut o, ev) = run("wrong_key", 5);
        assert_eq!(
            mtx_api(api, "/v3/paths/get/live/wrong_key"),
            Err(404),
            "server accepted a wrong key"
        );
        let list = mtx_api(api, "/v3/paths/list").expect("paths list");
        assert!(!list.contains("wrong_key"), "{list}");
        assert!(
            ev.iter()
                .any(|(k, m)| *k == ToastKind::Warn && m.starts_with("Stream dropped")),
            "a rejected key must surface: {ev:?}"
        );
        assert!(o.status().error.is_some(), "{:?}", o.status());
        assert!(
            ev.iter()
                .all(|(_, m)| !m.contains("good_key") && !m.contains("wrong_key")),
            "key leaked into a message: {ev:?}"
        );
        let _ = o.stop_live();
        server.kill();
        crate::test_dir::fail_late();
    }

    /// Streams to a local mediamtx RTMP server, confirms receipt with
    /// ffprobe, then kills the server and checks that the app reconnects
    /// with backoff once it is back.
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "mediamtx is installed on the Linux runner only"
    )]
    fn streams_to_local_rtmp_and_reconnects_with_backoff() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let mtx = mediamtx_bin()
            .expect("mediamtx not found: set MEDIAMTX or install it at ~/opt/mediamtx/mediamtx");
        let port = 19350 + (std::process::id() % 500) as u16;
        let mut server = start_mediamtx(&mtx, port);
        let cfg = OutputConfig {
            rtmp_url: format!("rtmp://127.0.0.1:{}/live", port),
            width: 640,
            height: 360,
            fps: 30,
            video_kbps: 1200,
            retry_sec: 1,
            retry_max: 8,
            ..Default::default()
        };
        let mut o = Output::new(cfg, String::new());
        o.set_key("studio_test_key", false).unwrap();
        o.push_frame(moving_frame(0));
        o.start_live().unwrap();
        let feed = |o: &mut Output, secs: f64, events: &mut Vec<(ToastKind, String)>| {
            let t0 = Instant::now();
            let mut i = 0;
            while t0.elapsed().as_secs_f64() < secs {
                o.push_frame(moving_frame(i));
                i += 1;
                events.extend(o.tick());
                std::thread::sleep(Duration::from_millis(33));
            }
        };
        let mut ev = Vec::new();
        feed(&mut o, 3.0, &mut ev);
        let url = format!("rtmp://127.0.0.1:{}/live/studio_test_key", port);
        let p = probe(&ff, &url).unwrap();
        assert!(
            p.contains("codec_name=h264") && p.contains("width=640"),
            "server did not receive the stream: {}",
            p
        );
        assert!(
            o.status().kbps > 0.0 && o.status().fps > 0.0,
            "{:?}",
            o.status()
        );

        // drop the server: ffmpeg dies, a reconnect is scheduled with backoff
        server.kill();
        feed(&mut o, 4.0, &mut ev);
        assert!(
            ev.iter()
                .any(|(k, m)| *k == ToastKind::Warn && m.contains("Reconnecting in")),
            "{:?}",
            ev
        );
        assert!(o.reconnect.attempts() >= 1 && o.is_live());
        assert!(
            ev.iter().all(|(_, m)| !m.contains("studio_test_key")),
            "key leaked into a message"
        );

        // bring it back: the next attempt connects and the counter resets
        let mut server = start_mediamtx(&mtx, port);
        let t0 = Instant::now();
        while o.reconnect.attempts() > 0 && t0.elapsed() < Duration::from_secs(25) {
            feed(&mut o, 0.5, &mut ev);
        }
        assert_eq!(o.reconnect.attempts(), 0, "did not reconnect: {:?}", ev);
        assert!(ev
            .iter()
            .any(|(k, m)| *k == ToastKind::Success && m.contains("reconnected")));
        let p = probe(&ff, &url).unwrap();
        assert!(p.contains("codec_name=h264"), "after reconnect: {}", p);
        o.stop_live().unwrap();
        assert!(!o.is_live());
        server.kill();
        crate::test_dir::fail_late();
    }

    /// Goes through Output::start_record → Session::spawn → build_args, the
    /// path the Record button uses. ffmpeg is a path that doesn't exist, so
    /// nothing runs: if the refusal were missing, spawning would fail with
    /// "could not start ffmpeg" instead of the device message.
    #[test]
    fn record_refuses_capture_audio_without_a_usable_device() {
        let cfg = OutputConfig {
            audio: Audio::Device,
            ..Default::default()
        };
        let start = |backend: AudioBackend, device: &str| {
            let mut o = Output::new(
                OutputConfig {
                    audio_device: device.into(),
                    ..cfg.clone()
                },
                String::new(),
            );
            o.ffmpeg = Some(PathBuf::from("/nonexistent/seraphina-test-ffmpeg"));
            o.audio_backend = backend;
            let r = o.start_record(Some(std::env::temp_dir().join("seraphina_never.mp4")));
            assert!(o.rec.is_none());
            r
        };
        let e = start(AudioBackend::DirectShow, "  ").unwrap_err();
        assert!(e.contains("no DirectShow device is named"), "{e}");
        let e = start(AudioBackend::Unsupported, "x").unwrap_err();
        assert!(e.contains("isn't supported on this OS"), "{e}");
        // A named device and Pulse's `default` get as far as starting ffmpeg.
        for (b, d) in [(AudioBackend::DirectShow, "Mic"), (AudioBackend::Pulse, "")] {
            let e = start(b, d).unwrap_err();
            assert!(e.starts_with("could not start ffmpeg"), "{b:?}: {e}");
        }
        assert_eq!(
            AudioBackend::host(),
            if cfg!(windows) {
                AudioBackend::DirectShow
            } else if cfg!(target_os = "linux") {
                AudioBackend::Pulse
            } else {
                AudioBackend::Unsupported
            }
        );
    }

    fn pactl(args: &[&str]) -> std::process::Output {
        Command::new("pactl")
            .args(args)
            .output()
            .expect("pactl not found: install pulseaudio-utils")
    }

    /// A PulseAudio null sink this test loaded. Dropping it (also when the
    /// test fails) unloads it, so no sink is left on the audio server.
    struct NullSink(String);

    impl NullSink {
        fn load(name: &str) -> NullSink {
            let m = pactl(&[
                "load-module",
                "module-null-sink",
                &format!("sink_name={name}"),
            ]);
            assert!(m.status.success(), "load null sink: {m:?}");
            NullSink(String::from_utf8_lossy(&m.stdout).trim().to_string())
        }
    }

    impl Drop for NullSink {
        fn drop(&mut self) {
            let o = pactl(&["unload-module", &self.0]);
            if !o.status.success() {
                eprintln!("could not unload null sink module {}: {o:?}", self.0);
            }
        }
    }

    /// Fraction of the audio track's energy at `hz` (Goertzel on mono
    /// 48 kHz PCM decoded by ffmpeg; needs at least 2 s). A pure tone at `hz` gives about 1.0;
    /// silence or any other tone gives about 0.
    fn energy_share_at(ff: &Path, file: &Path, hz: f64) -> f64 {
        let o = Command::new(ff)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(file)
            .args([
                "-map", "0:a", "-ac", "1", "-ar", "48000", "-f", "s16le", "-",
            ])
            .output()
            .expect("decode audio with ffmpeg");
        let x: Vec<f64> = o
            .stdout
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| f64::from(i16::from_le_bytes(*b)))
            .collect();
        assert!(
            x.len() >= 2 * 48_000,
            "3 s recorded but only {:.2} s of audio decoded",
            x.len() as f64 / 48_000.0
        );
        let c = 2.0 * (2.0 * std::f64::consts::PI * hz / 48_000.0).cos();
        let (mut s1, mut s2) = (0.0, 0.0);
        for &v in &x {
            let s0 = v + c * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        let power = s1 * s1 + s2 * s2 - c * s1 * s2;
        let total: f64 = x.iter().map(|v| v * v).sum();
        if total == 0.0 {
            return 0.0;
        }
        2.0 * power / (x.len() as f64 * total)
    }

    /// Records with Audio = Capture device from a real PulseAudio source and
    /// checks the AAC track carries the signal played into it. The source is
    /// the monitor of a null sink this test creates; a 1234 Hz tone is played
    /// into the sink with paplay, i.e. through the audio server, never by
    /// the app. The recording must carry that tone: a silent or synthetic
    /// stand-in track has almost no energy at 1234 Hz. Fails without a
    /// PulseAudio server.
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "PulseAudio capture is the Linux path; DirectShow needs a real Windows device"
    )]
    fn records_audio_from_a_real_pulse_source() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        assert!(
            pactl(&["info"]).status.success(),
            "needs a running PulseAudio server (pulseaudio --start)"
        );
        let sink = format!("seraphina_mic_test_{}", std::process::id());
        let _sink = NullSink::load(&sink);
        let tmp = crate::test_dir::TestDir::new("mic");
        let dir = tmp.path().to_path_buf();
        let tone = dir.join("tone.wav");
        assert!(Command::new(&ff)
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=1234:sample_rate=48000:duration=8")
            .arg("-y")
            .arg(&tone)
            .status()
            .is_ok_and(|s| s.success()));
        let mut play = Command::new("paplay")
            .arg(format!("--device={sink}"))
            .arg(&tone)
            .spawn()
            .expect("paplay not found: install pulseaudio-utils");
        let cfg = OutputConfig {
            width: 320,
            height: 180,
            audio: Audio::Device,
            audio_device: format!("{sink}.monitor"),
            ..Default::default()
        };
        let mut o = Output::new(cfg, String::new());
        let path = dir.join("mic.mp4");
        o.start_record(Some(path.clone())).expect("start record");
        let mut ev = Vec::new();
        let t0 = Instant::now();
        let mut i = 0;
        while t0.elapsed() < Duration::from_secs(3) {
            o.push_frame(moving_frame(i));
            i += 1;
            ev.extend(o.tick());
            std::thread::sleep(Duration::from_millis(33));
        }
        let msg = o.stop_record().expect("stop record");
        let _ = play.kill();
        let _ = play.wait();
        assert!(msg.contains("saved"), "{msg} {ev:?}");
        assert!(ev.iter().all(|(k, _)| *k != ToastKind::Error), "{ev:?}");
        let p = probe(&ff, &path.to_string_lossy()).expect("ffprobe");
        assert!(p.contains("codec_name=aac"), "no audio track: {p}");
        let share = energy_share_at(&ff, &path, 1234.0);
        assert!(
            share > 0.5,
            "only {share:.3} of the captured energy is the 1234 Hz tone played into the source"
        );
        crate::test_dir::fail_late();
    }

    /// A capture device that doesn't exist must fail loudly: an error
    /// toast and a status error, not a recording with a stand-in track.
    #[test]
    #[cfg_attr(
        not(target_os = "linux"),
        ignore = "PulseAudio capture is the Linux path; DirectShow needs a real Windows device"
    )]
    fn a_missing_audio_device_fails_loudly() {
        find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let cfg = OutputConfig {
            width: 320,
            height: 180,
            audio: Audio::Device,
            audio_device: "seraphina_no_such_source".into(),
            ..Default::default()
        };
        let mut o = Output::new(cfg, String::new());
        let tmp = crate::test_dir::TestDir::new("nomic");
        let path = tmp.path().join("nomic.mp4");
        o.start_record(Some(path.clone())).expect("ffmpeg starts");
        let mut ev = Vec::new();
        let t0 = Instant::now();
        while o.rec.is_some() && t0.elapsed() < Duration::from_secs(8) {
            o.push_frame(moving_frame(0));
            ev.extend(o.tick());
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(o.rec.is_none(), "recording kept running without its device");
        assert!(
            ev.iter()
                .any(|(k, m)| *k == ToastKind::Error && m.starts_with("Recording stopped")),
            "{ev:?}"
        );
        assert!(o.status().error.is_some(), "{:?}", o.status());
        crate::test_dir::fail_late();
    }
}

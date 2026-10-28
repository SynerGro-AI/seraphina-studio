//! Real recording and live streaming through an `ffmpeg` subprocess.
//!
//! * Canvas source: the active mode's composite is letterboxed to the output
//!   size and piped as raw RGBA frames to ffmpeg's stdin at a fixed rate. A
//!   tick that cannot be written in time is counted as a dropped frame.
//! * Screen source: ffmpeg grabs the desktop itself (x11grab; on Windows
//!   ddagrab, the DPI-correct Desktop Duplication grabber, with gdigrab as
//!   the fallback).
//! * Encode: libx264 (veryfast, zerolatency, yuv420p) + AAC (silent track or a
//!   capture device). Record writes MP4, live pushes FLV to RTMP.
//! * `-progress pipe:1` is parsed for fps / bitrate / encoded time / drops.
//! * Live sessions that die are restarted with exponential backoff
//!   (`obs_rust_bitstream::Reconnect`).
//!
//! The stream key lives in its own file (0600 on Unix), is never put in the
//! AI context, logs or error text (ffmpeg output is scrubbed), and the UI only
//! shows it masked unless the user asks to reveal it.

use crate::prefs::seraphina_home;
use crate::ToastKind;
use obs_rust_bitstream::ui_canvas::Raster;
use obs_rust_bitstream::Reconnect;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Canvas,
    Screen,
}

impl Source {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_lowercase().as_str() {
            "canvas" | "composite" | "app" => Some(Source::Canvas),
            "screen" | "desktop" | "display" => Some(Source::Screen),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Source::Canvas => "Canvas",
            Source::Screen => "Screen",
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
    pub record_dir: String,
    pub retry_sec: u32,
    pub retry_max: u32,
}

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
            record_dir: studio_dir()
                .join("recordings")
                .to_string_lossy()
                .into_owned(),
            retry_sec: 2,
            retry_max: 10,
        }
    }
}

pub fn studio_dir() -> PathBuf {
    seraphina_home().join("studio")
}

impl OutputConfig {
    pub fn path() -> PathBuf {
        studio_dir().join("output.json")
    }
    /// Settings and, if output.json was damaged, a message saying where
    /// the damaged copy was kept.
    pub fn load_from(path: &std::path::Path) -> (Self, Option<String>) {
        crate::project::load_json(path, "output settings file")
    }
    pub fn save_to(&self, path: &std::path::Path) -> Result<(), String> {
        let s = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        crate::project::write_atomic(path, s.as_bytes()).map_err(|e| e.to_string())
    }
    pub fn save(&self) -> Result<(), String> {
        self.save_to(&Self::path())
    }
    /// Even dimensions (yuv420p), sane bounds.
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
    let mut a: Vec<String> = [
        "-hide_banner",
        "-loglevel",
        "warning",
        "-nostats",
        "-progress",
        "pipe:1",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let mut push = |xs: &[&str]| a.extend(xs.iter().map(|s| s.to_string()));
    match cfg.source {
        Source::Canvas => {
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
        if cfg.source == Source::Canvas {
            push(&["-shortest"]);
        }
    }
    match kind {
        Kind::Record => push(&["-movflags", "+faststart", "-f", "mp4", "-y", target]),
        Kind::Live => push(&["-f", "flv", target]),
    }
    Ok(a)
}

#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Frames written into ffmpeg's stdin.
    pub frames_in: u64,
    /// Ticks skipped because the pipe/encoder was behind.
    pub dropped: u64,
    /// ffmpeg's own counters.
    pub enc_frames: u64,
    pub ffmpeg_drops: u64,
    pub fps: f32,
    pub kbps: f32,
    pub out_time: f64,
    pub total_bytes: u64,
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
        if let Some(out) = child.stdout.take() {
            let st = stats.clone();
            std::thread::spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Ok(mut s) = st.lock() {
                        parse_progress(&line, &mut s);
                    }
                }
            });
        }
        if let Some(err) = child.stderr.take() {
            let st = stats.clone();
            let secret = secret.to_string();
            std::thread::spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
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
            let mut next = Instant::now();
            let mut cache: Option<(usize, Vec<u8>)> = None;
            let black = letterbox(None, w, h);
            while !stp.load(Ordering::Relaxed) {
                let now = Instant::now();
                if now < next {
                    std::thread::sleep(next - now);
                } else {
                    let missed = ((now - next).as_secs_f64() / period.as_secs_f64()) as u32;
                    if missed > 0 {
                        if let Ok(mut s) = st.lock() {
                            s.dropped += missed as u64;
                        }
                        next += period * missed;
                    }
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
                if stdin.write_all(buf).is_err() {
                    if let Ok(mut s) = st.lock() {
                        if s.last_error.is_none() {
                            s.last_error = Some("ffmpeg stopped accepting frames".into());
                        }
                    }
                    break;
                }
                if let Ok(mut s) = st.lock() {
                    s.frames_in += 1;
                }
                next += period;
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

    /// `Some(success)` once ffmpeg has exited.
    pub fn exited(&mut self) -> Option<bool> {
        match self.child.try_wait() {
            Ok(Some(st)) => {
                self.finished = true;
                Some(st.success())
            }
            Ok(None) => None,
            Err(_) => Some(false),
        }
    }

    /// Graceful stop: end the frame pipe (or send `q`), wait for ffmpeg to
    /// finalise, kill it only after `timeout`.
    pub fn stop(&mut self, timeout: Duration) -> Result<(), String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.writer.take() {
            let _ = w.join();
        }
        let t0 = Instant::now();
        loop {
            if let Some(ok) = self.exited() {
                return if ok {
                    Ok(())
                } else {
                    Err(self
                        .stats()
                        .last_error
                        .unwrap_or_else(|| "ffmpeg exited with an error".into()))
                };
            }
            if t0.elapsed() > timeout {
                let _ = self.child.kill();
                let _ = self.child.wait();
                self.finished = true;
                return Err("ffmpeg did not finish in time and was killed".into());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
    }

    /// Hard kill (the test "drop connection" button).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
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
    pub dropped: u64,
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
}

impl Output {
    pub fn new(cfg: OutputConfig, key: String) -> Self {
        crate::crash::register_secret(&key);
        let mut reconnect = Reconnect::new();
        reconnect.set_settings(cfg.retry_max, cfg.retry_sec);
        Self {
            cfg,
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
        }
    }

    pub fn load() -> Self {
        let (cfg, warning) = OutputConfig::load_from(&OutputConfig::path());
        let mut o = Self::new(cfg, load_key());
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
        self.cfg.save()
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
        let s = Session::spawn(
            &ff,
            &self.cfg,
            Kind::Record,
            &path.to_string_lossy(),
            "",
            self.frames.clone(),
            self.audio_backend,
        )?;
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
        let s = Session::spawn(
            &ff,
            &self.cfg,
            Kind::Live,
            &target,
            &self.key,
            self.frames.clone(),
            self.audio_backend,
        )?;
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
                Ok("stream ended".into())
            }
            None => Ok("already offline".into()),
        }
    }

    /// Retry now: reconnect immediately if a reconnect is pending or the
    /// stream gave up; restart the stream if it is up but unhealthy.
    pub fn retry(&mut self) -> Result<String, String> {
        if self.live.is_some() && self.reconnect_at.is_none() {
            let healthy = self
                .live
                .as_ref()
                .map(|s| s.stats().out_time > 0.0)
                .unwrap_or(false);
            if healthy {
                return Ok("stream is connected; nothing to retry".into());
            }
            if let Some(mut s) = self.live.take() {
                s.kill();
            }
        } else if self.live.is_none() && self.reconnect_at.is_none() && self.live_error.is_none() {
            return Err("not live; use Go live".into());
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
            } else if self.reconnect.attempts() > 0 && s.stats().out_time > 2.0 {
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
        std::mem::take(&mut self.events)
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
            st.dropped += x.dropped + x.ffmpeg_drops;
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
            "Output: ffmpeg {}, source {}, {}x{} @ {} fps, audio {}; recording {}; live {}; RTMP URL {}, stream key {}",
            if self.ffmpeg.is_some() { "found" } else if self.ffmpeg_error.is_some() { "missing" } else { "not checked" },
            self.cfg.source.label(),
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
                &["Canvas", "Screen"],
                if self.cfg.source == Source::Canvas {
                    0
                } else {
                    1
                },
                90.0,
            ) {
                bus.push(AppCommand::SetOutputSource {
                    source: if i == 0 { "canvas" } else { "screen" }.into(),
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
            if st.recording.is_some() || st.live.is_some() {
                theme::kv(
                    ui,
                    p,
                    "Encoder",
                    format!("{:.0} fps · {:.0} kb/s", st.fps, st.kbps),
                );
                theme::kv(ui, p, "Dropped frames", format!("{}", st.dropped));
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
                        self.is_live() || self.live_error.is_some(),
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
            });
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
        let path =
            std::env::temp_dir().join(format!("seraphina_rec_test_{}.mp4", std::process::id()));
        let _ = std::fs::remove_file(&path);
        o.start_record(Some(path.clone())).unwrap();
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
        if let Ok(out) = std::env::var("RECORD_TEST_OUT") {
            std::fs::copy(&path, out).unwrap();
        }
        let _ = std::fs::remove_file(&path);
    }

    /// Goes live against `ffmpeg -listen 1` acting as the RTMP server: the
    /// listener writes what it receives to an FLV, which ffprobe must read
    /// back as H.264 at the configured size. Needs only ffmpeg; no network
    /// beyond 127.0.0.1.
    #[test]
    #[cfg_attr(windows, ignore = "the Windows runners and Wine have no ffmpeg.exe")]
    fn goes_live_to_an_ffmpeg_rtmp_listener() {
        let ff = find_ffmpeg().expect("ffmpeg not found: install it or set SERAPHINA_FFMPEG");
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a free port");
            l.local_addr().expect("local addr").port()
        };
        let got =
            std::env::temp_dir().join(format!("seraphina_rtmp_rx_{}.flv", std::process::id()));
        let _ = std::fs::remove_file(&got);
        let mut server = Command::new(&ff)
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
        let hex = format!(":{port:04X} ");
        let t0 = Instant::now();
        loop {
            match std::fs::read_to_string("/proc/net/tcp") {
                Ok(t) if t.lines().any(|l| l.contains(&hex) && l.contains(" 0A ")) => break,
                Ok(_) => {}
                Err(_) => {
                    std::thread::sleep(Duration::from_secs(1));
                    break;
                }
            }
            assert!(
                t0.elapsed() < Duration::from_secs(10),
                "listener never came up"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
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
        let _ = std::fs::remove_file(&got);
    }

    fn mediamtx_bin() -> Option<PathBuf> {
        let c = std::env::var_os("MEDIAMTX")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join("opt/mediamtx/mediamtx"))
            })?;
        c.is_file().then_some(c)
    }

    fn start_mediamtx(bin: &Path, port: u16) -> Child {
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
    fn spawn_mediamtx(bin: &Path, tag: &str, settings: &str) -> Child {
        let dir = std::env::temp_dir().join(format!("mtx_{}_{tag}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mediamtx config dir");
        let yml = dir.join("mediamtx.yml");
        std::fs::write(&yml, format!("logLevel: warn\nmetrics: no\npprof: no\nplayback: no\nrtsp: no\nhls: no\nwebrtc: no\nsrt: no\n{settings}")).expect("write mediamtx.yml");
        Command::new(bin)
            .arg(&yml)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn mediamtx")
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
        let _ = server.kill();
        let _ = server.wait();
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
        let _ = server.kill();
        let _ = server.wait();
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
        let _ = server.kill();
        let _ = server.wait();
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
        let m = pactl(&[
            "load-module",
            "module-null-sink",
            &format!("sink_name={sink}"),
        ]);
        assert!(m.status.success(), "load null sink: {m:?}");
        let module = String::from_utf8_lossy(&m.stdout).trim().to_string();
        let dir = std::env::temp_dir().join(format!("seraphina_mic_{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
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
        let _ = pactl(&["unload-module", &module]);
        assert!(msg.contains("saved"), "{msg} {ev:?}");
        assert!(ev.iter().all(|(k, _)| *k != ToastKind::Error), "{ev:?}");
        let p = probe(&ff, &path.to_string_lossy()).expect("ffprobe");
        assert!(p.contains("codec_name=aac"), "no audio track: {p}");
        let share = energy_share_at(&ff, &path, 1234.0);
        assert!(
            share > 0.5,
            "only {share:.3} of the captured energy is the 1234 Hz tone played into the source"
        );
        let _ = std::fs::remove_dir_all(&dir);
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
        let path = std::env::temp_dir().join(format!("seraphina_nomic_{}.mp4", std::process::id()));
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
        let _ = std::fs::remove_file(&path);
    }
}

//! M3/D1: crash logs.
//!
//! A panic hook writes `crash-<UTC time>-<pid>.txt` (`-2`, `-3`, ... for more
//! reports in the same second, so none is overwritten) to
//! `%APPDATA%\SeraphinaStudio\logs` (Windows; `~/.local/share/SeraphinaStudio/logs`
//! elsewhere, `SERAPHINA_LOG_DIR` overrides) and, for a panic on the main
//! thread, shows a native message box, because the release exe has no console.
//! The next launch lists reports the user hasn't seen yet.
//!
//! Secrets (the stream key, the xAI key) are registered with
//! [`register_secret`] and scrubbed from every report, as is anything shaped
//! like an xAI key.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

/// Set while a renderer is being tried: a panic then is a graphics-init
/// failure that the renderer fallback handles, so no message box.
pub static GPU_INIT: AtomicBool = AtomicBool::new(false);
/// Set once the first frame has been drawn.
pub static FIRST_FRAME: AtomicBool = AtomicBool::new(false);

/// `--crash-test`: panic on the first frame (to check the hook on a real PC).
pub static CRASH_TEST: AtomicBool = AtomicBool::new(false);

static SECRETS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static RENDERER: Mutex<String> = Mutex::new(String::new());

const KEEP: usize = 20;

pub fn logs_dir() -> PathBuf {
    if let Ok(d) = std::env::var("SERAPHINA_LOG_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d.trim());
        }
    }
    // dirs::data_dir() is FOLDERID_RoamingAppData (%APPDATA%) on Windows
    dirs::data_dir()
        .unwrap_or_else(crate::prefs::seraphina_home)
        .join("SeraphinaStudio")
        .join("logs")
}

/// Never let this value reach a log file.
pub fn register_secret(s: &str) {
    let s = s.trim();
    if s.len() < 4 {
        return;
    }
    if let Ok(mut v) = SECRETS.lock() {
        if !v.iter().any(|x| x == s) {
            v.push(s.to_string());
        }
    }
}

pub fn set_renderer(name: &str) {
    if let Ok(mut r) = RENDERER.lock() {
        *r = name.to_string();
    }
}

static FALLBACK_NOTE: Mutex<Option<String>> = Mutex::new(None);

/// Why the first renderer didn't start (shown once as a toast).
pub fn set_fallback_note(why: &str) {
    if let Ok(mut n) = FALLBACK_NOTE.lock() {
        *n = Some(redact(why));
    }
}

pub fn take_fallback_note() -> Option<String> {
    FALLBACK_NOTE.lock().ok().and_then(|mut n| n.take())
}

pub fn renderer() -> String {
    RENDERER.lock().map(|r| r.clone()).unwrap_or_default()
}

/// Remove registered secrets and anything that looks like an xAI key.
pub fn redact(text: &str) -> String {
    let mut out = text.to_string();
    // try_lock: the hook must never deadlock if a panic hit while it was held
    if let Ok(v) = SECRETS.try_lock() {
        for s in v.iter() {
            out = out.replace(s.as_str(), "****");
        }
    }
    let mut res = String::with_capacity(out.len());
    let mut rest = out.as_str();
    while let Some(i) = rest.find("xai-") {
        res.push_str(&rest[..i]);
        let tail = &rest[i + 4..];
        let n = tail
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .unwrap_or(tail.len());
        if n >= 8 {
            res.push_str("xai-****");
        } else {
            res.push_str(&rest[i..i + 4 + n]);
        }
        rest = &tail[n..];
    }
    res.push_str(rest);
    res
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Write one report; returns its path. `prefix` is `crash` for anything
/// shown on the next launch.
pub fn write_report_in(
    dir: &Path,
    prefix: &str,
    what: &str,
    detail: &str,
) -> std::io::Result<PathBuf> {
    write_report_at(dir, prefix, what, detail, now_secs())
}

/// `write_report_in` at a given time. A report already there for the same
/// second (a background panic and then the main-thread panic it causes,
/// or the gpu report and then the crash report) is never overwritten: the
/// new one is named `-2`, `-3`, ...
fn write_report_at(
    dir: &Path,
    prefix: &str,
    what: &str,
    detail: &str,
    secs: u64,
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let stamp = crate::output::utc_stamp(secs);
    let base = format!("{}-{}-{}", prefix, stamp, std::process::id());
    // Start after the highest number already used this second, so a name
    // freed by pruning is never reused (it would sort as the oldest).
    let mut n = next_number(dir, &base);
    let (path, mut f) = loop {
        let path = dir.join(if n == 1 {
            format!("{base}.txt")
        } else {
            format!("{base}-{n}.txt")
        });
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => break (path, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => n += 1,
            Err(e) => return Err(e),
        }
    };
    let body = format!(
        "Seraphina Studio {} crash report\nTime: {}\nOS: {} {}\nRenderer: {}\nThread: {}\n\n{}\n\n{}\n",
        env!("CARGO_PKG_VERSION"),
        stamp,
        std::env::consts::OS,
        std::env::consts::ARCH,
        {
            let r = renderer();
            if r.is_empty() { "not started".to_string() } else { r }
        },
        std::thread::current().name().unwrap_or("unnamed"),
        what,
        detail
    );
    f.write_all(redact(&body).as_bytes())?;
    prune(dir);
    Ok(path)
}

pub fn write_report(prefix: &str, what: &str, detail: &str) -> std::io::Result<PathBuf> {
    write_report_in(&logs_dir(), prefix, what, detail)
}

/// One more than the highest same-second number among `dir`'s reports
/// named `<base>.txt`, `<base>-<n>.txt` (and their `.seen` forms); 1 if none.
fn next_number(dir: &Path, base: &str) -> u32 {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return 1;
    };
    rd.filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(base))
                .is_some_and(|rest| rest.starts_with('-') || rest.starts_with('.'))
        })
        .map(|p| age_key(&p).1 + 1)
        .max()
        .unwrap_or(1)
}

/// Oldest first, whatever the prefix: by the time in the name, then the
/// same-second number (none, `-2`, `-3`, ... `-10`). Names without a time
/// sort by name, before the timed ones.
fn age_key(p: &Path) -> (String, u32, String) {
    let name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_string();
    let base = name.trim_end_matches(".txt").trim_end_matches(".seen");
    let rest = base.split_once('-').map_or("", |(_, r)| r);
    let Some((stamp, tail)) = rest.split_once('Z') else {
        return (String::new(), 0, name);
    };
    // tail is "-<pid>" or "-<pid>-<n>"
    let n = tail
        .trim_start_matches('-')
        .split_once('-')
        .and_then(|(_, n)| n.parse().ok())
        .unwrap_or(1);
    (stamp.to_string(), n, name)
}

fn sort_by_age(v: &mut [PathBuf]) {
    v.sort_by_cached_key(|p| age_key(p));
}

/// Keep the newest `KEEP` reports.
fn prune(dir: &Path) {
    let mut v: Vec<PathBuf> = list(dir, |_| true);
    if v.len() <= KEEP {
        return;
    }
    sort_by_age(&mut v);
    for p in &v[..v.len() - KEEP] {
        let _ = std::fs::remove_file(p);
    }
}

fn list(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.ends_with(".txt") && keep(n))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Crash reports not yet acknowledged, newest first.
pub fn unseen_in(dir: &Path) -> Vec<PathBuf> {
    let mut v = list(dir, |n| {
        n.starts_with("crash-") && !n.ends_with(".seen.txt")
    });
    sort_by_age(&mut v);
    v.reverse();
    v
}

/// Mark reports as seen (renamed to `*.seen.txt`; they stay in the folder).
pub fn mark_seen(paths: &[PathBuf]) {
    for p in paths {
        let seen = p.with_extension("seen.txt");
        let _ = std::fs::rename(p, seen);
    }
}

fn payload_text(info: &std::panic::PanicHookInfo<'_>) -> String {
    let p = info.payload();
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "(non-text panic payload)".into()
    }
}

/// Install the panic hook (keeps the default stderr output too).
pub fn install_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = payload_text(info);
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown location".into());
        let main = std::thread::current().name() == Some("main");
        let gpu = GPU_INIT.load(Ordering::SeqCst);
        let bt = std::backtrace::Backtrace::force_capture();
        let what = format!(
            "Panic{}: {}\nAt: {}",
            if gpu {
                " while starting the renderer"
            } else if main {
                ""
            } else {
                " on a background thread (the app may have kept running)"
            },
            msg,
            loc
        );
        let written = write_report(
            if gpu { "gpu" } else { "crash" },
            &what,
            &format!("Backtrace:\n{}", bt),
        );
        default(info);
        if main && !gpu {
            let where_ = match &written {
                Ok(p) => format!("A crash report was saved to:\n{}", p.display()),
                Err(e) => format!("The crash report couldn't be saved ({}).", e),
            };
            message_box(
                "Seraphina Studio has stopped",
                &format!(
                    "Seraphina Studio hit an unexpected error and has to close.\n\n{}\n\n{}\n\nAutosaved work can be recovered on the next launch.",
                    redact(&msg).chars().take(400).collect::<String>(),
                    where_
                ),
            );
        }
    }));
}

/// Native error box (Windows); stderr elsewhere.
pub fn message_box(title: &str, text: &str) {
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            MessageBoxW, MB_ICONERROR, MB_OK, MB_SETFOREGROUND,
        };
        let w = |s: &str| {
            s.encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<u16>>()
        };
        let (t, c) = (w(text), w(title));
        // SAFETY: both buffers are NUL-terminated UTF-16 and outlive the call
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                t.as_ptr(),
                c.as_ptr(),
                MB_OK | MB_ICONERROR | MB_SETFOREGROUND,
            );
        }
    }
    #[cfg(not(windows))]
    eprintln!("{}: {}", title, text);
}

/// Shown once on the next launch.
pub struct CrashNotice {
    pub reports: Vec<PathBuf>,
}

impl CrashNotice {
    pub fn check() -> Option<Self> {
        let reports = unseen_in(&logs_dir());
        (!reports.is_empty()).then_some(Self { reports })
    }

    /// First lines of the newest report (already redacted on write).
    pub fn summary(&self) -> String {
        self.reports
            .first()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .map(|s| {
                s.lines()
                    .skip_while(|l| !l.starts_with("Panic") && !l.starts_with("Startup"))
                    .take(2)
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }
}

/// Open a folder in the OS file manager.
pub fn open_folder(dir: &Path) -> std::io::Result<()> {
    let prog = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(prog)
        .arg(dir)
        .spawn()
        .map(|_| ())
}

// ---- next-launch notice -------------------------------------------------------

use eframe::egui;

impl crate::App {
    /// "Seraphina Studio closed unexpectedly" (after any recovery prompt).
    pub(crate) fn crash_modal(&mut self, ctx: &egui::Context) {
        if self.project.prompt.is_some() || self.project.dialog.is_some() {
            return;
        }
        let Some(n) = &self.crash_notice else {
            return;
        };
        let p = self.pal;
        let summary = n.summary();
        let newest = n.reports.first().cloned();
        let count = n.reports.len();
        let mut close = false;
        egui::Modal::new(egui::Id::new("crash-notice"))
            .frame(
                egui::Frame::NONE
                    .fill(p.panel)
                    .stroke(egui::Stroke::new(1.0_f32, p.line))
                    .corner_radius(egui::CornerRadius::same(crate::theme::R_LG as u8))
                    .inner_margin(egui::Margin::same(crate::theme::SP_LG as i8)),
            )
            .show(ctx, |ui| {
                ui.set_width(480.0);
                ui.label(
                    egui::RichText::new("Seraphina Studio closed unexpectedly last time")
                        .size(16.0)
                        .strong()
                        .color(p.text),
                );
                ui.add_space(crate::theme::SP_SM);
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(if count > 1 {
                            format!("{} crash reports were saved on this PC. Nothing was sent anywhere; you can attach a report to a bug report.", count)
                        } else {
                            "A crash report was saved on this PC. Nothing was sent anywhere; you can attach it to a bug report.".to_string()
                        })
                        .color(p.muted),
                    )
                    .wrap(),
                );
                if !summary.is_empty() {
                    ui.add_space(crate::theme::SP_SM);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(summary.chars().take(300).collect::<String>())
                                .monospace()
                                .color(p.text),
                        )
                        .wrap(),
                    );
                }
                if let Some(f) = &newest {
                    ui.add_space(crate::theme::SP_SM);
                    ui.add(egui::Label::new(crate::theme::muted(&p, &f.display().to_string())).wrap());
                }
                ui.add_space(crate::theme::SP_MD);
                ui.horizontal(|ui| {
                    if ui.add(crate::theme::primary_button(&p, "OK")).clicked() {
                        close = true;
                    }
                    if ui.button("Open logs folder").clicked() {
                        if let Err(e) = open_folder(&logs_dir()) {
                            self.toasts.push(
                                crate::ToastKind::Error,
                                &format!("Couldn't open the logs folder: {}", e),
                            );
                        }
                    }
                });
            });
        if close {
            if let Some(n) = self.crash_notice.take() {
                mark_seen(&n.reports);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_registered_secrets_and_xai_keys() {
        register_secret("live_987654_streamsecret");
        let s =
            redact("url rtmp://x/app/live_987654_streamsecret key xai-AbC123def456GHI789 xai-ok");
        assert!(!s.contains("streamsecret"));
        assert!(!s.contains("AbC123def456"));
        assert!(s.contains("xai-****") && s.contains("xai-ok"));
    }

    #[test]
    fn reports_are_written_listed_and_marked_seen() {
        let tmp = crate::test_dir::TestDir::new("crash");
        let dir = tmp.path();
        let p = write_report_in(
            dir,
            "crash",
            "Panic: boom xai-SECRETSECRET123",
            "Backtrace:\n…",
        )
        .unwrap();
        let body = std::fs::read_to_string(&p).unwrap();
        assert!(body.contains("Panic: boom") && !body.contains("SECRETSECRET"));
        write_report_in(dir, "gpu", "Panic while starting the renderer: x", "").unwrap();
        let un = unseen_in(dir);
        assert_eq!(un, vec![p.clone()], "gpu-* reports are not crash notices");
        mark_seen(&un);
        assert!(unseen_in(dir).is_empty());
        assert!(p.with_extension("seen.txt").exists());
        for i in 0..(KEEP + 5) {
            std::fs::write(dir.join(format!("crash-0000-{:03}.seen.txt", i)), "x").unwrap();
        }
        prune(dir);
        assert_eq!(list(dir, |_| true).len(), KEEP);
        crate::test_dir::fail_late();
    }

    /// Reports in the same second (a background panic and then the
    /// main-thread panic it causes; the gpu report and then the crash
    /// report) are all kept: the later ones are named `-2`, `-3`, and
    /// none overwrites another.
    #[test]
    fn reports_in_the_same_second_are_all_kept() {
        let tmp = crate::test_dir::TestDir::new("crash_same_second");
        let dir = tmp.path();
        let secs = 1_793_000_000;
        let base = format!("{}-{}", crate::output::utc_stamp(secs), std::process::id());
        let gpu =
            write_report_at(dir, "gpu", "Panic while starting the renderer: a", "", secs).unwrap();
        let first =
            write_report_at(dir, "crash", "Panic on a background thread: b", "", secs).unwrap();
        let second = write_report_at(dir, "crash", "Panic: c", "", secs).unwrap();
        let third = write_report_at(dir, "crash", "Panic: d", "", secs).unwrap();
        assert_eq!(gpu, dir.join(format!("gpu-{base}.txt")));
        assert_eq!(first, dir.join(format!("crash-{base}.txt")));
        assert_eq!(second, dir.join(format!("crash-{base}-2.txt")));
        assert_eq!(third, dir.join(format!("crash-{base}-3.txt")));
        for (p, what) in [
            (&gpu, "Panic while starting the renderer: a"),
            (&first, "Panic on a background thread: b"),
            (&second, "Panic: c"),
            (&third, "Panic: d"),
        ] {
            let body = std::fs::read_to_string(p).unwrap();
            assert!(body.contains(what), "{}: {body}", p.display());
        }
        assert_eq!(list(dir, |_| true).len(), 4);
        assert_eq!(
            unseen_in(dir),
            vec![third, second, first],
            "newest first, the same-second number counts"
        );
    }

    /// Pruning keeps the `KEEP` newest reports by their time and
    /// same-second number, whatever their prefix: newer `crash-*` reports
    /// outlive older `usage-*` and `gpu-*` ones, and within one second
    /// `-10` is newer than `-2` (the cut falls inside that second).
    #[test]
    fn pruning_keeps_the_newest_reports_whatever_their_prefix() {
        let tmp = crate::test_dir::TestDir::new("crash_prune_age");
        let dir = tmp.path();
        let t0 = 1_793_000_000;
        let mut made = Vec::new();
        for (i, prefix) in ["usage", "usage", "gpu", "gpu"].into_iter().enumerate() {
            made.push(write_report_at(dir, prefix, "older", "", t0 + i as u64).unwrap());
        }
        for _ in 0..22 {
            made.push(write_report_at(dir, "crash", "newest second", "", t0 + 4).unwrap());
        }
        assert!(made[made.len() - 1].to_string_lossy().ends_with("-22.txt"));
        let mut left = list(dir, |_| true);
        left.sort();
        let mut want = made[made.len() - KEEP..].to_vec();
        want.sort();
        assert_eq!(left, want, "the {KEEP} newest are kept");
    }
}

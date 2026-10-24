//! `.seraphina` project files: a zip holding `manifest.json` (format
//! name, version, every mode's state as JSON) plus raw binary assets
//! (RGBA rasters, masks, the inspected bitstream). Writes are atomic
//! (temp file in the same directory, fsync, rename), so a crash or a full
//! disk mid-save never damages the previous copy.
//!
//! Autosave writes the same format to the per-user data directory
//! (`%APPDATA%\Seraphina\autosave` on Windows, `$XDG_DATA_HOME/Seraphina/
//! autosave` on Linux) every two minutes. A session marker in that
//! directory is removed on a clean exit; finding it at launch means the
//! last session crashed and its autosave is offered for recovery.

use obs_rust_bitstream::ui_canvas::Raster;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const FORMAT: &str = "seraphina-project";
/// Bump when the manifest layout changes, and teach `migrate` the step.
pub const FORMAT_VERSION: u32 = 1;
pub const EXT: &str = "seraphina";
pub const MANIFEST: &str = "manifest.json";
pub const AUTOSAVE_FILE: &str = "autosave.seraphina";
pub const SESSION_MARKER: &str = "session.lock";
/// Autosave interval in seconds.
pub const AUTOSAVE_SECS: f64 = 120.0;

// ---- assets ---------------------------------------------------------------

/// A raster stored as a binary asset (`width * height * 4` RGBA bytes).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RasterRef {
    pub width: u32,
    pub height: u32,
    pub asset: String,
}

/// Binary payloads referenced from the manifest by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Assets {
    pub files: BTreeMap<String, Vec<u8>>,
}

impl Assets {
    pub fn put(&mut self, stem: &str, bytes: Vec<u8>) -> String {
        let clean: String = stem
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .take(40)
            .collect();
        let name = format!("assets/{:04}-{}.bin", self.files.len(), clean);
        self.files.insert(name.clone(), bytes);
        name
    }

    pub fn get(&self, name: &str) -> Result<&[u8], String> {
        self.files
            .get(name)
            .map(|v| v.as_slice())
            .ok_or_else(|| format!("project is missing asset '{}'", name))
    }

    pub fn put_raster(&mut self, stem: &str, r: &Raster) -> RasterRef {
        RasterRef {
            width: r.width,
            height: r.height,
            asset: self.put(stem, r.data.clone()),
        }
    }

    pub fn raster(&self, r: &RasterRef) -> Result<Raster, String> {
        let d = self.get(&r.asset)?;
        Raster::from_rgba(r.width, r.height, d.to_vec()).ok_or_else(|| {
            format!(
                "asset '{}' is {} bytes, expected {}×{} RGBA",
                r.asset,
                d.len(),
                r.width,
                r.height
            )
        })
    }

    /// An 8-bit plane of `w * h` bytes.
    pub fn plane(&self, name: &str, w: u32, h: u32) -> Result<Vec<u8>, String> {
        let d = self.get(name)?;
        if d.len() != (w as usize) * (h as usize) {
            return Err(format!(
                "asset '{}' is {} bytes, expected {}×{}",
                name,
                d.len(),
                w,
                h
            ));
        }
        Ok(d.to_vec())
    }
}

// ---- container ------------------------------------------------------------

/// A decoded project: the manifest JSON and its assets.
#[derive(Clone, Debug, PartialEq)]
pub struct Container {
    pub manifest: Value,
    pub assets: Assets,
}

/// Header fields every manifest carries.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Header {
    pub format: String,
    pub format_version: u32,
    #[serde(default)]
    pub app_version: String,
    #[serde(default)]
    pub saved_at: u64,
}

pub fn header() -> Header {
    Header {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        saved_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    }
}

/// Upgrade an older manifest in place to `FORMAT_VERSION`. Each step
/// rewrites JSON from version N to N+1; files from a newer app are refused.
pub fn migrate(mut m: Value) -> Result<Value, String> {
    let format = m.get("format").and_then(|v| v.as_str()).unwrap_or("");
    if format != FORMAT {
        return Err("not a Seraphina project (manifest format mismatch)".into());
    }
    let mut v = m
        .get("format_version")
        .and_then(|v| v.as_u64())
        .ok_or("manifest has no format_version")? as u32;
    if v > FORMAT_VERSION {
        return Err(format!(
            "this project was saved by a newer Seraphina Studio (format {}, this build reads up to {})",
            v, FORMAT_VERSION
        ));
    }
    while v < FORMAT_VERSION {
        // No older formats exist yet; future steps go here:
        // 1 => { /* rename or add fields */ }
        #[allow(clippy::match_single_binding)]
        match v {
            _ => {}
        }
        v += 1;
    }
    m["format_version"] = Value::from(FORMAT_VERSION);
    Ok(m)
}

pub fn encode(c: &Container) -> Result<Vec<u8>, String> {
    let mut buf = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(3));
        let json = serde_json::to_vec_pretty(&c.manifest).map_err(|e| e.to_string())?;
        z.start_file(MANIFEST, opts).map_err(|e| e.to_string())?;
        z.write_all(&json).map_err(|e| e.to_string())?;
        for (name, bytes) in &c.assets.files {
            z.start_file(name.as_str(), opts)
                .map_err(|e| e.to_string())?;
            z.write_all(bytes).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf.into_inner())
}

pub fn decode(bytes: &[u8]) -> Result<Container, String> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| format!("not a Seraphina project (bad zip: {})", e))?;
    let mut manifest = None;
    let mut assets = Assets::default();
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        if f.is_dir() {
            continue;
        }
        let name = f.name().map_err(|e| e.to_string())?.into_owned();
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data)
            .map_err(|e| format!("project entry '{}' is damaged: {}", name, e))?;
        if name == MANIFEST {
            manifest = Some(
                serde_json::from_slice::<Value>(&data)
                    .map_err(|e| format!("manifest.json is not valid JSON: {}", e))?,
            );
        } else {
            assets.files.insert(name, data);
        }
    }
    let manifest = migrate(manifest.ok_or("project has no manifest.json")?)?;
    Ok(Container { manifest, assets })
}

pub fn read(path: &Path) -> Result<Container, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {}", path.display(), e))?;
    decode(&bytes).map_err(|e| format!("{}: {}", path.display(), e))
}

pub fn write(path: &Path, c: &Container) -> Result<(), String> {
    let bytes = encode(c)?;
    write_atomic(path, &bytes).map_err(|e| format!("{}: {}", path.display(), e))
}

// ---- atomic writes ----------------------------------------------------------

fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    path.with_file_name(format!(".{}.tmp-{}", name, std::process::id()))
}

/// UTC timestamp for file names: `20261023T201500Z`.
pub fn file_stamp(secs: u64) -> String {
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // civil-from-days (H. Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        y,
        m,
        d,
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Move a file that could not be parsed out of the way, to
/// `<name>.corrupt-<UTC timestamp>`, so the next save cannot destroy it.
pub fn quarantine(path: &Path) -> std::io::Result<PathBuf> {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let mut dest = path.with_file_name(format!("{}.corrupt-{}", name, file_stamp(secs)));
    let mut n = 1;
    while dest.exists() {
        n += 1;
        dest = path.with_file_name(format!("{}.corrupt-{}-{}", name, file_stamp(secs), n));
    }
    std::fs::rename(path, &dest)?;
    Ok(dest)
}

/// Load a JSON settings file. A missing file gives the defaults quietly; a
/// file that cannot be parsed is kept (see [`quarantine`]) and the returned
/// message tells the user where it went.
pub fn load_json<T: serde::de::DeserializeOwned + Default>(
    path: &Path,
    what: &str,
) -> (T, Option<String>) {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (T::default(), None),
        Err(e) => {
            eprintln!("warning: cannot read {}: {}", path.display(), e);
            return (
                T::default(),
                Some(format!(
                    "Could not read the {} ({}): {}. Using defaults for now.",
                    what,
                    path.display(),
                    e
                )),
            );
        }
    };
    match serde_json::from_str(&text) {
        Ok(v) => (v, None),
        Err(e) => {
            let msg = match quarantine(path) {
                Ok(bak) => format!(
                    "The {} was damaged ({}). It was kept as {} and a fresh one was started.",
                    what,
                    e,
                    bak.display()
                ),
                Err(re) => format!(
                    "The {} ({}) was damaged ({}) and could not be backed up ({}).",
                    what,
                    path.display(),
                    e,
                    re
                ),
            };
            eprintln!("warning: {}", msg);
            (T::default(), Some(msg))
        }
    }
}

/// Replace `path` with `bytes` so readers only ever see the old or the new
/// file: write a sibling temp file, fsync it, then rename over the target.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    write_atomic_inner(path, bytes, None)
}

/// `interrupt_after`: test hook simulating a crash after that many bytes
/// (the temp file is left behind exactly as a killed process would).
fn write_atomic_inner(
    path: &Path,
    bytes: &[u8],
    interrupt_after: Option<usize>,
) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = temp_path(path);
    let r = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        if let Some(n) = interrupt_after {
            f.write_all(&bytes[..n.min(bytes.len())])?;
            return Err(std::io::Error::other("write interrupted"));
        }
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)?;
        #[cfg(unix)]
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
            if let Ok(d) = std::fs::File::open(dir) {
                let _ = d.sync_all();
            }
        }
        Ok(())
    })();
    if r.is_err() && interrupt_after.is_none() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

// ---- locations ----------------------------------------------------------------

fn env_dir(key: &str) -> Option<PathBuf> {
    std::env::var(key)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .map(PathBuf::from)
}

/// `%APPDATA%\Seraphina\autosave` / `$XDG_DATA_HOME/Seraphina/autosave`
/// (`SERAPHINA_AUTOSAVE_DIR` overrides).
pub fn autosave_dir() -> PathBuf {
    if let Some(d) = env_dir("SERAPHINA_AUTOSAVE_DIR") {
        return d;
    }
    dirs::data_dir()
        .unwrap_or_else(crate::prefs::seraphina_home)
        .join("Seraphina")
        .join("autosave")
}

/// `Documents\Seraphina` (`SERAPHINA_DOCUMENTS` overrides): where Save and
/// Export dialogs start.
pub fn documents_dir() -> PathBuf {
    if let Some(d) = env_dir("SERAPHINA_DOCUMENTS") {
        return d;
    }
    dirs::document_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Seraphina")
}

/// Whether to use the OS file dialogs. `SERAPHINA_NO_NATIVE_DIALOG`
/// forces the in-app prompt; Linux needs a session bus for the portal
/// (rfd falls back to zenity, and to the in-app prompt if neither works).
pub fn native_dialogs_available() -> bool {
    if std::env::var_os("SERAPHINA_NO_NATIVE_DIALOG").is_some() {
        return false;
    }
    cfg!(any(windows, target_os = "macos"))
        || std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some()
}

/// `name.seraphina` (adds the extension when missing).
pub fn with_ext(path: &str) -> PathBuf {
    let p = PathBuf::from(path.trim());
    match p.extension() {
        Some(e) if e.eq_ignore_ascii_case(EXT) => p,
        _ => {
            let mut s = p.into_os_string();
            s.push(".");
            s.push(EXT);
            PathBuf::from(s)
        }
    }
}

// ---- autosave session -----------------------------------------------------------

/// Crash detection and the autosave file for one app session.
#[derive(Debug, Default)]
pub struct Session {
    /// `None` = autosave disabled (tests, headless scripts).
    pub dir: Option<PathBuf>,
    /// The previous session crashed and left this autosave.
    pub recovery: Option<PathBuf>,
}

impl Session {
    /// Start a session in `dir`: detect a crashed predecessor, then write
    /// our own marker.
    pub fn start(dir: PathBuf) -> Self {
        let marker = dir.join(SESSION_MARKER);
        let auto = dir.join(AUTOSAVE_FILE);
        let crashed = marker.exists() && !marker_alive(&marker);
        let recovery = (crashed && auto.exists()).then_some(auto);
        let _ = std::fs::create_dir_all(&dir);
        let _ = write_atomic(&marker, std::process::id().to_string().as_bytes());
        Self {
            dir: Some(dir),
            recovery,
        }
    }

    pub fn autosave_path(&self) -> Option<PathBuf> {
        self.dir.as_ref().map(|d| d.join(AUTOSAVE_FILE))
    }

    /// Clean exit: the autosave is no longer needed.
    pub fn end(&mut self) {
        if let Some(d) = self.dir.take() {
            let _ = std::fs::remove_file(d.join(AUTOSAVE_FILE));
            let _ = std::fs::remove_file(d.join(SESSION_MARKER));
        }
    }

    /// Drop the crashed session's autosave (user chose Discard).
    pub fn discard_recovery(&mut self) {
        if let Some(p) = self.recovery.take() {
            let _ = std::fs::remove_file(p);
        }
    }
}

/// Another running instance owns the marker (Linux checks /proc; elsewhere a
/// marker is always treated as stale, so a crash is never missed).
fn marker_alive(marker: &Path) -> bool {
    let Ok(s) = std::fs::read_to_string(marker) else {
        return false;
    };
    let Ok(pid) = s.trim().parse::<u32>() else {
        return false;
    };
    if pid == std::process::id() {
        return false;
    }
    #[cfg(target_os = "linux")]
    {
        Path::new(&format!("/proc/{}", pid)).exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Container {
        let mut a = Assets::default();
        let r = Raster::filled(3, 2, [1, 2, 3, 4]);
        let rr = a.put_raster("layer one", &r);
        let mut m = serde_json::to_value(header()).unwrap();
        m["raster"] = serde_json::to_value(&rr).unwrap();
        Container {
            manifest: m,
            assets: a,
        }
    }

    #[test]
    fn container_round_trips() {
        let c = sample();
        let back = decode(&encode(&c).unwrap()).unwrap();
        assert_eq!(back, c);
        let rr: RasterRef = serde_json::from_value(back.manifest["raster"].clone()).unwrap();
        assert_eq!(
            back.assets.raster(&rr).unwrap(),
            Raster::filled(3, 2, [1, 2, 3, 4])
        );
    }

    #[test]
    fn migrate_checks_format_and_version() {
        let mut m = serde_json::to_value(header()).unwrap();
        assert!(migrate(m.clone()).is_ok());
        m["format_version"] = Value::from(FORMAT_VERSION + 1);
        assert!(migrate(m.clone()).unwrap_err().contains("newer"));
        m["format"] = Value::from("psd");
        assert!(migrate(m).is_err());
        assert!(decode(b"not a zip").unwrap_err().contains("bad zip"));
    }

    #[test]
    fn atomic_write_survives_an_interrupted_write() {
        let tmp = crate::test_dir::TestDir::new("atomic");
        let d = tmp.path().to_path_buf();
        let p = d.join("art.seraphina");
        let good = encode(&sample()).unwrap();
        write_atomic(&p, &good).unwrap();
        // a second save dies half way (process killed / disk full)
        let mut c2 = sample();
        c2.manifest["extra"] = Value::from("x".repeat(10_000));
        let newer = encode(&c2).unwrap();
        assert!(write_atomic_inner(&p, &newer, Some(newer.len() / 2)).is_err());
        assert!(
            temp_path(&p).exists(),
            "the partial temp file is what a crash leaves"
        );
        // the target still holds the complete previous save
        assert_eq!(std::fs::read(&p).unwrap(), good);
        assert_eq!(read(&p).unwrap(), sample());
        // the next save replaces both cleanly
        write_atomic(&p, &newer).unwrap();
        assert_eq!(read(&p).unwrap(), c2);
        assert!(!temp_path(&p).exists());
        crate::test_dir::fail_late();
    }

    #[test]
    fn session_marker_detects_a_crash() {
        let tmp = crate::test_dir::TestDir::new("session");
        let d = tmp.path().to_path_buf();
        // clean run: no recovery next time
        let mut s = Session::start(d.clone());
        assert!(s.recovery.is_none());
        write_atomic(&s.autosave_path().unwrap(), b"x").unwrap();
        s.end();
        assert!(Session::start(d.clone()).recovery.is_none());
        // crashed run: marker and autosave left behind
        let s = Session::start(d.clone());
        write_atomic(&s.autosave_path().unwrap(), b"x").unwrap();
        std::fs::write(d.join(SESSION_MARKER), "999999999").unwrap();
        drop(s);
        let mut s2 = Session::start(d.clone());
        assert_eq!(
            s2.recovery.as_deref(),
            Some(d.join(AUTOSAVE_FILE).as_path())
        );
        s2.discard_recovery();
        assert!(!d.join(AUTOSAVE_FILE).exists());
        crate::test_dir::fail_late();
    }

    #[test]
    fn extension_is_added_once() {
        assert_eq!(with_ext("a/b"), PathBuf::from("a/b.seraphina"));
        assert_eq!(with_ext("c.SERAPHINA"), PathBuf::from("c.SERAPHINA"));
    }
}

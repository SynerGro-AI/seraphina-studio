//! Secrets at rest: the stream key and the xAI API key.
//!
//! On Windows they live in Credential Manager (generic credentials named
//! `SeraphinaStudio/<name>`, per user, never roaming). Elsewhere, and on
//! Windows if Credential Manager refuses, they go to a file in the studio
//! folder that only the user can read (0600 on Unix). A key found in the
//! old plaintext file is moved into Credential Manager the first time it
//! is read, and the file is removed once the move succeeds.
//!
//! Values are never logged: every value read or written is passed to
//! [`crate::crash::register_secret`] so crash reports scrub it.

use std::io::Write;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Secret {
    StreamKey,
    XaiKey,
}

impl Secret {
    fn name(self) -> &'static str {
        match self {
            Secret::StreamKey => "stream_key",
            Secret::XaiKey => "xai_api_key",
        }
    }
}

/// Where a secret currently lives (for the Settings window).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Where {
    CredentialManager,
    File,
    None,
}

impl Where {
    pub fn label(self) -> &'static str {
        match self {
            Where::CredentialManager => "Windows Credential Manager",
            Where::File => "a private file in the studio folder",
            Where::None => "not stored",
        }
    }
}

/// A secret store rooted at a folder, with a credential-name prefix.
/// [`Store::user`] is the real one; tests make their own.
#[derive(Clone, Debug)]
pub struct Store {
    dir: PathBuf,
    prefix: String,
    use_vault: bool,
}

impl Store {
    pub fn user() -> Self {
        Self {
            dir: crate::output::studio_dir(),
            prefix: "SeraphinaStudio".into(),
            use_vault: cfg!(windows),
        }
    }

    #[cfg(test)]
    pub fn at(dir: &Path, prefix: &str, use_vault: bool) -> Self {
        Self {
            dir: dir.to_path_buf(),
            prefix: prefix.into(),
            use_vault: use_vault && cfg!(windows),
        }
    }

    fn file(&self, s: Secret) -> PathBuf {
        self.dir.join(s.name())
    }

    fn target(&self, s: Secret) -> String {
        format!("{}/{}", self.prefix, s.name())
    }

    /// The stored value, or "" when there is none.
    pub fn load(&self, s: Secret) -> String {
        if self.use_vault {
            if let Some(v) = vault::read(&self.target(s)) {
                crate::crash::register_secret(&v);
                return v;
            }
        }
        let v = read_file(&self.file(s));
        if !v.is_empty() {
            crate::crash::register_secret(&v);
            // Migrate an old plaintext file into Credential Manager.
            if self.use_vault && vault::write(&self.target(s), &v).is_ok() {
                let _ = std::fs::remove_file(self.file(s));
            }
        }
        v
    }

    /// Store `value` (trimmed). Empty clears it. Returns where it went.
    pub fn save(&self, s: Secret, value: &str) -> Result<Where, String> {
        let v = value.trim();
        if v.is_empty() {
            self.clear(s);
            return Ok(Where::None);
        }
        crate::crash::register_secret(v);
        if self.use_vault {
            match vault::write(&self.target(s), v) {
                Ok(()) => {
                    let _ = std::fs::remove_file(self.file(s));
                    return Ok(Where::CredentialManager);
                }
                Err(_) => {
                    // Credential Manager refused: fall back to the private file.
                    write_private(&self.dir, &self.file(s), v)?;
                    return Ok(Where::File);
                }
            }
        }
        write_private(&self.dir, &self.file(s), v)?;
        Ok(Where::File)
    }

    pub fn clear(&self, s: Secret) {
        if self.use_vault {
            vault::delete(&self.target(s));
        }
        let _ = std::fs::remove_file(self.file(s));
    }

    pub fn location(&self, s: Secret) -> Where {
        if self.use_vault && vault::read(&self.target(s)).is_some() {
            Where::CredentialManager
        } else if !read_file(&self.file(s)).is_empty() {
            Where::File
        } else {
            Where::None
        }
    }
}

fn read_file(p: &Path) -> String {
    std::fs::read_to_string(p)
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn write_private(dir: &Path, p: &Path, v: &str) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    let mut f = o.open(p).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        // `mode` only applies on create; tighten an existing file too.
        use std::os::unix::fs::PermissionsExt;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
    }
    f.write_all(v.as_bytes()).map_err(|e| e.to_string())
}

/// Where the xAI key comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySource {
    /// SERAPHINA_GROK_API_KEY or XAI_API_KEY (always wins).
    Env,
    /// Entered in Settings.
    Saved,
}

fn env_key() -> Option<String> {
    ["SERAPHINA_GROK_API_KEY", "XAI_API_KEY"]
        .iter()
        .filter_map(|n| std::env::var(n).ok())
        .map(|v| v.trim().to_string())
        .find(|v| !v.is_empty())
}

/// The xAI key: environment first, then the saved one. Unit tests never
/// read the user's real store.
pub fn xai_key() -> Option<(String, KeySource)> {
    if let Some(k) = env_key() {
        crate::crash::register_secret(&k);
        return Some((k, KeySource::Env));
    }
    if cfg!(test) {
        return None;
    }
    let k = Store::user().load(Secret::XaiKey);
    (!k.is_empty()).then_some((k, KeySource::Saved))
}

#[cfg(test)]
fn env_overrides_xai() -> bool {
    env_key().is_some()
}

/// GET /v1/models with the key. Ok(model count) or a short reason that
/// never includes the key.
pub fn test_xai_key(base: &str, key: &str) -> Result<usize, String> {
    let url = format!("{}/models", base.trim_end_matches('/'));
    crate::net::key_allowed(&url)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(20))
        .build();
    match agent
        .get(&url)
        .set("Authorization", &format!("Bearer {}", key.trim()))
        .call()
    {
        Ok(r) => {
            let v: serde_json::Value = r.into_json().map_err(|e| e.to_string())?;
            Ok(v["data"].as_array().map(|a| a.len()).unwrap_or(0))
        }
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => {
            Err("the key was rejected (401/403)".into())
        }
        // xAI answers 400 "Incorrect API key provided" for a malformed key.
        Err(ureq::Error::Status(400, r)) => {
            let body = r.into_string().unwrap_or_default().to_ascii_lowercase();
            if body.contains("api key") {
                Err("the key was rejected (incorrect API key)".into())
            } else {
                Err("the server answered 400".into())
            }
        }
        Err(ureq::Error::Status(c, _)) => Err(format!("the server answered {}", c)),
        Err(ureq::Error::Transport(t)) => Err(format!("couldn't reach {} ({})", url, t.kind())),
    }
}

#[cfg(windows)]
mod vault {
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn read(target: &str) -> Option<String> {
        let t = wide(target);
        let mut p: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: valid NUL-terminated target; `p` is freed with CredFree.
        unsafe {
            if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut p) == 0 || p.is_null() {
                return None;
            }
            let c = &*p;
            let bytes = if c.CredentialBlob.is_null() {
                Vec::new()
            } else {
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec()
            };
            CredFree(p as *const _);
            let v = String::from_utf8(bytes).ok()?.trim().to_string();
            (!v.is_empty()).then_some(v)
        }
    }

    pub fn write(target: &str, value: &str) -> Result<(), String> {
        let mut t = wide(target);
        let mut user = wide("Seraphina Studio");
        let mut blob = value.as_bytes().to_vec();
        // SAFETY: all pointers outlive the call; zeroed is a valid CREDENTIALW.
        let ok = unsafe {
            let mut c: CREDENTIALW = std::mem::zeroed();
            c.Type = CRED_TYPE_GENERIC;
            c.TargetName = t.as_mut_ptr();
            c.UserName = user.as_mut_ptr();
            c.CredentialBlobSize = blob.len() as u32;
            c.CredentialBlob = blob.as_mut_ptr();
            c.Persist = CRED_PERSIST_LOCAL_MACHINE;
            CredWriteW(&c, 0)
        };
        blob.iter_mut().for_each(|b| *b = 0);
        if ok != 0 {
            Ok(())
        } else {
            Err(format!("error {}", std::io::Error::last_os_error()))
        }
    }

    pub fn delete(target: &str) {
        let t = wide(target);
        // SAFETY: valid NUL-terminated target.
        unsafe {
            CredDeleteW(t.as_ptr(), CRED_TYPE_GENERIC, 0);
        }
    }
}

#[cfg(not(windows))]
mod vault {
    pub fn read(_: &str) -> Option<String> {
        None
    }
    pub fn write(_: &str, _: &str) -> Result<(), String> {
        Err("no credential store on this platform".into())
    }
    pub fn delete(_: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrip_and_clear() {
        let tmp = crate::test_dir::TestDir::new("secrets-file");
        let d = tmp.path();
        let s = Store::at(d, "SeraphinaStudioTest", false);
        assert_eq!(s.load(Secret::StreamKey), "");
        assert_eq!(s.save(Secret::StreamKey, "  live_abc  \n"), Ok(Where::File));
        assert_eq!(s.load(Secret::StreamKey), "live_abc");
        assert_eq!(s.location(Secret::StreamKey), Where::File);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(d.join("stream_key"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(m & 0o777, 0o600);
        }
        assert_eq!(s.save(Secret::StreamKey, ""), Ok(Where::None));
        assert_eq!(s.location(Secret::StreamKey), Where::None);
        // Separate slots.
        s.save(Secret::XaiKey, "xai-test").unwrap();
        assert_eq!(s.load(Secret::StreamKey), "");
        assert_eq!(s.load(Secret::XaiKey), "xai-test");
        crate::test_dir::fail_late();
    }

    #[test]
    fn unit_tests_never_read_a_saved_xai_key() {
        if !env_overrides_xai() {
            assert!(xai_key().is_none());
        }
    }

    #[test]
    fn key_test_reports_unreachable_without_the_key() {
        let k = "xai-NOT-A-REAL-KEY-0123456789";
        let e = test_xai_key("http://127.0.0.1:9", k).unwrap_err();
        assert!(e.contains("couldn't reach") && !e.contains(k), "{e}");
        // B7: never over plain http to another machine (refused before connecting)
        let e = test_xai_key("http://192.0.2.1/v1", k).unwrap_err();
        assert!(e.contains("unencrypted") && !e.contains(k), "{e}");
    }

    /// Live: the env key passes, a fake one is rejected. Never prints keys.
    #[test]
    #[ignore = "live: needs XAI_API_KEY or SERAPHINA_GROK_API_KEY"]
    fn live_xai_key_test() {
        let k = env_key().expect("live_xai_key_test needs XAI_API_KEY");
        let n = test_xai_key("https://api.x.ai/v1", &k).expect("env key should work");
        assert!(n > 0);
        let e = test_xai_key("https://api.x.ai/v1", "xai-FAKEdemoKEY0000000000000000").unwrap_err();
        assert!(e.contains("rejected"), "{e}");
        eprintln!("live_xai_key_test: real key ok ({n} models), fake key: {e}");
    }

    /// Real Credential Manager (Windows only; also runs under Wine).
    #[test]
    #[cfg_attr(
        not(windows),
        ignore = "Windows Credential Manager only (runs under Wine)"
    )]
    fn vault_store_migrates_plaintext_file() {
        let tmp = crate::test_dir::TestDir::new("secrets-vault");
        let d = tmp.path();
        let prefix = format!("SeraphinaStudioTest{}", std::process::id());
        let s = Store::at(d, &prefix, true);
        write_private(d, &d.join("stream_key"), "live_old").unwrap();
        assert_eq!(s.load(Secret::StreamKey), "live_old");
        assert!(!d.join("stream_key").exists(), "plaintext file migrated");
        assert_eq!(s.location(Secret::StreamKey), Where::CredentialManager);
        assert_eq!(
            s.save(Secret::StreamKey, "live_new"),
            Ok(Where::CredentialManager)
        );
        assert_eq!(s.load(Secret::StreamKey), "live_new");
        s.clear(Secret::StreamKey);
        assert_eq!(s.location(Secret::StreamKey), Where::None);
        crate::test_dir::fail_late();
    }
}

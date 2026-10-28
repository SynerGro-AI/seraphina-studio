//! Shared HTTP plumbing for the AI calls (M4a): timed agents, retry with
//! backoff, a Stop flag the UI can raise, and the https-only rule for
//! requests that carry an API key.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Raised by the UI's Stop button; workers check it between steps.
pub type Cancel = Arc<AtomicBool>;

pub fn cancel_flag() -> Cancel {
    Arc::new(AtomicBool::new(false))
}

/// Error text for a job the user stopped.
pub const STOPPED: &str = "stopped";

/// Retries after the first attempt.
pub const MAX_RETRIES: u32 = 3;

/// Longest wait we honour from a server's Retry-After header.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);

/// An agent with connect / read / write timeouts so a dead server can't
/// hang a worker forever.
pub fn agent(read: Duration) -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(read)
        .timeout_write(Duration::from_secs(60))
        .build()
}

/// Which failures are safe to send again.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Policy {
    /// Chat completions and GETs: 429, 500, 502, 503, 504 and
    /// connection failures.
    Transient,
    /// Billed job submissions (Grok Imagine): only answers that say the
    /// job was not accepted (429, 503) and connections that never
    /// reached the server, so a retry can't start a second paid job.
    NotAccepted,
}

/// Should `e` be retried under `p`? Read timeouts and broken streams are
/// never retried: the server may already have done (and billed) the work.
pub fn retryable(e: &ureq::Error, p: Policy) -> bool {
    match e {
        ureq::Error::Status(code, _) => match p {
            Policy::Transient => matches!(code, 429 | 500 | 502 | 503 | 504),
            Policy::NotAccepted => matches!(code, 429 | 503),
        },
        ureq::Error::Transport(t) => matches!(
            t.kind(),
            ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed
        ),
    }
}

/// Retry-After as seconds (the HTTP-date form is ignored).
pub fn parse_retry_after(v: Option<&str>) -> Option<Duration> {
    v?.trim()
        .parse::<f64>()
        .ok()
        .filter(|s| s.is_finite() && *s >= 0.0)
        .map(|s| Duration::from_secs_f64(s).min(MAX_RETRY_AFTER))
}

/// Cheap jitter in [0, 1): no rand crate needed.
fn jitter() -> f64 {
    use std::sync::atomic::AtomicU64;
    static N: AtomicU64 = AtomicU64::new(0);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0);
    let mut x = t ^ N.fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::Relaxed);
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51_afd7_ed55_8ccd);
    x ^= x >> 33;
    (x % 10_000) as f64 / 10_000.0
}

/// Wait before retry `attempt` (0-based): Retry-After when the server
/// sent one, else 1 s, 2 s, 4 s … plus up to 50 % jitter.
pub fn backoff(attempt: u32, retry_after: Option<Duration>) -> Duration {
    if let Some(d) = retry_after {
        return d;
    }
    let base = 2f64.powi(attempt.min(6) as i32);
    Duration::from_secs_f64(base * (1.0 + 0.5 * jitter()))
}

/// Sleep for `d` in short slices; false if `cancel` was raised meanwhile.
pub fn sleep_unless(cancel: &AtomicBool, d: Duration) -> bool {
    let end = std::time::Instant::now() + d;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return false;
        }
        let now = std::time::Instant::now();
        if now >= end {
            return true;
        }
        std::thread::sleep((end - now).min(Duration::from_millis(50)));
    }
}

/// Why a call with retries gave up.
#[derive(Debug)]
pub enum Failure {
    Stopped,
    Http(Box<ureq::Error>),
}

/// Send the request built by `make` (with `body`, if any) until it
/// succeeds, fails for good, runs out of retries or the user presses
/// Stop. `on_retry(attempt, wait, reason)` lets the UI say what is
/// happening.
pub fn with_retry(
    cancel: &AtomicBool,
    policy: Policy,
    mut make: impl FnMut() -> ureq::Request,
    body: Option<&str>,
    on_retry: &mut dyn FnMut(u32, Duration, &str),
) -> Result<ureq::Response, Failure> {
    let mut attempt = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Failure::Stopped);
        }
        let sent = match body {
            Some(b) => make().send_string(b),
            None => make().call(),
        };
        match sent {
            Ok(r) => return Ok(r),
            Err(e) if attempt < MAX_RETRIES && retryable(&e, policy) => {
                let (reason, after) = match &e {
                    ureq::Error::Status(c, r) => (
                        format!("HTTP {}", c),
                        parse_retry_after(r.header("retry-after")),
                    ),
                    ureq::Error::Transport(t) => (format!("{}", t.kind()), None),
                };
                let wait = backoff(attempt, after);
                attempt += 1;
                on_retry(attempt, wait, &reason);
                if !sleep_unless(cancel, wait) {
                    return Err(Failure::Stopped);
                }
            }
            Err(e) => return Err(Failure::Http(Box::new(e))),
        }
    }
}

/// Host part of an http(s) URL, without userinfo or port.
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let auth = auth.rsplit('@').next().unwrap_or(auth);
    if let Some(v6) = auth.strip_prefix('[') {
        return v6.split(']').next().unwrap_or(v6);
    }
    auth.split(':').next().unwrap_or(auth)
}

fn loopback(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    h == "localhost" || h.ends_with(".localhost") || h == "::1" || h.starts_with("127.")
}

/// True for an http(s) URL on this machine (a local model server).
pub fn is_local(url: &str) -> bool {
    loopback(host(url.trim()))
}

/// B7: an API key only goes over https, or plain http to this machine
/// (local model servers).
pub fn key_allowed(url: &str) -> Result<(), String> {
    let lower = url.trim().to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    let h = host(url.trim());
    if lower.starts_with("http://") && loopback(h) {
        return Ok(());
    }
    Err(format!(
        "refusing to send your API key to {} over an unencrypted connection; use an https:// URL (plain http is only allowed for localhost)",
        if h.is_empty() { url.trim() } else { h }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_and_backoff() {
        assert_eq!(parse_retry_after(Some("2")), Some(Duration::from_secs(2)));
        assert_eq!(
            parse_retry_after(Some(" 0.5 ")),
            Some(Duration::from_millis(500))
        );
        assert_eq!(parse_retry_after(Some("600")), Some(MAX_RETRY_AFTER));
        assert_eq!(
            parse_retry_after(Some("Wed, 21 Oct 2015 07:28:00 GMT")),
            None
        );
        assert_eq!(parse_retry_after(Some("-1")), None);
        assert_eq!(parse_retry_after(None), None);
        for a in 0..3 {
            let d = backoff(a, None).as_secs_f64();
            let base = 2f64.powi(a as i32);
            assert!(d >= base && d <= base * 1.5, "attempt {} waited {}", a, d);
        }
        assert_eq!(
            backoff(2, Some(Duration::from_secs(3))),
            Duration::from_secs(3)
        );
    }

    #[test]
    fn sleep_stops_early_when_cancelled() {
        let c = cancel_flag();
        assert!(sleep_unless(&c, Duration::from_millis(10)));
        c.store(true, Ordering::Relaxed);
        let t = std::time::Instant::now();
        assert!(!sleep_unless(&c, Duration::from_secs(10)));
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    /// A tiny local HTTP server that answers each connection with the next
    /// canned response.
    fn serve(replies: Vec<&'static str>) -> (String, std::thread::JoinHandle<usize>) {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let mut n = 0;
            for r in replies {
                let Ok((mut s, _)) = l.accept() else { break };
                let mut buf = [0u8; 4096];
                let _ = s.read(&mut buf);
                let _ = s.write_all(r.as_bytes());
                n += 1;
            }
            n
        });
        (url, h)
    }

    const BUSY: &str =
        "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const DOWN: &str =
        "HTTP/1.1 503 Service Unavailable\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const FAIL: &str =
        "HTTP/1.1 500 Internal Server Error\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const BAD: &str = "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
    const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";

    #[test]
    fn retries_429_and_5xx_then_succeeds() {
        let (url, h) = serve(vec![BUSY, DOWN, OK]);
        let a = agent(Duration::from_secs(5));
        let mut seen = Vec::new();
        let r = with_retry(
            &AtomicBool::new(false),
            Policy::Transient,
            || a.get(&url),
            None,
            &mut |n, _, why| seen.push((n, why.to_string())),
        );
        assert_eq!(r.unwrap().into_string().unwrap(), "ok");
        assert_eq!(seen, vec![(1, "HTTP 429".into()), (2, "HTTP 503".into())]);
        assert_eq!(h.join().unwrap(), 3);
    }

    #[test]
    fn gives_up_after_three_retries_and_never_retries_401() {
        let (url, h) = serve(vec![BUSY, BUSY, BUSY, BUSY]);
        let a = agent(Duration::from_secs(5));
        let mut n = 0;
        let r = with_retry(
            &AtomicBool::new(false),
            Policy::Transient,
            || a.get(&url),
            None,
            &mut |_, _, _| n += 1,
        );
        assert!(
            matches!(r, Err(Failure::Http(ref e)) if matches!(**e, ureq::Error::Status(429, _)))
        );
        assert_eq!((n, h.join().unwrap()), (3, 4));

        let (url, h) = serve(vec![BAD]);
        let r = with_retry(
            &AtomicBool::new(false),
            Policy::Transient,
            || a.get(&url),
            None,
            &mut |_, _, _| n += 1,
        );
        assert!(
            matches!(r, Err(Failure::Http(ref e)) if matches!(**e, ureq::Error::Status(401, _)))
        );
        assert_eq!((n, h.join().unwrap()), (3, 1));
    }

    #[test]
    fn billed_submissions_do_not_retry_a_500() {
        let (url, h) = serve(vec![FAIL]);
        let a = agent(Duration::from_secs(5));
        let r = with_retry(
            &AtomicBool::new(false),
            Policy::NotAccepted,
            || a.get(&url),
            None,
            &mut |_, _, _| {},
        );
        assert!(
            matches!(r, Err(Failure::Http(ref e)) if matches!(**e, ureq::Error::Status(500, _)))
        );
        assert_eq!(h.join().unwrap(), 1);
    }

    #[test]
    fn stop_wins_over_retry_and_connect_failures_retry() {
        let c = AtomicBool::new(true);
        let r = with_retry(
            &c,
            Policy::Transient,
            || unreachable!(),
            None,
            &mut |_, _, _| {},
        );
        assert!(matches!(r, Err(Failure::Stopped)));
        // A closed port: connection refused is retried, then reported.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let a = agent(Duration::from_secs(2));
        let url = format!("http://127.0.0.1:{}/", port);
        let c = AtomicBool::new(false);
        let mut n = 0;
        let r = with_retry(
            &c,
            Policy::Transient,
            || a.get(&url),
            None,
            &mut |_, _, _| {
                n += 1;
                c.store(true, Ordering::Relaxed); // Stop during the first wait
            },
        );
        assert!(matches!(r, Err(Failure::Stopped)));
        assert_eq!(n, 1);
    }

    #[test]
    fn api_keys_only_over_https_or_localhost() {
        assert!(key_allowed("https://api.x.ai/v1/chat/completions").is_ok());
        assert!(key_allowed("HTTPS://API.X.AI/v1").is_ok());
        assert!(key_allowed("http://localhost:11434/v1/chat/completions").is_ok());
        assert!(key_allowed("http://127.0.0.1:8080/v1").is_ok());
        assert!(key_allowed("http://[::1]:8080/v1").is_ok());
        let e = key_allowed("http://api.x.ai/v1/chat/completions").unwrap_err();
        assert!(e.contains("api.x.ai") && e.contains("https"), "{}", e);
        assert!(key_allowed("http://localhost.evil.com/v1").is_err());
        assert!(key_allowed("http://127.0.0.1@evil.com/v1").is_err());
        assert!(key_allowed("ftp://x").is_err());
        assert_eq!(host("http://user:pw@example.com:8080/a?b"), "example.com");
        assert_eq!(host("http://[::1]:80/"), "::1");
        assert!(is_local("http://localhost:11434/v1") && !is_local("https://api.x.ai/v1"));
    }
}

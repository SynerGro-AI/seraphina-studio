//! Safe pure Reconnect (OBS obs-output.c reconnect).
//!
//! Parsed from libobs/obs-output.c.
//! Defaults: retry_sec = 2, retry_max = 20, base exp = 1.5.
//! Tracks attempt count and computes the next wait.
//! No unwraps.
//!
//! Differential: more attempts produce a longer wait.

/// Base exponent from OBS.
pub const RECONNECT_RETRY_BASE_EXP: f32 = 1.5;
/// Maximum wait in milliseconds from OBS.
pub const RECONNECT_RETRY_MAX_MSEC: u64 = 15 * 60 * 1000;

#[derive(Clone, Debug)]
pub struct Reconnect {
    retry_sec: u32,
    retry_max: u32,
    attempts: u32,
    active: bool,
}

impl Reconnect {
    /// Defaults matching OBS: 2 seconds, max 20 attempts.
    pub fn new() -> Self {
        Self {
            retry_sec: 2,
            retry_max: 20,
            attempts: 0,
            active: false,
        }
    }

    /// Override the settings. Matching obs_output_set_reconnect_settings.
    pub fn set_settings(&mut self, retry_max: u32, retry_sec: u32) {
        self.retry_max = retry_max;
        self.retry_sec = retry_sec.max(1);
    }

    pub fn retry_sec(&self) -> u32 {
        self.retry_sec
    }

    pub fn retry_max(&self) -> u32 {
        self.retry_max
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Begin a reconnect attempt. Returns false if max is reached.
    pub fn start(&mut self) -> bool {
        if self.attempts >= self.retry_max {
            return false;
        }
        self.attempts += 1;
        self.active = true;
        true
    }

    /// Compute the wait for the current attempt in milliseconds.
    /// Uses exponential backoff capped at RECONNECT_RETRY_MAX_MSEC.
    pub fn wait_ms(&self) -> u64 {
        if self.attempts == 0 {
            return 0;
        }
        let base = self.retry_sec as f64 * 1000.0;
        let exp = RECONNECT_RETRY_BASE_EXP as f64;
        let wait = base * exp.powi((self.attempts - 1) as i32);
        wait.min(RECONNECT_RETRY_MAX_MSEC as f64) as u64
    }

    /// Mark success. Resets attempts and clears active.
    pub fn success(&mut self) {
        self.attempts = 0;
        self.active = false;
    }

    /// Abort. Clears active without resetting the count.
    pub fn abort(&mut self) {
        self.active = false;
    }
}

impl Default for Reconnect {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults() {
        let r = Reconnect::new();
        assert_eq!(r.retry_sec(), 2);
        assert_eq!(r.retry_max(), 20);
        assert!(!r.is_active());
    }

    #[test]
    fn start_increments() {
        let mut r = Reconnect::new();
        assert!(r.start());
        assert_eq!(r.attempts(), 1);
        assert!(r.is_active());
        assert!(r.wait_ms() > 0);
    }

    #[test]
    fn max_stops() {
        let mut r = Reconnect::new();
        r.set_settings(2, 1);
        assert!(r.start());
        assert!(r.start());
        assert!(!r.start());
    }

    #[test]
    fn success_resets() {
        let mut r = Reconnect::new();
        r.start();
        r.success();
        assert_eq!(r.attempts(), 0);
        assert!(!r.is_active());
    }

    #[test]
    fn hand_checked_values() {
        let mut a = Reconnect::new();
        let mut b = Reconnect::new();
        a.start();
        b.start();
        b.start();
        assert!(b.wait_ms() > a.wait_ms());
    }
}

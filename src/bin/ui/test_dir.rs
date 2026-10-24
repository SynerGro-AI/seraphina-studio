//! A scratch directory for tests that is removed when dropped, so a test
//! that fails (panics) part-way still cleans up after itself.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub struct TestDir(PathBuf);

impl TestDir {
    /// A new empty directory under the system temp dir, unique to this
    /// process and call.
    pub fn new(tag: &str) -> TestDir {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("seraphina_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap_or_else(|e| panic!("create {}: {e}", p.display()));
        TestDir(p)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("could not remove {}: {e}", self.0.display());
            }
        }
    }
}

#[test]
fn a_test_dir_is_removed_even_when_the_test_panics() {
    let mut kept = PathBuf::new();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let d = TestDir::new("test_dir_unwind");
        std::fs::write(d.path().join("f"), b"x").unwrap();
        kept = d.path().to_path_buf();
        panic!("test failed part-way");
    }));
    assert!(r.is_err());
    assert!(kept.is_absolute(), "{}", kept.display());
    assert!(!kept.exists(), "{} left behind", kept.display());
}

/// Set in the child run of `the_tests_leave_nothing_in_temp_even_when_they_fail`.
const FAIL_LATE: &str = "SERAPHINA_TEST_FAIL_LATE";

/// Called at the end of each test that keeps files in temp. In a normal run
/// it does nothing; when the leak check re-runs the test with
/// `SERAPHINA_TEST_FAIL_LATE` set, it panics there, after the test has made
/// all its files, so the check sees what a failing test leaves behind.
pub fn fail_late() {
    if std::env::var_os(FAIL_LATE).is_some() {
        panic!("failing late on purpose ({FAIL_LATE})");
    }
}

/// Every test that writes into temp, as named by the test harness.
#[cfg(target_os = "linux")]
const TESTS_USING_TEMP: &[&str] = &[
    "output::tests::a_missing_audio_device_fails_loudly",
    "output::tests::camera_failure_stops_the_recording_with_a_reason",
    "output::tests::camera_source_records_the_camera_picture",
    "output::tests::goes_live_to_an_ffmpeg_rtmp_listener",
    "output::tests::records_audio_from_a_real_pulse_source",
    "output::tests::publishes_to_mediamtx_under_the_callers_key_and_rejects_a_wrong_key",
    "output::tests::records_five_seconds_to_mp4_verified_by_ffprobe",
    "output::tests::stream_delay_holds_the_stream_back_by_the_delay",
    "output::tests::streams_to_local_rtmp_and_reconnects_with_backoff",
    "prefs::tests::corrupt_output_settings_are_preserved",
    "prefs::tests::corrupt_prefs_file_is_preserved_and_reported",
    "prefs::tests::missing_or_valid_prefs_load_quietly",
    "project::tests::atomic_write_survives_an_interrupted_write",
    "project::tests::session_marker_detects_a_crash",
    "tests::a_damaged_learning_bank_is_announced_at_startup",
    "tests::a_fresh_launch_is_not_marked_unsaved",
    "workspace::tests::a_bad_file_leaves_the_workspace_alone",
    "workspace::tests::a_relative_name_saves_and_opens_in_documents",
    "workspace::tests::animate_round_trips_losslessly",
    "workspace::tests::chat_round_trips_and_is_optional",
    "workspace::tests::composite_round_trips_losslessly",
    "workspace::tests::dirty_tracks_document_edits_not_view_changes",
    "workspace::tests::discarding_recovery_deletes_the_autosave",
    "workspace::tests::every_mode_round_trips_together",
    "workspace::tests::kill_mid_edit_then_relaunch_recovers_every_mode",
    "workspace::tests::lab_circuits_reactions_cpu_and_relativity_round_trip",
    "workspace::tests::materials_round_trip_losslessly",
    "workspace::tests::old_projects_without_physics_or_lab_still_open",
    "workspace::tests::open_with_unsaved_changes_asks_first",
    "workspace::tests::output_settings_round_trip_without_secrets",
    "workspace::tests::paint_round_trips_losslessly",
    "workspace::tests::physics_arena_round_trips_and_continues_identically",
    "workspace::tests::recovery_remembers_the_project_file",
    "workspace::tests::save_as_never_silently_overwrites",
    "workspace::tests::scene_round_trips_losslessly",
    "workspace::tests::scripts_need_a_flag_for_project_files",
    "workspace::tests::stream_round_trips_losslessly",
];

/// Re-runs every test that writes into temp in a child process, each made to
/// fail at its very end, then checks the temp dir holds nothing named with
/// the child's process id: a test that panics must still remove its files.
#[test]
#[cfg_attr(
    not(target_os = "linux"),
    ignore = "re-runs the ffmpeg and PulseAudio tests, which are Linux-only here"
)]
fn the_tests_leave_nothing_in_temp_even_when_they_fail() {
    #[cfg(target_os = "linux")]
    {
        let exe = std::env::current_exe().unwrap();
        let child = std::process::Command::new(&exe)
            .args(TESTS_USING_TEMP)
            // one at a time, so the timed recording tests in here don't
            // compete with each other on top of the main run
            .args(["--exact", "--test-threads=1"])
            .env(FAIL_LATE, "1")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap_or_else(|e| panic!("re-run {}: {e}", exe.display()));
        let pid = child.id().to_string();
        let out = child.wait_with_output().unwrap();
        let text = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
        // each test ran to its end and failed there, not earlier
        for t in TESTS_USING_TEMP {
            let section = text
                .split("\n---- ")
                .find(|s| s.starts_with(&format!("{t} stdout ----")))
                .unwrap_or_else(|| panic!("{t} did not run and fail late:\n{text}"));
            assert!(
                section.contains("failing late on purpose"),
                "{t} failed before its end:\n{section}"
            );
        }
        let left: Vec<String> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|n| n.split(['-', '_', '.']).any(|part| part == pid))
            .collect();
        assert!(
            left.is_empty(),
            "failing tests left these in {}: {left:?}",
            std::env::temp_dir().display()
        );
    }
}

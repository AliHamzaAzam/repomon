//! Windows-native agent runtime, end to end — the `tests/integration.rs` flows without tmux.
//!
//! Each test gets its own session name and registry data dir (tempdir), spawns real
//! `repomon-agent-host.exe` processes (built by `cargo test --workspace`; the backend finds
//! the binary one directory above the test executable in `target\debug`), and drives them
//! through the `SessionBackend` trait exactly as the daemon does: spawn (cmd.exe stand-ins
//! for agents), capture, input, kill, stale-entry GC, owner back-off, byte streaming, and —
//! the point of the host architecture — re-adoption of live hosts by a fresh backend, which
//! is what a daemon restart does.

#![cfg(windows)]

use std::path::Path;
use std::time::{Duration, Instant};

use repomon_core::WindowsBackend;
use repomon_core::agent::backend::{CaptureOpts, OwnerState, SessionBackend, SpawnSpec};
use repomon_core::agent::detect_usage_limit;

/// A per-test session name: unique per process AND per test tag, so parallel tests never
/// share pipes or registry directories.
fn unique_session(tag: &str) -> String {
    format!("rmwtest-{tag}-{}", std::process::id())
}

/// Poll until `f` is true or fail loudly. Host spawn + ConPTY output are asynchronous; every
/// assertion about window content goes through this (mirrors the sleeps in integration.rs,
/// but bounded and self-describing).
fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    panic!("timed out waiting for {what}");
}

fn capture(b: &WindowsBackend, window: &str) -> String {
    b.capture_named(window, CaptureOpts::visible())
        .unwrap_or_default()
}

/// A cmd.exe stand-in for an agent: prints `marker`, then sits at an interactive prompt
/// (`/K`), alive until killed — the Windows analogue of `sh -c 'echo X; sleep 30'`.
fn echo_agent(marker: &str, cwd: &Path) -> SpawnSpec {
    SpawnSpec::new(format!("cmd.exe /Q /K echo {marker}"), cwd)
}

#[test]
fn spawn_capture_input_kill_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let b = WindowsBackend::new(unique_session("roundtrip"), "me", dir.path());
    assert!(b.available(), "repomon-agent-host.exe must be findable");

    let target = b
        .spawn(1, &echo_agent("HELLO_REPOMON", dir.path()))
        .unwrap();
    assert_eq!(target, format!("{}:=lane-1", b.label()));
    assert!(b.has_window(1));
    wait_for("initial output", || {
        capture(&b, "lane-1").contains("HELLO_REPOMON")
    });

    // Typed input lands in the window (send_text = literal + Enter).
    b.send_text_named("lane-1", "echo SECOND_LINE").unwrap();
    wait_for("typed output", || {
        capture(&b, "lane-1").contains("SECOND_LINE")
    });

    // The pane answers geometry questions like tmux does (220x50 spawn default).
    assert_eq!(b.size_named("lane-1"), Some((220, 50)));
    b.resize_named("lane-1", 190, 45).unwrap();
    wait_for("resize", || b.size_named("lane-1") == Some((190, 45)));
    assert!(!b.alternate_on_named("lane-1"), "cmd.exe is not a TUI");

    // A second spawn runs side by side in the next slot; the first agent survives.
    b.spawn(1, &echo_agent("SLOT_TWO", dir.path())).unwrap();
    assert_eq!(b.windows_for(1).unwrap(), vec!["lane-1", "lane-1-2"]);
    wait_for("slot two output", || {
        b.capture_named("lane-1-2", CaptureOpts::visible())
            .unwrap_or_default()
            .contains("SLOT_TWO")
    });

    // Kill is exact: lane-1 dies, lane-1-2 survives, and the dead window reads as empty
    // output (benign absence — capture parity with the tmux impl).
    b.kill_named("lane-1").unwrap();
    wait_for("lane-1 gone", || {
        b.windows_for(1).unwrap() == vec!["lane-1-2"]
    });
    wait_for(
        "dead window reads empty",
        || matches!(b.capture_named("lane-1", CaptureOpts::visible()), Ok(s) if s.is_empty()),
    );

    b.kill_named("lane-1-2").unwrap();
    wait_for("all windows gone", || !b.has_window(1));
    // Clean exits remove their registry entries: no JSON litter left behind.
    wait_for("registry empty", || {
        let hosts = dir.path().join("hosts").join(b.label());
        !std::fs::read_dir(&hosts).is_ok_and(|d| {
            d.flatten()
                .any(|e| e.path().extension().is_some_and(|x| x == "json"))
        })
    });
}

/// THE durability test: hosts outlive the backend that spawned them (as they outlive a dying
/// daemon), and a fresh backend with the same identity re-adopts them from the registry with
/// scrollback intact.
#[test]
fn re_adopts_live_hosts_after_a_daemon_restart() {
    let dir = tempfile::tempdir().unwrap();
    let session = unique_session("readopt");

    let first = WindowsBackend::new(session.clone(), "daemon-me", dir.path());
    first.spawn(1, &echo_agent("SURVIVOR", dir.path())).unwrap();
    wait_for("output before restart", || {
        capture(&first, "lane-1").contains("SURVIVOR")
    });
    drop(first); // the "daemon" dies; the host keeps running detached

    // A restarted daemon: same session, owner identity, and data dir.
    let second = WindowsBackend::new(session, "daemon-me", dir.path());
    assert!(second.session_exists(), "host survived and is re-adopted");
    assert_eq!(second.list_windows().unwrap(), vec!["lane-1"]);

    // Scrollback survived: the marker was printed before the "restart".
    assert!(capture(&second, "lane-1").contains("SURVIVOR"));

    // The reaper's view carries the original cwd and a sane activity time.
    let acts = second.list_windows_with_activity().unwrap();
    assert_eq!(acts.len(), 1);
    assert_eq!(acts[0].name, "lane-1");
    assert_eq!(acts[0].cwd, dir.path());
    assert!(acts[0].last_activity > 0);

    // And the re-adopted window is fully drivable.
    second
        .send_text_named("lane-1", "echo AFTER_RESTART")
        .unwrap();
    wait_for("input after re-adoption", || {
        capture(&second, "lane-1").contains("AFTER_RESTART")
    });
    second.kill_named("lane-1").unwrap();
    wait_for("killed after re-adoption", || {
        second.list_windows().unwrap().is_empty()
    });
}

#[test]
fn stale_registry_entries_are_garbage_collected() {
    let dir = tempfile::tempdir().unwrap();
    let session = unique_session("gc");
    let b = WindowsBackend::new(session.clone(), "me", dir.path());

    // A registry entry whose host is long gone: its pipe does not exist.
    let hosts = dir.path().join("hosts").join(&session);
    std::fs::create_dir_all(&hosts).unwrap();
    let stale = hosts.join("lane-9.json");
    std::fs::write(
        &stale,
        format!(
            r#"{{"v":1,"session":"{session}","window":"lane-9","pipe":"\\\\.\\pipe\\repomon-{session}-lane-9","host_pid":1,"agent_pid":2,"program":"cmd.exe","args":[],"cwd":"C:\\","owner":"me","started_at":1}}"#
        ),
    )
    .unwrap();

    assert!(b.list_windows().unwrap().is_empty());
    assert!(!stale.exists(), "dead-pipe entry was GC'd by the scan");
}

/// PROTOCOL.md §6: another daemon's hosts are invisible and untouchable — never adopted,
/// reaped, or killed — and the registry-level owner stamp locks the second daemon out of
/// destructive sweeps.
#[test]
fn foreign_owned_hosts_are_backed_off_from() {
    let dir = tempfile::tempdir().unwrap();
    let session = unique_session("owner");

    let a = WindowsBackend::new(session.clone(), "daemon-A", dir.path());
    assert_eq!(a.claim_or_verify_owner("daemon-A"), OwnerState::Owned);
    a.spawn(1, &echo_agent("MINE", dir.path())).unwrap();

    let b = WindowsBackend::new(session.clone(), "daemon-B", dir.path());
    // The owner stamp belongs to A; B must back off (and keeps verifying so).
    assert_eq!(
        b.claim_or_verify_owner("daemon-B"),
        OwnerState::OwnedByOther
    );
    assert_eq!(a.claim_or_verify_owner("daemon-A"), OwnerState::Owned);
    // A's host never shows up in B's world: not listed, not adopted...
    assert!(b.list_windows().unwrap().is_empty());
    assert!(b.list_windows_with_activity().unwrap().is_empty());
    // ...and its registry entry is NOT GC'd — the pipe is alive, just not B's.
    let entry = dir.path().join("hosts").join(&session).join("lane-1.json");
    assert!(entry.exists());

    // The rightful owner still sees and controls it.
    assert_eq!(a.list_windows().unwrap(), vec!["lane-1"]);
    a.kill_named("lane-1").unwrap();
    wait_for("owner killed its host", || {
        a.list_windows().unwrap().is_empty()
    });
}

/// `subscribe_bytes` on a second connection: the first frame replays the full current screen
/// (a fresh emulator converges), later frames are live PTY output, and closing the stream
/// ends the channel (the forwarder's EOF signal).
#[test]
fn byte_stream_replays_then_follows_live_output() {
    let dir = tempfile::tempdir().unwrap();
    let b = std::sync::Arc::new(WindowsBackend::new(
        unique_session("bytes"),
        "me",
        dir.path(),
    ));
    b.spawn(1, &echo_agent("STREAM_START", dir.path())).unwrap();
    wait_for("pre-stream output", || {
        capture(&b, "lane-1").contains("STREAM_START")
    });

    let stream = b.open_byte_stream("lane-1").unwrap();
    let got = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let pump = {
        let (got, done) = (got.clone(), done.clone());
        std::thread::spawn(move || {
            let mut rx = stream.rx;
            while let Some(chunk) = rx.blocking_recv() {
                got.lock().unwrap().extend_from_slice(&chunk);
            }
            done.store(true, std::sync::atomic::Ordering::Relaxed);
        })
    };
    let text = |got: &std::sync::Arc<std::sync::Mutex<Vec<u8>>>| {
        String::from_utf8_lossy(&got.lock().unwrap()).into_owned()
    };

    // Frame 1 is a full-screen replay: output from BEFORE the subscription is in the stream.
    wait_for("replay frame", || text(&got).contains("STREAM_START"));

    // Live output follows.
    b.send_text_named("lane-1", "echo STREAM_LIVE").unwrap();
    wait_for("live frames", || text(&got).contains("STREAM_LIVE"));

    // Closing the stream ends the channel, which ends the consumer loop.
    b.close_byte_stream("lane-1").unwrap();
    wait_for("stream closed", || {
        done.load(std::sync::atomic::Ordering::Relaxed)
    });
    pump.join().unwrap();
    b.kill_named("lane-1").unwrap();
}

/// The auto-continue trigger path, minus the daemon loop (whose decision state machine is
/// unit-tested): a pane showing Claude's usage-limit message is detected from a backend
/// capture, and the resume message can be typed back through the same backend.
#[test]
fn usage_limit_capture_detects_and_resume_types_back() {
    let dir = tempfile::tempdir().unwrap();
    let b = WindowsBackend::new(unique_session("limit"), "me", dir.path());
    let spec = SpawnSpec::new(
        "cmd.exe /Q /K echo Claude usage limit reached. Your limit will reset at 3:00 PM.",
        dir.path(),
    );
    b.spawn(1, &spec).unwrap();

    wait_for("limit message on screen", || {
        capture(&b, "lane-1").contains("usage limit reached")
    });
    let pane = capture(&b, "lane-1");
    // A Some return IS the blocking-pause signal auto_continue keys off.
    let _limit = detect_usage_limit(&pane).expect("usage-limit pause detected from capture");

    // What auto_continue does on resume: type the continue message into the window.
    b.send_text_named("lane-1", "continue").unwrap();
    wait_for("resume text landed", || {
        capture(&b, "lane-1").contains("continue")
    });
    b.kill_named("lane-1").unwrap();
}

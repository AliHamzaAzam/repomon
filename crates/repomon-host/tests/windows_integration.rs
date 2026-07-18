//! Windows-only end-to-end tests: spawn the real `repomon-agent-host` binary with a real
//! ConPTY child, drive the named-pipe protocol exactly as PROTOCOL.md specifies, and check
//! tmux-parity lifecycle semantics (registry appears/disappears, host dies with its child).
//!
//! These compile on every OS (the whole file is cfg(windows)-gated) but only execute on the
//! Windows CI leg (landing with Track A). Written before the runtime existed — they are the
//! RED half of TDD for the cfg(windows) modules.

#![cfg(windows)]

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use base64::Engine as _;
use repomon_host::codec::{FrameDecoder, encode_frame};
use repomon_host::registry;

const HOST_BIN: &str = env!("CARGO_BIN_EXE_repomon-agent-host");

struct HostUnderTest {
    child: Child,
    data_dir: tempfile::TempDir,
    session: String,
    window: String,
}

impl HostUnderTest {
    fn spawn(window: &str, command: &[&str]) -> Self {
        let data_dir = tempfile::tempdir().unwrap();
        let session = format!("itest{}", std::process::id());
        let mut args = vec![
            "--session".to_string(),
            session.clone(),
            "--window".to_string(),
            window.to_string(),
            "--cwd".to_string(),
            data_dir.path().display().to_string(),
            "--owner".to_string(),
            "itest-owner-token".to_string(),
            "--".to_string(),
        ];
        args.extend(command.iter().map(|s| s.to_string()));
        let child = Command::new(HOST_BIN)
            .args(&args)
            .env("REPOMON_DATA_DIR", data_dir.path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn host binary");
        Self {
            child,
            data_dir,
            session,
            window: window.to_string(),
        }
    }

    fn registry_path(&self) -> PathBuf {
        registry::registry_path(self.data_dir.path(), &self.session, &self.window)
    }

    fn pipe_name(&self) -> String {
        registry::pipe_name(&self.session, &self.window)
    }

    fn wait_for(&self, what: &str, timeout: Duration, mut cond: impl FnMut() -> bool) {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if cond() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("timed out waiting for {what}");
    }

    fn connect(&self) -> PipeClient {
        let mut last_err = None;
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(10) {
            match std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(self.pipe_name())
            {
                Ok(f) => {
                    return PipeClient {
                        file: f,
                        decoder: FrameDecoder::new(),
                        next_id: 1,
                    };
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(Duration::from_millis(50));
                }
            }
        }
        panic!("could not connect to {}: {last_err:?}", self.pipe_name());
    }
}

impl Drop for HostUnderTest {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct PipeClient {
    file: std::fs::File,
    decoder: FrameDecoder,
    next_id: u64,
}

impl PipeClient {
    fn read_frame(&mut self) -> serde_json::Value {
        let mut buf = [0u8; 65536];
        loop {
            if let Some(payload) = self.decoder.next_frame().expect("well-formed frame") {
                return serde_json::from_slice(&payload).expect("frame is JSON");
            }
            let n = self.file.read(&mut buf).expect("pipe read");
            assert!(n > 0, "pipe EOF while waiting for a frame");
            self.decoder.extend(&buf[..n]);
        }
    }

    fn request(&mut self, op_and_params: serde_json::Value) -> serde_json::Value {
        let id = self.next_id;
        self.next_id += 1;
        let mut req = op_and_params;
        req["id"] = id.into();
        self.file
            .write_all(&encode_frame(req.to_string().as_bytes()))
            .unwrap();
        self.file.flush().unwrap();
        let resp = self.read_frame();
        assert_eq!(resp["id"], id, "response id echoes request id");
        resp
    }

    fn ok(&mut self, op_and_params: serde_json::Value) -> serde_json::Value {
        let resp = self.request(op_and_params);
        assert!(resp.get("ok").is_some(), "expected ok, got {resp}");
        resp["ok"].clone()
    }
}

fn capture_text(client: &mut PipeClient) -> String {
    client.ok(serde_json::json!({"op": "capture"}))["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn host_serves_the_full_protocol_end_to_end() {
    let host = HostUnderTest::spawn("w1", &["cmd.exe"]);
    host.wait_for("registry file", Duration::from_secs(10), || {
        host.registry_path().exists()
    });

    // Registry entry matches PROTOCOL.md §8.
    let entry: registry::RegistryEntry =
        serde_json::from_slice(&std::fs::read(host.registry_path()).unwrap()).unwrap();
    assert_eq!(entry.v, 1);
    assert_eq!(entry.window, "w1");
    assert_eq!(entry.pipe, host.pipe_name());
    assert_eq!(entry.owner, "itest-owner-token");
    assert!(entry.agent_pid > 0);

    let mut c = host.connect();

    // hello: owner-token handshake + meta.
    let hello = c.ok(serde_json::json!({"op": "hello"}));
    assert_eq!(hello["proto"], 1);
    assert_eq!(hello["window"], "w1");
    assert_eq!(hello["owner"], "itest-owner-token");
    assert_eq!(hello["program"], "cmd.exe");
    assert_eq!(hello["agent_pid"], entry.agent_pid);
    assert!(hello["last_activity"].as_i64().unwrap() >= hello["started_at"].as_i64().unwrap());

    // Default size is tmux parity 220×50.
    let size = c.ok(serde_json::json!({"op": "size"}));
    assert_eq!(
        (size["cols"].as_u64(), size["rows"].as_u64()),
        (Some(220), Some(50))
    );

    // send_text + capture: type a command into cmd.exe and see its output.
    c.ok(serde_json::json!({"op": "send_text", "text": "echo host-e2e-done"}));
    let host_ref = &host;
    let mut probe = host.connect();
    host_ref.wait_for("echo output in capture", Duration::from_secs(15), || {
        capture_text(&mut probe).contains("host-e2e-done")
    });

    // cursor is somewhere on screen and visible for a shell prompt.
    let cursor = c.ok(serde_json::json!({"op": "cursor"}));
    assert_eq!(cursor["visible"], true);

    // alternate_on is false for a plain shell.
    assert_eq!(c.ok(serde_json::json!({"op": "alternate_on"}))["on"], false);

    // resize: last client wins.
    c.ok(serde_json::json!({"op": "resize", "cols": 100, "rows": 30}));
    let size = c.ok(serde_json::json!({"op": "size"}));
    assert_eq!(
        (size["cols"].as_u64(), size["rows"].as_u64()),
        (Some(100), Some(30))
    );

    // send_key round-trip (unknown key errors, connection stays usable).
    let resp = c.request(serde_json::json!({"op": "send_key", "key": "NoSuchKey"}));
    assert!(resp["err"].as_str().unwrap().contains("NoSuchKey"));
    c.ok(serde_json::json!({"op": "send_key", "key": "Escape"}));

    // subscribe_bytes on a separate connection: first frame is a full replay.
    let mut sub = host.connect();
    sub.ok(serde_json::json!({"op": "subscribe_bytes"}));
    let first = sub.read_frame();
    assert_eq!(first["stream"], "bytes");
    let replay = base64::engine::general_purpose::STANDARD
        .decode(first["data"].as_str().unwrap())
        .unwrap();
    let mut emu = vt100::Parser::new(30, 100, 0);
    emu.process(&replay);
    assert!(
        emu.screen().contents().contains("host-e2e-done"),
        "replay reproduces the screen: {:?}",
        emu.screen().contents()
    );

    // Live bytes follow the replay.
    c.ok(serde_json::json!({"op": "send_text", "text": "echo stream-follows"}));
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut streamed = Vec::new();
    while Instant::now() < deadline {
        let frame = sub.read_frame();
        streamed.extend(
            base64::engine::general_purpose::STANDARD
                .decode(frame["data"].as_str().unwrap())
                .unwrap(),
        );
        if String::from_utf8_lossy(&streamed).contains("stream-follows") {
            break;
        }
    }
    assert!(
        String::from_utf8_lossy(&streamed).contains("stream-follows"),
        "live PTY bytes arrive on the subscription"
    );

    // kill: window disappears like tmux kill-window.
    c.ok(serde_json::json!({"op": "kill"}));
    host.wait_for("registry entry removed", Duration::from_secs(10), || {
        !host.registry_path().exists()
    });
}

#[test]
fn child_exit_removes_registry_and_host_exits() {
    let mut host = HostUnderTest::spawn("w2", &["cmd.exe", "/c", "ping -n 2 127.0.0.1 >NUL"]);
    host.wait_for("registry file", Duration::from_secs(10), || {
        host.registry_path().exists()
    });
    host.wait_for(
        "registry cleanup after child exit",
        Duration::from_secs(20),
        || !host.registry_path().exists(),
    );
    // The host process itself exits cleanly (status 0), like a tmux window closing.
    let start = Instant::now();
    let status = loop {
        if let Some(s) = host.child.try_wait().unwrap() {
            break s;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "host process exits"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(status.success(), "clean exit, got {status:?}");
}

#[test]
fn npm_cmd_shim_note_is_covered_by_commandbuilder() {
    // Risk item from the master plan: `.cmd` shims (the `claude` npm shim) must spawn.
    // CommandBuilder resolves them; prove it with a throwaway .cmd script.
    let dir = tempfile::tempdir().unwrap();
    let shim = dir.path().join("fakeclaude.cmd");
    std::fs::write(
        &shim,
        "@echo off\r\necho shim-ran\r\nping -n 30 127.0.0.1 >NUL\r\n",
    )
    .unwrap();

    let host = HostUnderTest::spawn("w3", &[shim.to_str().unwrap()]);
    host.wait_for("registry file", Duration::from_secs(10), || {
        host.registry_path().exists()
    });
    let mut c = host.connect();
    let host_ref = &host;
    let mut probe = host.connect();
    host_ref.wait_for("shim output", Duration::from_secs(15), || {
        capture_text(&mut probe).contains("shim-ran")
    });
    c.ok(serde_json::json!({"op": "kill"}));
    host.wait_for("registry entry removed", Duration::from_secs(10), || {
        !host.registry_path().exists()
    });
}

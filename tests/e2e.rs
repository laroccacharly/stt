//! End-to-end tests: run the real `stt` binary against a fake ElevenLabs
//! server, a fake `pw-record` (microphone) and a fake `wtype` (keyboard).

// tungstenite's handshake callback signature returns a large `Err`.
#![allow(clippy::result_large_err)]

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};
use tokio_tungstenite::{
    accept_hdr_async,
    tungstenite::{
        Message,
        handshake::server::{Request, Response},
    },
};

const KEY: &str = "test-key";

/// How the fake server reacts to the client.
#[derive(Clone, Copy)]
enum Script {
    /// Commits "hello world" after the first audio chunk and "goodbye" on the final commit.
    Dictation,
    /// Rejects the session with an error message.
    AuthError,
}

/// What the fake server observed.
#[derive(Debug, Default)]
struct Seen {
    api_key: Option<String>,
    query: String,
    audio_chunks: usize,
    committed: bool,
}

async fn fake_elevenlabs(
    script: Script,
) -> (
    String,
    JoinHandle<Seen>,
    mpsc::UnboundedReceiver<&'static str>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (events, events_rx) = mpsc::unbounded_channel();
    let server = tokio::spawn(async move {
        let mut seen = Seen::default();
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_hdr_async(stream, |request: &Request, response: Response| {
            seen.api_key = request
                .headers()
                .get("xi-api-key")
                .map(|v| v.to_str().unwrap().to_owned());
            seen.query = request.uri().query().unwrap_or_default().to_owned();
            Ok(response)
        })
        .await
        .unwrap();
        let send = |value: Value| Message::text(value.to_string());
        match script {
            Script::AuthError => {
                ws.send(send(
                    json!({"message_type": "auth_error", "error": "invalid key"}),
                ))
                .await
                .unwrap();
                return seen;
            }
            Script::Dictation => {
                ws.send(send(json!({"message_type": "session_started"})))
                    .await
                    .unwrap();
            }
        }
        while let Some(Ok(message)) = ws.next().await {
            let Message::Text(text) = message else {
                continue;
            };
            let chunk: Value = serde_json::from_str(&text).unwrap();
            assert_eq!(chunk["message_type"], "input_audio_chunk");
            seen.audio_chunks += 1;
            if seen.audio_chunks == 1 {
                ws.send(send(
                    json!({"message_type": "committed_transcript", "text": "hello world"}),
                ))
                .await
                .unwrap();
                let _ = events.send("first_commit");
            }
            if chunk["commit"] == true {
                seen.committed = true;
                ws.send(send(
                    json!({"message_type": "committed_transcript", "text": " goodbye "}),
                ))
                .await
                .unwrap();
            }
        }
        seen
    });
    (url, server, events_rx)
}

/// An isolated environment: private runtime dir and fake tools on `PATH`.
struct Sandbox {
    dir: TempDir,
    server_url: String,
}

impl Sandbox {
    fn new(server_url: String) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        fs::create_dir(&bin).unwrap();
        // Endless 100 ms chunks of a quiet square wave, like a real microphone.
        script(
            &bin,
            "pw-record",
            r#"while :; do printf '\x00\x10\x00\xf0%.0s' $(seq 800); sleep 0.1; done"#,
        );
        script(&bin, "wtype", r#"printf '%s|' "$1" >> "$STT_TYPED""#);
        fs::create_dir(dir.path().join("runtime")).unwrap();
        Self { dir, server_url }
    }

    fn typed_path(&self) -> PathBuf {
        self.dir.path().join("typed")
    }

    fn typed(&self) -> String {
        fs::read_to_string(self.typed_path()).unwrap_or_default()
    }

    fn pid_file(&self) -> PathBuf {
        self.dir.path().join("runtime/stt/recording.pid")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("runtime/stt/stt.log")).unwrap_or_default()
    }

    fn stt(&self, args: &[&str]) -> Output {
        let path = format!(
            "{}:{}",
            self.dir.path().join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        Command::new(env!("CARGO_BIN_EXE_stt"))
            .args(args)
            .env("PATH", path)
            .env("XDG_RUNTIME_DIR", self.dir.path().join("runtime"))
            .env("ELEVENLABS_API_KEY", KEY)
            .env("STT_ELEVENLABS_URL", &self.server_url)
            .env("STT_TYPED", self.typed_path())
            // Headless: no overlay window and no desktop notifications.
            .env_remove("WAYLAND_DISPLAY")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
            .output()
            .unwrap()
    }

    fn status(&self) -> String {
        String::from_utf8(self.stt(&["status"]).stdout)
            .unwrap()
            .trim()
            .to_owned()
    }

    async fn wait_until(&self, what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done() {
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}\nlog:\n{}",
                self.log()
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

fn script(dir: &Path, name: &str, body: &str) {
    let path = dir.join(name);
    fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

#[tokio::test]
async fn toggle_dictates_and_types_transcript() {
    let (url, server, mut events) = fake_elevenlabs(Script::Dictation).await;
    let sandbox = Sandbox::new(url);
    assert_eq!(sandbox.status(), "idle");

    assert!(sandbox.stt(&["toggle"]).status.success());
    assert_eq!(sandbox.status(), "recording");

    // Text is typed live, while still recording.
    assert_eq!(events.recv().await, Some("first_commit"));
    sandbox
        .wait_until("live typing", || sandbox.typed() == "hello world|")
        .await;
    assert_eq!(sandbox.status(), "recording");

    // Second toggle stops, flushes the final commit and exits.
    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;
    assert_eq!(sandbox.status(), "idle");
    assert_eq!(sandbox.typed(), "hello world| goodbye|");

    let seen = server.await.unwrap();
    assert_eq!(seen.api_key.as_deref(), Some(KEY));
    assert!(
        seen.query.contains("model_id=scribe_v2_realtime"),
        "{}",
        seen.query
    );
    assert!(
        seen.query.contains("audio_format=pcm_16000"),
        "{}",
        seen.query
    );
    assert!(seen.audio_chunks >= 2, "{seen:?}");
    assert!(seen.committed, "final chunk must carry commit: true");
}

#[tokio::test]
async fn server_error_ends_session_and_cleans_up() {
    let (url, server, _events) = fake_elevenlabs(Script::AuthError).await;
    let sandbox = Sandbox::new(url);

    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;
    server.await.unwrap();

    assert_eq!(sandbox.status(), "idle");
    assert_eq!(sandbox.typed(), "");
    assert!(
        sandbox.log().contains("ElevenLabs: invalid key"),
        "log:\n{}",
        sandbox.log()
    );
}

#[tokio::test]
async fn stale_pid_file_is_ignored() {
    let sandbox = Sandbox::new("ws://127.0.0.1:9".into());
    fs::create_dir_all(sandbox.pid_file().parent().unwrap()).unwrap();
    // PID 1 exists but is not an stt recorder.
    fs::write(sandbox.pid_file(), "1").unwrap();
    assert_eq!(sandbox.status(), "idle");
}

#[tokio::test]
async fn tls_failure_is_reported_not_a_crash() {
    // A server that accepts TCP and hangs up, so the TLS handshake fails.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        while let Ok((stream, _)) = listener.accept().await {
            drop(stream);
        }
    });
    let sandbox = Sandbox::new(format!("wss://127.0.0.1:{port}"));

    assert!(sandbox.stt(&["toggle"]).status.success(), "log:\n{}", sandbox.log());
    sandbox.wait_until("recorder exit", || !sandbox.pid_file().exists()).await;

    let log = sandbox.log();
    assert!(log.contains("ElevenLabs connection failed"), "log:\n{log}");
    assert!(!log.contains("panicked"), "log:\n{log}");
}

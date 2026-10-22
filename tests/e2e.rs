//! End-to-end tests: run the real `stt` binary against fake ElevenLabs,
//! Cartesia and OpenRouter servers, a fake `pw-record` (microphone), a fake
//! `wtype` (keyboard) and a fake `wl-copy` (clipboard). The `#[ignore]`d
//! tests use the real Cartesia API and the real local models, and only run
//! with `cargo test -- --ignored`.

// tungstenite's handshake callback signature returns a large `Err`.
#![allow(clippy::result_large_err)]

use std::{
    collections::HashMap,
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tempfile::TempDir;
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
    sync::mpsc,
    task::JoinHandle,
};
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
    /// Like `Dictation`, but answers the final commit with an error.
    FinalError,
    /// Like `Dictation`, but never answers the final commit.
    NoFinal,
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
            Script::Dictation | Script::FinalError | Script::NoFinal => {
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
                let reply = match script {
                    Script::FinalError => {
                        json!({"message_type": "transcriber_error", "error": "boom"})
                    }
                    Script::NoFinal => continue,
                    _ => json!({"message_type": "committed_transcript", "text": " goodbye "}),
                };
                ws.send(send(reply)).await.unwrap();
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
    extra_env: Vec<(&'static str, String)>,
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
        // Typed text and pasted clipboard contents land in the same log; the
        // paste keystroke itself isn't logged.
        script(
            &bin,
            "wtype",
            r#"[ "$1" = -M ] || printf '%s|' "$1" >> "$STT_TYPED""#,
        );
        script(
            &bin,
            "wl-copy",
            r#"cat >> "$STT_TYPED"; printf '|' >> "$STT_TYPED""#,
        );
        fs::create_dir(dir.path().join("runtime")).unwrap();
        Self {
            dir,
            server_url,
            extra_env: Vec::new(),
        }
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
        self.command(args).output().unwrap()
    }

    /// Run stt with `input` on stdin.
    fn stt_with_input(&self, args: &[&str], input: &str) -> Output {
        let mut child = self
            .command(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(input.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    fn command(&self, args: &[&str]) -> Command {
        let path = format!(
            "{}:{}",
            self.dir.path().join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_stt"));
        command
            .args(args)
            .env("PATH", path)
            .env("XDG_RUNTIME_DIR", self.dir.path().join("runtime"))
            .env("XDG_CONFIG_HOME", self.dir.path().join("config"))
            .env("XDG_CACHE_HOME", self.dir.path().join("cache"))
            .env("ELEVENLABS_API_KEY", KEY)
            .env("STT_ELEVENLABS_URL", &self.server_url)
            .env("STT_TYPED", self.typed_path())
            .envs(self.extra_env.iter().map(|(k, v)| (k, v)))
            // Headless: no overlay window and no desktop notifications.
            .env_remove("WAYLAND_DISPLAY")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent");
        command
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
        sandbox
            .log()
            .contains("notify: STT error: ElevenLabs: invalid key"),
        "log:\n{}",
        sandbox.log()
    );
}

/// Records one live segment, stops, and returns the log once the recorder exits.
async fn dictate_and_stop(script: Script) -> (Sandbox, String) {
    let (url, _server, mut events) = fake_elevenlabs(script).await;
    let sandbox = Sandbox::new(url);
    assert!(sandbox.stt(&["toggle"]).status.success());
    assert_eq!(events.recv().await, Some("first_commit"));
    sandbox
        .wait_until("live typing", || sandbox.typed() == "hello world|")
        .await;
    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;
    let log = sandbox.log();
    (sandbox, log)
}

#[tokio::test]
async fn error_on_final_commit_is_notified() {
    let (sandbox, log) = dictate_and_stop(Script::FinalError).await;
    assert!(
        log.contains("notify: STT error: ElevenLabs: boom"),
        "log:\n{log}"
    );
    assert_eq!(sandbox.typed(), "hello world|");
}

#[tokio::test]
async fn missing_final_transcript_is_notified() {
    let (_sandbox, log) = dictate_and_stop(Script::NoFinal).await;
    assert!(
        log.contains("notify: STT error: ElevenLabs sent no final transcript"),
        "log:\n{log}"
    );
}

#[tokio::test]
async fn microphone_dying_is_notified() {
    let (url, _server, _events) = fake_elevenlabs(Script::Dictation).await;
    let sandbox = Sandbox::new(url);
    script(&sandbox.dir.path().join("bin"), "pw-record", "exit 1");
    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;
    let log = sandbox.log();
    assert!(
        log.contains("notify: STT error: Microphone stopped unexpectedly"),
        "log:\n{log}"
    );
}

#[tokio::test]
async fn typing_failure_is_notified() {
    let (url, _server, mut events) = fake_elevenlabs(Script::Dictation).await;
    let sandbox = Sandbox::new(url);
    script(
        &sandbox.dir.path().join("bin"),
        "wtype",
        "echo 'no focused window' >&2; exit 1",
    );
    assert!(sandbox.stt(&["toggle"]).status.success());
    assert_eq!(events.recv().await, Some("first_commit"));
    sandbox
        .wait_until("typing failure", || {
            sandbox
                .log()
                .contains("notify: STT typing failed: no focused window")
        })
        .await;
    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;
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

    assert!(
        sandbox.stt(&["toggle"]).status.success(),
        "log:\n{}",
        sandbox.log()
    );
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;

    let log = sandbox.log();
    assert!(
        log.contains("notify: STT error: ElevenLabs connection failed"),
        "log:\n{log}"
    );
    assert!(!log.contains("panicked"), "log:\n{log}");
}

/// A request a fake HTTP server received.
#[derive(Debug)]
struct HttpRequest {
    request_line: String,
    /// Header names are lowercase.
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

impl HttpRequest {
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// Answers one HTTP request with `status` and `reply`.
fn fake_http(listener: TcpListener, status: u16, reply: Value) -> JoinHandle<HttpRequest> {
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line).await.unwrap();
        let mut headers = HashMap::new();
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            let (name, value) = line.split_once(": ").unwrap();
            headers.insert(name.to_ascii_lowercase(), value.to_owned());
        }
        let length: usize = headers
            .get("content-length")
            .map_or(0, |length| length.parse().unwrap());
        let mut body = vec![0; length];
        reader.read_exact(&mut body).await.unwrap();
        let reply = reply.to_string();
        let response = format!(
            "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{reply}",
            reply.len()
        );
        reader
            .get_mut()
            .write_all(response.as_bytes())
            .await
            .unwrap();
        HttpRequest {
            request_line: request_line.trim_end().to_owned(),
            headers,
            body,
        }
    })
}

/// A fake HTTP server on its own port; `scheme` prefixes the returned URL.
async fn fake_http_at(
    scheme: &str,
    status: u16,
    reply: Value,
) -> (String, JoinHandle<HttpRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("{scheme}://{}", listener.local_addr().unwrap());
    (url, fake_http(listener, status, reply))
}

async fn fake_openrouter(status: u16, reply: Value) -> (String, JoinHandle<HttpRequest>) {
    fake_http_at("http", status, reply).await
}

fn openrouter_sandbox(url: String) -> Sandbox {
    openrouter_sandbox_with_elevenlabs(url, "ws://127.0.0.1:9".into())
}

/// Uses OpenRouter at `url`, with the ElevenLabs fallback at `elevenlabs_url`.
fn openrouter_sandbox_with_elevenlabs(url: String, elevenlabs_url: String) -> Sandbox {
    let mut sandbox = Sandbox::new(elevenlabs_url);
    sandbox.extra_env = vec![
        ("OPENROUTER_API_KEY", KEY.into()),
        ("STT_OPENROUTER_URL", url),
    ];
    assert!(
        sandbox
            .stt(&["provider", "set", "openrouter"])
            .status
            .success()
    );
    sandbox
}

/// Records for a moment, stops, and waits for the recorder to exit.
async fn record_briefly(sandbox: &Sandbox) {
    assert!(sandbox.stt(&["toggle"]).status.success());
    assert_eq!(sandbox.status(), "recording");
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;
}

#[tokio::test]
async fn openrouter_transcribes_recording_on_stop() {
    let (url, server) = fake_openrouter(200, json!({"text": " hello there "})).await;
    let sandbox = openrouter_sandbox(url);
    record_briefly(&sandbox).await;
    assert_eq!(sandbox.typed(), "hello there|", "log:\n{}", sandbox.log());

    let request = server.await.unwrap();
    assert_eq!(
        request.request_line,
        "POST /api/v1/audio/transcriptions HTTP/1.1"
    );
    assert_eq!(
        request.headers.get("authorization"),
        Some(&format!("Bearer {KEY}"))
    );
    let body = request.json();
    assert_eq!(body["model"], "microsoft/mai-transcribe-2");
    assert_eq!(body["input_audio"]["format"], "wav");
    let audio = body["input_audio"]["data"].as_str().unwrap();
    assert!(audio.starts_with("UklGR"), "base64 of RIFF: {audio:.20}");
    assert!(audio.len() > 10_000, "should hold several chunks of audio");
}

#[tokio::test]
async fn openrouter_failure_falls_back_to_elevenlabs() {
    let (url, _openrouter) = fake_openrouter(
        502,
        json!({"error": {"message": "Provider returned error"}}),
    )
    .await;
    let (elevenlabs_url, elevenlabs) =
        fake_http_at("ws", 200, json!({"text": " saved by the fallback "})).await;
    let sandbox = openrouter_sandbox_with_elevenlabs(url, elevenlabs_url);
    record_briefly(&sandbox).await;
    let log = sandbox.log();
    assert_eq!(sandbox.typed(), "saved by the fallback|", "log:\n{log}");
    assert!(
        log.contains(
            "OpenRouter: 502 Bad Gateway: Provider returned error; falling back to ElevenLabs"
        ),
        "log:\n{log}"
    );
    assert!(!log.contains("STT error"), "log:\n{log}");

    let request = elevenlabs.await.unwrap();
    assert_eq!(request.request_line, "POST /v1/speech-to-text HTTP/1.1");
    assert_eq!(request.headers.get("xi-api-key"), Some(&KEY.to_owned()));
    let body = String::from_utf8_lossy(&request.body);
    assert!(body.contains("scribe_v2\r\n"), "{body:.500}");
    assert!(body.contains("pcm_s16le_16\r\n"), "{body:.500}");
    assert!(body.len() > 10_000, "should hold several chunks of audio");
}

#[tokio::test]
async fn openrouter_and_fallback_errors_are_notified() {
    let (url, _openrouter) = fake_openrouter(
        401,
        json!({"error": {"message": "No auth credentials found"}}),
    )
    .await;
    let (elevenlabs_url, _elevenlabs) =
        fake_http_at("ws", 429, json!({"detail": "quota exceeded"})).await;
    let sandbox = openrouter_sandbox_with_elevenlabs(url, elevenlabs_url);
    record_briefly(&sandbox).await;
    let log = sandbox.log();
    assert!(
        log.contains(
            "notify: STT error: OpenRouter: 401 Unauthorized: No auth credentials found; \
             the ElevenLabs fallback failed too: ElevenLabs: 429 Too Many Requests: \
             {\"detail\":\"quota exceeded\"}"
        ),
        "log:\n{log}"
    );
    assert_eq!(sandbox.typed(), "");
}

#[tokio::test]
async fn elevenlabs_batch_transcribes_recording_on_stop() {
    let (url, server) = fake_http_at("ws", 200, json!({"text": " hello batch "})).await;
    let sandbox = Sandbox::new(url);
    assert!(
        sandbox
            .stt(&["provider", "set", "elevenlabs-batch"])
            .status
            .success()
    );
    record_briefly(&sandbox).await;
    let log = sandbox.log();
    assert_eq!(sandbox.typed(), "hello batch|", "log:\n{log}");
    assert!(!log.contains("STT error"), "log:\n{log}");

    let request = server.await.unwrap();
    assert_eq!(request.request_line, "POST /v1/speech-to-text HTTP/1.1");
    assert_eq!(request.headers.get("xi-api-key"), Some(&KEY.to_owned()));
    let body = String::from_utf8_lossy(&request.body);
    assert!(body.contains("scribe_v2\r\n"), "{body:.500}");
    assert!(body.contains("pcm_s16le_16\r\n"), "{body:.500}");
    assert!(body.len() > 10_000, "should hold several chunks of audio");
}

#[tokio::test]
async fn elevenlabs_batch_error_is_notified() {
    let (url, _server) = fake_http_at("ws", 401, json!({"detail": "invalid key"})).await;
    let sandbox = Sandbox::new(url);
    assert!(
        sandbox
            .stt(&["provider", "set", "elevenlabs-batch"])
            .status
            .success()
    );
    record_briefly(&sandbox).await;
    let log = sandbox.log();
    assert!(
        log.contains(
            "notify: STT error: ElevenLabs: 401 Unauthorized: {\"detail\":\"invalid key\"}"
        ),
        "log:\n{log}"
    );
    assert_eq!(sandbox.typed(), "");
}

#[test]
fn old_elevenlabs_name_means_realtime() {
    let sandbox = Sandbox::new("ws://127.0.0.1:9".into());
    let ls =
        |sandbox: &Sandbox| String::from_utf8(sandbox.stt(&["provider", "ls"]).stdout).unwrap();
    let config_dir = sandbox.dir.path().join("config/stt");
    fs::create_dir_all(&config_dir).unwrap();
    fs::write(
        config_dir.join("config.json"),
        r#"{"provider": "elevenlabs"}"#,
    )
    .unwrap();
    assert!(ls(&sandbox).starts_with("* elevenlabs-realtime\n"));

    assert!(
        sandbox
            .stt(&["provider", "set", "cartesia"])
            .status
            .success()
    );
    assert!(
        sandbox
            .stt(&["provider", "set", "elevenlabs"])
            .status
            .success()
    );
    assert!(ls(&sandbox).starts_with("* elevenlabs-realtime\n"));
}

#[test]
fn login_without_env_keys_fails_without_touching_keyring() {
    let output = Command::new(env!("CARGO_BIN_EXE_stt"))
        .arg("login")
        .env_remove("ELEVENLABS_API_KEY")
        .env_remove("CARTESIA_API_KEY")
        .env_remove("OPENROUTER_API_KEY")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout
            .matches("ELEVENLABS_API_KEY not set; skipped")
            .count(),
        1,
        "{stdout}"
    );
    assert!(
        stdout.contains("CARTESIA_API_KEY not set; skipped"),
        "{stdout}"
    );
    assert!(
        stdout.contains("OPENROUTER_API_KEY not set; skipped"),
        "{stdout}"
    );
}

#[test]
fn provider_set_is_remembered_and_listed() {
    let sandbox = Sandbox::new("ws://127.0.0.1:9".into());
    let names: Vec<String> = provider_names(&sandbox);
    for name in ["elevenlabs-realtime", "openrouter", "bark-3.0-clean"] {
        assert!(names.iter().any(|listed| listed == name), "{names:?}");
    }
    assert_eq!(
        selected_provider(&sandbox).as_deref(),
        Some("elevenlabs-realtime")
    );

    let set = sandbox.stt(&["provider", "set", "openrouter"]);
    assert!(set.status.success());
    assert_eq!(String::from_utf8_lossy(&set.stdout), "Using OpenRouter\n");
    assert_eq!(selected_provider(&sandbox).as_deref(), Some("openrouter"));
    let config: Value = serde_json::from_str(
        &fs::read_to_string(sandbox.dir.path().join("config/stt/config.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(config, json!({"provider": "openrouter"}));

    assert!(!sandbox.stt(&["provider", "set", "nope"]).status.success());
    assert_eq!(selected_provider(&sandbox).as_deref(), Some("openrouter"));
}

#[test]
fn provider_set_stops_local_model_servers() {
    let sandbox = Sandbox::new("ws://127.0.0.1:9".into());
    let runtime = sandbox.dir.path().join("runtime/stt");
    fs::create_dir_all(&runtime).unwrap();
    // A fake server, in its own process group like a real one, with a worker.
    let spawn = |args: &[&Path]| {
        let mut command = Command::new("sh");
        command.args(["-c", "sleep 60 & wait"]).args(args);
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
        command.spawn().unwrap()
    };
    let socket = runtime.join("tinybark_server.sock");
    fs::write(&socket, "").unwrap();
    let mut server = spawn(&[&socket]);
    fs::write(runtime.join("tinybark_server.pid"), server.id().to_string()).unwrap();
    // A PID file whose process serves no socket: left alone.
    let mut stranger = spawn(&[]);
    fs::write(runtime.join("phonon.pid"), stranger.id().to_string()).unwrap();

    let set = sandbox.stt(&["provider", "set", "openrouter"]);
    assert!(set.status.success());
    assert!(server.wait().is_ok());
    assert!(!runtime.join("tinybark_server.pid").exists());
    assert!(!socket.exists());
    assert!(stranger.try_wait().unwrap().is_none());
    let _ = stranger.kill();
}

#[test]
fn provider_set_without_argument_asks() {
    let sandbox = Sandbox::new("ws://127.0.0.1:9".into());
    let names: Vec<String> = provider_names(&sandbox);
    let cartesia_number: usize = names.iter().position(|name| name == "cartesia").unwrap() + 1;

    let set = sandbox.stt_with_input(&["provider", "set"], &format!("{cartesia_number}\n"));
    assert!(set.status.success());
    let stdout: String = String::from_utf8_lossy(&set.stdout).into_owned();
    assert!(
        stdout.contains("Provider [elevenlabs-realtime]: Using Cartesia"),
        "{stdout}"
    );
    assert_eq!(selected_provider(&sandbox).as_deref(), Some("cartesia"));

    let set = sandbox.stt_with_input(&["provider", "set"], "OpenRouter\n");
    assert!(set.status.success());
    assert_eq!(selected_provider(&sandbox).as_deref(), Some("openrouter"));

    // Enter keeps the current provider.
    assert!(
        sandbox
            .stt_with_input(&["provider", "set"], "\n")
            .status
            .success()
    );
    assert_eq!(selected_provider(&sandbox).as_deref(), Some("openrouter"));

    for bad in [
        format!("{}\n", names.len() + 1),
        "0\n".to_owned(),
        "nope\n".to_owned(),
    ] {
        let set = sandbox.stt_with_input(&["provider", "set"], &bad);
        assert!(!set.status.success(), "{bad:?}");
    }
    assert_eq!(selected_provider(&sandbox).as_deref(), Some("openrouter"));
}

/// `stt provider ls` output, one entry per line.
fn provider_lines(sandbox: &Sandbox) -> Vec<String> {
    String::from_utf8(sandbox.stt(&["provider", "ls"]).stdout)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

fn provider_names(sandbox: &Sandbox) -> Vec<String> {
    provider_lines(sandbox)
        .into_iter()
        .map(|line| line[2..].to_owned())
        .collect()
}

fn selected_provider(sandbox: &Sandbox) -> Option<String> {
    provider_lines(sandbox)
        .into_iter()
        .find_map(|line| line.strip_prefix("* ").map(str::to_owned))
}

/// What the fake Cartesia server observed.
#[derive(Debug, Default)]
struct CartesiaSeen {
    api_key: Option<String>,
    query: String,
    audio_bytes: usize,
    finalized: bool,
}

/// Sends a live delta after the first audio, then on `finalize` the rest of
/// the transcript and `flush_done`; or, with `error`, rejects the session.
async fn fake_cartesia(error: bool) -> (String, JoinHandle<CartesiaSeen>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut seen = CartesiaSeen::default();
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = accept_hdr_async(stream, |request: &Request, response: Response| {
            assert_eq!(request.uri().path(), "/stt/websocket");
            seen.api_key = request
                .headers()
                .get("x-api-key")
                .map(|v| v.to_str().unwrap().to_owned());
            seen.query = request.uri().query().unwrap_or_default().to_owned();
            Ok(response)
        })
        .await
        .unwrap();
        let send = |value: Value| Message::text(value.to_string());
        let delta =
            |text: &str| send(json!({"type": "transcript", "is_final": true, "text": text}));
        if error {
            let reply = json!({"type": "error", "status_code": 401, "title": "Unauthorized", "message": "invalid key"});
            ws.send(send(reply)).await.unwrap();
            return seen;
        }
        while let Some(Ok(message)) = ws.next().await {
            match message {
                Message::Binary(audio) => {
                    if seen.audio_bytes == 0 {
                        ws.send(delta(" Hello")).await.unwrap();
                    }
                    seen.audio_bytes += audio.len();
                }
                Message::Text(text) if text.as_str() == "finalize" => {
                    seen.finalized = true;
                    // An interim result must not be typed.
                    let interim = json!({"type": "transcript", "is_final": false, "text": " wor"});
                    ws.send(send(interim)).await.unwrap();
                    ws.send(delta(" world")).await.unwrap();
                    ws.send(delta(".")).await.unwrap();
                    ws.send(send(json!({"type": "flush_done", "request_id": "r"})))
                        .await
                        .unwrap();
                }
                _ => {}
            }
        }
        seen
    });
    (url, server)
}

fn cartesia_sandbox(url: Option<String>) -> Sandbox {
    let mut sandbox = Sandbox::new("ws://127.0.0.1:9".into());
    if let Some(url) = url {
        sandbox.extra_env = vec![("CARTESIA_API_KEY", KEY.into()), ("STT_CARTESIA_URL", url)];
    }
    assert!(
        sandbox
            .stt(&["provider", "set", "cartesia"])
            .status
            .success()
    );
    sandbox
}

#[tokio::test]
async fn cartesia_types_deltas_live_and_on_finalize() {
    let (url, server) = fake_cartesia(false).await;
    let sandbox = cartesia_sandbox(Some(url));

    assert!(sandbox.stt(&["toggle"]).status.success());
    // The first delta is typed while still recording, without its leading space.
    sandbox
        .wait_until("live typing", || sandbox.typed() == "Hello|")
        .await;
    assert_eq!(sandbox.status(), "recording");

    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;
    // Later deltas keep their own spacing: "Hello world."
    assert_eq!(
        sandbox.typed(),
        "Hello| world|.|",
        "log:\n{}",
        sandbox.log()
    );
    assert!(
        !sandbox.log().contains("STT error"),
        "log:\n{}",
        sandbox.log()
    );

    let seen = server.await.unwrap();
    assert_eq!(seen.api_key.as_deref(), Some(KEY));
    for param in [
        "model=ink-2",
        "encoding=pcm_s16le",
        "sample_rate=16000",
        "cartesia_version=",
    ] {
        assert!(seen.query.contains(param), "{param} in {}", seen.query);
    }
    assert!(seen.audio_bytes > 0, "{seen:?}");
    assert!(seen.finalized, "stopping must send finalize");
}

#[tokio::test]
async fn cartesia_error_is_notified() {
    let (url, server) = fake_cartesia(true).await;
    let sandbox = cartesia_sandbox(Some(url));
    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;
    server.await.unwrap();
    let log = sandbox.log();
    assert!(
        log.contains("notify: STT error: Cartesia: invalid key"),
        "log:\n{log}"
    );
    assert_eq!(sandbox.typed(), "");
}

/// Dictates a recorded sentence to the real Cartesia API, using
/// `CARTESIA_API_KEY` from the environment.
#[tokio::test]
#[ignore = "calls the real Cartesia API"]
async fn cartesia_transcribes_real_speech() {
    assert!(
        std::env::var("CARTESIA_API_KEY").is_ok_and(|key| !key.is_empty()),
        "set CARTESIA_API_KEY"
    );
    let sandbox = cartesia_sandbox(None);
    // "The quick brown fox jumps over the lazy dog.": 16 kHz s16le mono,
    // played in real time 100 ms at a time, then silence.
    let speech = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/hello.pcm");
    script(
        &sandbox.dir.path().join("bin"),
        "pw-record",
        &format!(
            "for i in $(seq 0 30); do dd if='{}' bs=3200 skip=$i count=1 status=none; sleep 0.1; done\n\
             while :; do head -c 3200 /dev/zero; sleep 0.1; done",
            speech.display()
        ),
    );

    assert!(sandbox.stt(&["toggle"]).status.success());
    tokio::time::sleep(Duration::from_secs(4)).await;
    let live = sandbox.typed();
    assert!(sandbox.stt(&["toggle"]).status.success());
    sandbox
        .wait_until("recorder exit", || !sandbox.pid_file().exists())
        .await;

    let log = sandbox.log();
    assert!(log.contains("Cartesia WebSocket connected"), "log:\n{log}");
    assert!(!log.contains("STT error"), "log:\n{log}");
    assert!(!live.is_empty(), "text should be typed while speaking");
    let typed = sandbox.typed().replace('|', "").to_lowercase();
    assert!(
        typed.starts_with("the quick brown fox jumps over the lazy dog"),
        "typed: {typed}\nlog:\n{log}"
    );
}

/// Kills a local model server, and the `uv` above it, when dropped.
struct ServerGuard(PathBuf);

impl ServerGuard {
    fn pid(&self) -> Option<String> {
        fs::read_to_string(&self.0).ok()
    }
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if let Some(pid) = self.pid() {
            // The server runs in its own process group.
            let _ = Command::new("kill")
                .args(["--", &format!("-{}", pid.trim())])
                .status();
        }
    }
}

/// Lowercase words only, for comparing transcripts. "Mr." counts as "mister":
/// Bark abbreviates it.
fn words(text: &str) -> String {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '\'')
        .filter(|word| !word.is_empty())
        .map(|word| if word == "mr" { "mister" } else { word })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Runs the local model tests one at a time: they share the real cache and
/// each loads a model on the CPU.
static LOCAL_MODELS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Dictates two recorded sentences to the real local `provider`, through the
/// real `uv`, and checks the server is started once and kept up.
async fn local_transcribes_real_speech(provider: &str) {
    let _one_at_a_time = LOCAL_MODELS.lock().await;
    let mut sandbox = Sandbox::new("ws://127.0.0.1:9".into());
    // The real cache, where uv keeps the model's environment.
    let cache = std::env::var("XDG_CACHE_HOME")
        .unwrap_or_else(|_| format!("{}/.cache", std::env::var("HOME").unwrap()));
    sandbox.extra_env = vec![("XDG_CACHE_HOME", cache)];
    let name = match provider {
        "phonon-2" => "phonon",
        "parakeet-redux" => "parakeet_redux",
        "onnxbark" => "onnxbark",
        _ => "bark",
    };
    let server = ServerGuard(sandbox.dir.path().join(format!("runtime/stt/{name}.pid")));
    let set = sandbox.stt(&["provider", "set", provider]);
    assert!(
        set.status.success(),
        "{}",
        String::from_utf8_lossy(&set.stderr)
    );

    let mut pid = None;
    // Real speech, 16 kHz s16le mono: a recording, then a LibriSpeech clip.
    for (fixture, expected) in [
        ("hello.pcm", "the quick brown fox jumps over the lazy dog"),
        (
            "quilter.pcm",
            "mister quilter is the apostle of the middle classes and we are glad to welcome his gospel",
        ),
    ] {
        let speech = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(fixture);
        let chunks = fs::metadata(&speech).unwrap().len().div_ceil(3200);
        // Played in real time 100 ms at a time, then silence.
        script(
            &sandbox.dir.path().join("bin"),
            "pw-record",
            &format!(
                "for i in $(seq 0 {}); do dd if='{}' bs=3200 skip=$i count=1 status=none; sleep 0.1; done\n\
                 while :; do head -c 3200 /dev/zero; sleep 0.1; done",
                chunks - 1,
                speech.display()
            ),
        );
        let _ = fs::remove_file(sandbox.typed_path());

        assert!(sandbox.stt(&["toggle"]).status.success());
        tokio::time::sleep(Duration::from_millis(chunks * 100 + 500)).await;
        assert!(sandbox.stt(&["toggle"]).status.success());
        sandbox
            .wait_until("recorder exit", || !sandbox.pid_file().exists())
            .await;

        let log = sandbox.log();
        assert!(!log.contains("STT error"), "log:\n{log}");
        assert_eq!(
            words(&sandbox.typed().replace('|', "")),
            expected,
            "log:\n{log}"
        );
        let now = server.pid();
        assert!(now.is_some());
        if pid.is_some() {
            assert_eq!(now, pid, "the server should stay up between dictations");
        }
        pid = now;
    }
}

#[tokio::test]
#[ignore = "runs the real Phonon-2 model through uv"]
async fn phonon_transcribes_real_speech() {
    local_transcribes_real_speech("phonon-2").await;
}

#[tokio::test]
#[ignore = "runs the real Parakeet Redux model through uv"]
async fn parakeet_redux_transcribes_real_speech() {
    local_transcribes_real_speech("parakeet-redux").await;
}

#[tokio::test]
#[ignore = "runs the real Underdog Bark 3.0 Clean model through uv"]
async fn bark_transcribes_real_speech() {
    local_transcribes_real_speech("bark-3.0-clean").await;
}

#[tokio::test]
#[ignore = "runs the real Underdog Bark 3.0 Clean model through tinybark's ONNX backend"]
async fn onnxbark_transcribes_real_speech() {
    local_transcribes_real_speech("onnxbark").await;
}

//! Live dictation for Hyprland, via ElevenLabs, Cartesia, OpenRouter or a
//! local model.

mod audio;
mod history;
mod hyprland;
mod overlay;
mod provider;

use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Output, Stdio};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use rustix::process::{Pid, Signal, kill_process};
use tokio::io::AsyncWriteExt;
use tokio::signal::unix::{self, Signal as UnixSignal, SignalKind};
use tokio::sync::mpsc;
use tokio::time;

use audio::Microphone;
use history::History;
use overlay::{Event, Overlay, Phase};
use provider::{Backend, Live, Provider, ReceiveDyn, TransmitDyn, Update, elevenlabs, local};

/// How long to wait for the final transcript after stopping.
const FINISH_TIMEOUT: Duration = Duration::from_millis(4500);
/// Notifications share one id so each replaces the previous.
const NOTIFICATION_ID: u32 = 47511;
/// Start/stop notifications are off for now; errors always notify.
const ENABLE_STATUS_NOTIFICATIONS: bool = false;
/// Batch transcripts arrive whole, so paste them through the clipboard
/// (overwriting it) instead of typing them key by key. Set to false to type them.
const PASTE_BATCH_TRANSCRIPTS: bool = true;

#[derive(Parser)]
#[command(version, about = "Live speech to text for Hyprland")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start dictation, or stop it if it is running (default).
    Toggle,
    /// Print `recording` or `idle`.
    Status,
    /// Run a dictation session in the foreground.
    Record,
    /// Copy API keys from the environment (ELEVENLABS_API_KEY,
    /// CARTESIA_API_KEY, OPENROUTER_API_KEY) into the keyring.
    Login,
    /// List or choose the transcription provider.
    #[command(subcommand)]
    Provider(ProviderCmd),
    /// Browse saved dictations.
    #[command(subcommand)]
    History(HistoryCmd),
    /// Bind End to `stt toggle` in Hyprland.
    Install,
    /// Remove the End binding.
    Uninstall,
    /// Show the overlay with synthetic audio, for testing.
    #[command(hide = true)]
    Demo,
}

#[derive(Subcommand)]
enum ProviderCmd {
    /// List providers, marking the one in use.
    Ls,
    /// Use this provider from now on; asks which one if not given.
    Set {
        #[arg(value_enum)]
        provider: Option<Provider>,
    },
}

#[derive(Subcommand)]
enum HistoryCmd {
    /// List the 10 most recent dictations, newest first.
    Ls,
}

/// How many dictations `stt history ls` lists.
const HISTORY_LS_LIMIT: usize = 10;

struct Paths {
    runtime: PathBuf,
    pid: PathBuf,
    log: PathBuf,
}

impl Paths {
    fn new() -> Self {
        let base: PathBuf = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw()))
            });
        let runtime: PathBuf = base.join("stt");
        Self {
            pid: runtime.join("recording.pid"),
            log: runtime.join("stt.log"),
            runtime,
        }
    }

    /// PID of the running recorder, ignoring stale files.
    fn recording_pid(&self) -> Option<Pid> {
        let pid: i32 = fs::read_to_string(&self.pid).ok()?.trim().parse().ok()?;
        let cmdline: Vec<u8> = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let args: Vec<&[u8]> = cmdline.split(|&b| b == 0).collect();
        let is_stt: bool = args.first().is_some_and(|exe| exe.ends_with(b"stt"));
        (is_stt && args.iter().skip(1).any(|arg| *arg == b"record"))
            .then(|| Pid::from_raw(pid))
            .flatten()
    }
}

fn main() {
    let cli: Cli = Cli::parse();
    if let Err(error) = run(&cli.command.unwrap_or(Cmd::Toggle)) {
        let _ = notify("STT error", &format!("{error:#}"), 5000).join();
        std::process::exit(1);
    }
}

fn run(command: &Cmd) -> Result<()> {
    let paths: Paths = Paths::new();
    fs::create_dir_all(&paths.runtime)?;
    match command {
        Cmd::Toggle => toggle(&paths),
        Cmd::Status => {
            println!(
                "{}",
                if paths.recording_pid().is_some() {
                    "recording"
                } else {
                    "idle"
                }
            );
            Ok(())
        }
        Cmd::Record => tokio::runtime::Runtime::new()?.block_on(record(&paths)),
        Cmd::Login => login(),
        Cmd::Provider(ProviderCmd::Ls) => {
            let current: Provider = Provider::current()?;
            for &provider in Provider::ALL {
                let marker: &str = if provider == current { "*" } else { " " };
                println!("{marker} {}", provider.name());
            }
            Ok(())
        }
        Cmd::Provider(ProviderCmd::Set { provider }) => {
            let provider: Provider = match provider {
                Some(provider) => *provider,
                None => choose_provider()?,
            };
            if let Some(model) = provider.local_model() {
                local::prepare(model)?;
            }
            provider.save()?;
            local::stop_others(&paths.runtime, provider.local_model())?;
            println!("Using {}", provider.label());
            Ok(())
        }
        Cmd::History(HistoryCmd::Ls) => {
            for entry in History::open_default()?.recent(HISTORY_LS_LIMIT)? {
                let transcript: &str = if entry.transcript.is_empty() {
                    "(no speech)"
                } else {
                    &entry.transcript
                };
                println!(
                    "{}  {} ({})  {transcript}",
                    entry.time, entry.provider, entry.model
                );
            }
            Ok(())
        }
        Cmd::Install => hyprland::install(),
        Cmd::Uninstall => hyprland::uninstall(),
        Cmd::Demo => {
            demo();
            Ok(())
        }
    }
}

/// Ask on stdin which provider to use, by number or name.
fn choose_provider() -> Result<Provider> {
    let current: Provider = Provider::current()?;
    for (i, &provider) in Provider::ALL.iter().enumerate() {
        let marker: &str = if provider == current { "*" } else { " " };
        println!("{marker} {}. {}", i + 1, provider.name());
    }
    print!("Provider [{}]: ", current.name());
    std::io::stdout().flush()?;
    let mut answer: String = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer: &str = answer.trim();
    if answer.is_empty() {
        return Ok(current);
    }
    if let Ok(number) = answer.parse::<usize>() {
        if (1..=Provider::ALL.len()).contains(&number) {
            return Ok(Provider::ALL[number - 1]);
        }
        bail!("Unknown provider: {answer}");
    }
    for &provider in Provider::ALL {
        if provider.matches(answer) {
            return Ok(provider);
        }
    }
    bail!("Unknown provider: {answer}")
}

fn login() -> Result<()> {
    let runtime: tokio::runtime::Runtime = tokio::runtime::Runtime::new()?;
    let mut saved: usize = 0;
    let mut seen: Vec<&str> = Vec::new();
    for &provider in Provider::ALL {
        let name: &str = match provider.key_name() {
            Some(name) => name,
            // Local models need no key.
            None => continue,
        };
        // Providers from the same company share a key.
        if seen.contains(&name) {
            continue;
        }
        seen.push(name);
        let key: Option<String> = std::env::var(name)
            .ok()
            .filter(|key| !key.trim().is_empty());
        if let Some(key) = key {
            runtime.block_on(provider::save_api_key(provider, key.trim()))?;
            println!("Saved {} API key to the keyring", provider.company());
            saved += 1;
        } else {
            println!("{name} not set; skipped");
        }
    }
    if saved == 0 {
        bail!(
            "No API keys in the environment: set ELEVENLABS_API_KEY, CARTESIA_API_KEY or OPENROUTER_API_KEY"
        );
    }
    Ok(())
}

fn toggle(paths: &Paths) -> Result<()> {
    if let Some(pid) = paths.recording_pid() {
        kill_process(pid, Signal::USR2)?;
        return Ok(());
    }
    let _ = fs::remove_file(&paths.pid);
    let log: File = File::create(&paths.log)?;
    Process::new(std::env::current_exe()?)
        .arg("record")
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log)
        .process_group(0)
        .spawn()
        .context("failed to start recorder")?;
    for _ in 0..20 {
        if paths.recording_pid().is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(50));
    }
    bail!("STT did not start; inspect {}", paths.log.display())
}

fn demo() {
    let overlay: Overlay = Overlay::spawn();
    thread::sleep(Duration::from_secs(1));
    overlay.send(Event::Phase(Phase::Listening));
    for i in 0..60 {
        let t: f32 = i as f32 * 0.1;
        overlay.send(Event::Level(((t * 2.3).sin() * (t * 0.7).cos()).abs()));
        thread::sleep(Duration::from_millis(100));
    }
    overlay.send(Event::Phase(Phase::Finishing));
    thread::sleep(Duration::from_secs(1));
    overlay.close();
}

/// Removes the PID file when the session ends, however it ends.
struct PidFile(PathBuf);

impl PidFile {
    fn create(path: PathBuf) -> Result<Self> {
        let mut file: File = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        write!(file, "{}", std::process::id())?;
        Ok(Self(path))
    }
}

impl Drop for PidFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

async fn record(paths: &Paths) -> Result<()> {
    let _pid: PidFile = PidFile::create(paths.pid.clone())?;
    let overlay: Overlay = Overlay::spawn();
    let result: Result<()> = session(&overlay, &paths.runtime).await;
    overlay.close();
    result
}

async fn session(overlay: &Overlay, runtime: &Path) -> Result<()> {
    // Listen for stop requests before anything slow, so an early one isn't fatal.
    let mut stop: StopSignals = StopSignals::new()?;
    let provider: Provider = Provider::current()?;
    match provider.backend() {
        Backend::Realtime(connect) => {
            let key: String = provider::api_key(provider).await?;
            let connection: Live = connect(&key).await?;
            realtime_session(overlay, &mut stop, provider, connection).await
        }
        Backend::Batch {
            transcribe,
            fallback,
        } => {
            let key: String = provider::api_key(provider).await?;
            let recording: Vec<u8> = record_until_stopped(overlay, &mut stop).await?;
            // The provider that actually transcribed, for the history.
            let mut used: Provider = provider;
            let text: String = match transcribe(&key, &recording).await {
                Ok(text) => text,
                Err(error) if fallback => {
                    used = Provider::ElevenLabsBatch;
                    elevenlabs_fallback(&recording, error).await?
                }
                Err(error) => return Err(error),
            };
            type_transcript(text.clone()).await;
            remember(used, &recording, &text).await;
            Ok(())
        }
        Backend::Local(model) => {
            let server: local::Server = local::Server::new(model, runtime);
            // Lazily, so a first start loads the model while you speak.
            server.ensure_running()?;
            let recording: Vec<u8> = record_until_stopped(overlay, &mut stop).await?;
            let text: String = server.transcribe(&recording).await?;
            type_transcript(text.clone()).await;
            remember(provider, &recording, &text).await;
            Ok(())
        }
    }
}

/// Resolves on the first stop request: `stt toggle`, Ctrl-C or SIGTERM.
struct StopSignals {
    toggle: UnixSignal,
    interrupt: UnixSignal,
    terminate: UnixSignal,
}

impl StopSignals {
    fn new() -> Result<Self> {
        Ok(Self {
            toggle: unix::signal(SignalKind::user_defined2())?,
            interrupt: unix::signal(SignalKind::interrupt())?,
            terminate: unix::signal(SignalKind::terminate())?,
        })
    }

    async fn recv(&mut self) {
        tokio::select! {
            _ = self.toggle.recv() => {}
            _ = self.interrupt.recv() => {}
            _ = self.terminate.recv() => {}
        }
    }
}

/// Records until stopped, for a batch backend to transcribe at once.
async fn record_until_stopped(overlay: &Overlay, stop: &mut StopSignals) -> Result<Vec<u8>> {
    let mut mic: Microphone = Microphone::start()?;
    overlay.send(Event::Phase(Phase::Listening));
    if ENABLE_STATUS_NOTIFICATIONS {
        notify("STT recording", "Speak now; press End again to stop", 2500);
    }

    let mut recording: Vec<u8> = Vec::new();
    loop {
        tokio::select! {
            _ = stop.recv() => break,
            chunk = mic.next_chunk() => match chunk? {
                Some(chunk) => {
                    overlay.send(Event::Level(audio::level(&chunk)));
                    recording.extend_from_slice(&chunk);
                }
                None => bail!("Microphone stopped unexpectedly (pw-record exited)"),
            },
        }
    }

    overlay.send(Event::Phase(Phase::Finishing));
    mic.stop().await;
    Ok(recording)
}

/// Saves the session to the history; a failure is reported but doesn't fail
/// the dictation, which has already been inserted.
async fn remember(provider: Provider, recording: &[u8], transcript: &str) {
    let saved: Result<()> =
        History::open_default().and_then(|history| history.save(provider, recording, transcript));
    if let Err(error) = saved {
        let handle: thread::JoinHandle<()> =
            notify("STT history failed", &format!("{error:#}"), 4000);
        let _ = tokio::task::spawn_blocking(move || handle.join()).await;
    }
}

/// Inserts a batch transcript into the focused window.
async fn type_transcript(text: String) {
    let has_speech: bool = !text.is_empty();
    if PASTE_BATCH_TRANSCRIPTS {
        if has_speech && let Err(error) = paste(&text).await {
            typing_failed(&format!("{error:#}")).await;
        }
    } else {
        let mut typist: Typist = Typist::spawn();
        typist.type_text(text);
        typist.finish().await;
    }
    if ENABLE_STATUS_NOTIFICATIONS {
        notify(
            "STT finished",
            if has_speech {
                "Dictation inserted"
            } else {
                "No speech detected"
            },
            2500,
        )
        .join()
        .ok();
    }
}

/// Copies `text` to the clipboard and presses Shift+Insert, which pastes in
/// terminals and GUI apps alike (Omarchy's universal paste).
async fn paste(text: &str) -> Result<()> {
    let mut copy: tokio::process::Child = tokio::process::Command::new("wl-copy")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .spawn()
        .context("wl-copy")?;
    let mut stdin: tokio::process::ChildStdin = copy.stdin.take().context("wl-copy stdin")?;
    stdin.write_all(text.as_bytes()).await?;
    drop(stdin);
    if !copy.wait().await?.success() {
        bail!("wl-copy failed");
    }
    wtype(&["-M", "shift", "-k", "Insert", "-m", "shift"]).await
}

/// Runs `wtype` with `args`, failing with its stderr.
async fn wtype(args: &[&str]) -> Result<()> {
    let output: Output = tokio::process::Command::new("wtype")
        .args(args)
        .stdout(Stdio::null())
        .output()
        .await
        .context("wtype")?;
    if !output.status.success() {
        let error: std::borrow::Cow<'_, str> = String::from_utf8_lossy(&output.stderr);
        bail!(
            "{}",
            if error.is_empty() {
                "Check the focused window"
            } else {
                &error
            }
        );
    }
    Ok(())
}

/// Transcribes the recording with ElevenLabs after OpenRouter failed with
/// `error`, reporting both failures if this fails too.
async fn elevenlabs_fallback(recording: &[u8], error: anyhow::Error) -> Result<String> {
    eprintln!("{error:#}; falling back to ElevenLabs");
    let fallback: Result<String> = match provider::api_key(Provider::ElevenLabsBatch).await {
        Ok(key) => elevenlabs::transcribe(&key, recording).await,
        Err(key_error) => Err(key_error),
    };
    fallback.map_err(|fallback_error| {
        anyhow!("{error:#}; the ElevenLabs fallback failed too: {fallback_error:#}")
    })
}

/// Streams audio to a realtime backend and types each segment as it is committed.
async fn realtime_session(
    overlay: &Overlay,
    stop: &mut StopSignals,
    provider: Provider,
    connection: Live,
) -> Result<()> {
    let mut tx: Box<dyn TransmitDyn> = connection.sender;
    let mut rx: Box<dyn ReceiveDyn> = connection.receiver;
    let one_transcript_per_commit: bool = rx.one_transcript_per_commit();
    let name: &str = provider.company();
    eprintln!("{name} WebSocket connected");
    let mut mic: Microphone = Microphone::start()?;
    overlay.send(Event::Phase(Phase::Listening));
    if ENABLE_STATUS_NOTIFICATIONS {
        notify("STT recording", "Speak now; press End again to stop", 2500);
    }

    let mut typist: Typist = Typist::spawn();
    let mut typed_any: bool = false;
    let mut recording: Vec<u8> = Vec::new();

    loop {
        tokio::select! {
            _ = stop.recv() => break,
            chunk = mic.next_chunk() => match chunk? {
                Some(chunk) => {
                    overlay.send(Event::Level(audio::level(&chunk)));
                    tx.send_audio(&chunk).await?;
                    recording.extend_from_slice(&chunk);
                }
                None => bail!("Microphone stopped unexpectedly (pw-record exited)"),
            },
            update = rx.next() => match update.transpose()? {
                Some(Update::Committed(text)) => typed_any |= typist.type_text(text),
                Some(Update::Delta(text)) => typed_any |= typist.type_delta(&text),
                Some(Update::Flushed | Update::Other) => {}
                None => bail!("{name} closed the connection"),
            },
        }
    }

    overlay.send(Event::Phase(Phase::Finishing));
    mic.stop().await;
    tx.commit().await?;
    // Wait for the transcript of the final commit, but not forever.
    let finished: Result<()> = time::timeout(FINISH_TIMEOUT, async {
        while let Some(update) = rx.next().await {
            match update? {
                Update::Committed(text) => {
                    typed_any |= typist.type_text(text);
                    if one_transcript_per_commit {
                        return Ok(());
                    }
                }
                Update::Delta(text) => typed_any |= typist.type_delta(&text),
                Update::Flushed => return Ok(()),
                Update::Other => {}
            }
        }
        bail!("{name} closed the connection before the final transcript")
    })
    .await
    .unwrap_or_else(|_| {
        Err(anyhow!(
            "{name} sent no final transcript; the end may be missing"
        ))
    });
    tx.close().await;
    let transcript: String = typist.finish().await;
    // Typed and saved what we could; now report the failure.
    remember(provider, &recording, &transcript).await;
    finished?;
    if ENABLE_STATUS_NOTIFICATIONS {
        notify(
            "STT finished",
            if typed_any {
                "Dictation inserted"
            } else {
                "No speech detected"
            },
            2500,
        )
        .join()
        .ok();
    }
    Ok(())
}

/// Types transcript segments into the focused window one at a time, in order.
struct Typist {
    queue: mpsc::UnboundedSender<String>,
    worker: tokio::task::JoinHandle<()>,
    /// Everything queued so far, as typed.
    transcript: String,
}

impl Typist {
    fn spawn() -> Self {
        let channel: (
            mpsc::UnboundedSender<String>,
            mpsc::UnboundedReceiver<String>,
        ) = mpsc::unbounded_channel();
        let mut segments: mpsc::UnboundedReceiver<String> = channel.1;
        let worker: tokio::task::JoinHandle<()> = tokio::spawn(async move {
            while let Some(segment) = segments.recv().await {
                if let Err(error) = wtype(&[&segment]).await {
                    typing_failed(&format!("{error:#}")).await;
                }
            }
        });
        Self {
            queue: channel.0,
            worker,
            transcript: String::new(),
        }
    }

    /// Queues a segment, spaced from the previous one; returns whether there
    /// was anything to type.
    fn type_text(&mut self, text: String) -> bool {
        if text.is_empty() {
            return false;
        }
        self.push(if self.started() {
            format!(" {text}")
        } else {
            text
        })
    }

    /// Queues a delta that carries its own spacing; returns whether there was
    /// anything to type.
    fn type_delta(&mut self, text: &str) -> bool {
        let text: &str = if self.started() {
            text
        } else {
            text.trim_start()
        };
        !text.is_empty() && self.push(text.to_owned())
    }

    /// Whether anything has been queued yet: the first text gets no leading space.
    fn started(&self) -> bool {
        !self.transcript.is_empty()
    }

    fn push(&mut self, text: String) -> bool {
        self.transcript.push_str(&text);
        let _ = self.queue.send(text);
        true
    }

    /// Waits for everything to be typed; returns the whole transcript.
    async fn finish(self) -> String {
        drop(self.queue);
        let _ = self.worker.await;
        self.transcript
    }
}

/// Reports a typing failure and waits for the notification to go out, so it
/// isn't lost if the session ends right after.
async fn typing_failed(error: &str) {
    let handle: thread::JoinHandle<()> = notify("STT typing failed", error, 4000);
    let _ = tokio::task::spawn_blocking(move || handle.join()).await;
}

/// Shows a desktop notification from a plain OS thread: notify-rust blocks on
/// D-Bus, which panics inside the Tokio runtime. Join the handle when the
/// notification must go out before the process exits.
fn notify(summary: &str, body: &str, timeout_ms: u32) -> thread::JoinHandle<()> {
    eprintln!("notify: {summary}: {body}");
    let mut notification: notify_rust::Notification = notify_rust::Notification::new();
    notification
        .appname("stt")
        .id(NOTIFICATION_ID)
        .summary(summary)
        .body(body)
        .timeout(notify_rust::Timeout::Milliseconds(timeout_ms));
    thread::spawn(move || {
        if let Err(error) = notification.show() {
            eprintln!("notification failed: {error}");
        }
    })
}

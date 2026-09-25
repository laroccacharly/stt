//! Live ElevenLabs dictation for Hyprland.

mod audio;
mod elevenlabs;
mod hyprland;
mod overlay;

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Command as Process, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rustix::process::{Pid, Signal, kill_process};
use tokio::{
    signal::unix::{SignalKind, signal},
    sync::mpsc,
    time::timeout,
};

use audio::Microphone;
use elevenlabs::Update;
use overlay::{Event, Overlay, Phase};

/// How long to wait for the final transcript after stopping.
const FINISH_TIMEOUT: Duration = Duration::from_millis(4500);
/// Notifications share one id so each replaces the previous.
const NOTIFICATION_ID: u32 = 47511;
/// Start/stop notifications are off for now; errors always notify.
const ENABLE_STATUS_NOTIFICATIONS: bool = false;

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
    /// Bind End to `stt toggle` in Hyprland.
    Install,
    /// Remove the End binding.
    Uninstall,
    /// Show the overlay with synthetic audio, for testing.
    #[command(hide = true)]
    Demo,
}

struct Paths {
    runtime: PathBuf,
    pid: PathBuf,
    log: PathBuf,
}

impl Paths {
    fn new() -> Self {
        let base = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(format!("/run/user/{}", rustix::process::getuid().as_raw()))
            });
        let runtime = base.join("stt");
        Self {
            pid: runtime.join("recording.pid"),
            log: runtime.join("stt.log"),
            runtime,
        }
    }

    /// PID of the running recorder, ignoring stale files.
    fn recording_pid(&self) -> Option<Pid> {
        let pid: i32 = fs::read_to_string(&self.pid).ok()?.trim().parse().ok()?;
        let cmdline = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let mut args = cmdline.split(|&b| b == 0);
        let is_stt = args.next().is_some_and(|exe| exe.ends_with(b"stt"));
        (is_stt && args.any(|arg| arg == b"record"))
            .then(|| Pid::from_raw(pid))
            .flatten()
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(error) = run(cli.command.unwrap_or(Cmd::Toggle)) {
        eprintln!("{error:#}");
        let _ = notify("STT error", &format!("{error:#}"), 5000).join();
        std::process::exit(1);
    }
}

fn run(command: Cmd) -> Result<()> {
    let paths = Paths::new();
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
        Cmd::Install => hyprland::install(),
        Cmd::Uninstall => hyprland::uninstall(),
        Cmd::Demo => {
            demo();
            Ok(())
        }
    }
}

fn toggle(paths: &Paths) -> Result<()> {
    if let Some(pid) = paths.recording_pid() {
        kill_process(pid, Signal::USR2)?;
        return Ok(());
    }
    let _ = fs::remove_file(&paths.pid);
    let log = File::create(&paths.log)?;
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
    let overlay = Overlay::spawn();
    thread::sleep(Duration::from_secs(1));
    overlay.send(Event::Phase(Phase::Listening));
    for i in 0..60 {
        let t = i as f32 * 0.1;
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
        let mut file = OpenOptions::new()
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
    let _pid = PidFile::create(paths.pid.clone())?;
    let overlay = Overlay::spawn();
    let result = session(&overlay).await;
    overlay.close();
    result
}

async fn session(overlay: &Overlay) -> Result<()> {
    let mut stop = signal(SignalKind::user_defined2())?;
    let mut interrupt = signal(SignalKind::interrupt())?;
    let mut terminate = signal(SignalKind::terminate())?;

    let (mut tx, mut rx) = elevenlabs::connect(&elevenlabs::api_key().await?).await?;
    eprintln!("ElevenLabs WebSocket connected");
    let mut mic = Microphone::start()?;
    overlay.send(Event::Phase(Phase::Listening));
    if ENABLE_STATUS_NOTIFICATIONS {
        notify("STT recording", "Speak now; press End again to stop", 2500);
    }

    let typist = Typist::spawn();
    let mut typed_any = false;
    // Hold one chunk back so the last one can carry the commit flag.
    let mut pending: Option<Vec<u8>> = None;

    loop {
        tokio::select! {
            _ = stop.recv() => break,
            _ = interrupt.recv() => break,
            _ = terminate.recv() => break,
            chunk = mic.next_chunk() => match chunk? {
                Some(chunk) => {
                    overlay.send(Event::Level(audio::level(&chunk)));
                    if let Some(previous) = pending.replace(chunk) {
                        tx.send_audio(&previous, false).await?;
                    }
                }
                None => break,
            },
            update = rx.next() => match update.transpose()? {
                Some(Update::Committed(text)) => typed_any |= typist.type_text(text),
                Some(Update::Other) => {}
                None => bail!("ElevenLabs closed the connection"),
            },
        }
    }

    overlay.send(Event::Phase(Phase::Finishing));
    mic.stop().await;
    tx.send_audio(pending.as_deref().unwrap_or_default(), true)
        .await?;
    // Wait for the transcript of the final commit, but not forever.
    let _ = timeout(FINISH_TIMEOUT, async {
        while let Some(update) = rx.next().await {
            match update {
                Ok(Update::Committed(text)) => {
                    typed_any |= typist.type_text(text);
                    break;
                }
                Ok(Update::Other) => {}
                Err(error) => {
                    eprintln!("{error:#}");
                    break;
                }
            }
        }
    })
    .await;
    tx.close().await;
    typist.finish().await;
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
}

impl Typist {
    fn spawn() -> Self {
        let (queue, mut segments) = mpsc::unbounded_channel::<String>();
        let worker = tokio::spawn(async move {
            let mut first = true;
            while let Some(text) = segments.recv().await {
                let segment = if first { text } else { format!(" {text}") };
                first = false;
                match tokio::process::Command::new("wtype")
                    .arg(&segment)
                    .stdout(Stdio::null())
                    .output()
                    .await
                {
                    Ok(output) if output.status.success() => {}
                    Ok(output) => {
                        let error = String::from_utf8_lossy(&output.stderr);
                        eprintln!("wtype failed: {error}");
                        notify(
                            "STT typing failed",
                            if error.is_empty() {
                                "Check the focused window"
                            } else {
                                &error
                            },
                            4000,
                        );
                    }
                    Err(error) => eprintln!("wtype failed: {error}"),
                }
            }
        });
        Self { queue, worker }
    }

    /// Queues `text`; returns whether there was anything to type.
    fn type_text(&self, text: String) -> bool {
        let has_text = !text.is_empty();
        if has_text {
            let _ = self.queue.send(text);
        }
        has_text
    }

    async fn finish(self) {
        drop(self.queue);
        let _ = self.worker.await;
    }
}

/// Shows a desktop notification from a plain OS thread: notify-rust blocks on
/// D-Bus, which panics inside the Tokio runtime. Join the handle when the
/// notification must go out before the process exits.
fn notify(summary: &str, body: &str, timeout_ms: u32) -> thread::JoinHandle<()> {
    let mut notification = notify_rust::Notification::new();
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

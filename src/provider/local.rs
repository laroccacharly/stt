//! Local models (Phonon-2, Parakeet Redux, Bark 3.0 Clean, onnxbark), each
//! served by a long-lived Python process run through `uv`. The server is
//! started on first use and stays up, so the model loads once; it loads while
//! the first recording is made.

use std::{
    fs::{self, File},
    os::unix::process::CommandExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

/// The shared server loop, imported by each model's script.
const SERVE: &str = include_str!("local/serve.py");
/// How long a server may take to come up: the very first start installs its
/// dependencies and downloads the model.
const START_TIMEOUT: Duration = Duration::from_secs(600);

/// A local model: its name (also the socket, PID and log file) and its server
/// script. Both are declared in the provider registry.
pub struct Model {
    pub name: &'static str,
    pub script: &'static str,
}

/// A model's server, addressed through files in stt's runtime directory.
pub struct Server {
    model: &'static Model,
    socket: PathBuf,
    pid: PathBuf,
    log: PathBuf,
}

#[derive(Deserialize)]
struct Reply {
    text: Option<String>,
    error: Option<String>,
}

impl Server {
    pub fn new(model: &'static Model, runtime: &Path) -> Self {
        let name: &str = model.name;
        Self {
            model,
            socket: runtime.join(format!("{name}.sock")),
            pid: runtime.join(format!("{name}.pid")),
            log: runtime.join(format!("{name}.log")),
        }
    }

    /// Starts the server unless it is serving or still loading.
    pub fn ensure_running(&self) -> Result<()> {
        if self.alive() {
            return Ok(());
        }
        let log: File = File::create(&self.log)?;
        let child: std::process::Child = uv(self.model)?
            .arg(&self.socket)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            // Its own process group, so it outlives this dictation session.
            .process_group(0)
            .spawn()
            .context("failed to start uv; is it installed?")?;
        fs::write(&self.pid, child.id().to_string())?;
        eprintln!("started the {} server", self.model.name);
        Ok(())
    }

    /// Transcribes 16-bit mono PCM at `SAMPLE_RATE`, waiting for the server to
    /// come up if it is still loading.
    pub async fn transcribe(&self, pcm: &[u8]) -> Result<String> {
        let deadline: Instant = Instant::now() + START_TIMEOUT;
        let mut stream: UnixStream = loop {
            match UnixStream::connect(&self.socket).await {
                Ok(stream) => break stream,
                Err(_) if !self.alive() => {
                    bail!("The {} server exited: {}", self.model.name, self.log_tail())
                }
                Err(_) if Instant::now() > deadline => {
                    bail!("The {} server did not start in time", self.model.name)
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(50)).await,
            }
        };
        stream.write_all(pcm).await?;
        stream.shutdown().await?;
        let mut reply: String = String::new();
        stream.read_to_string(&mut reply).await?;
        parse(&reply)
    }

    /// Whether the server process recorded in the PID file is running.
    fn alive(&self) -> bool {
        fs::read_to_string(&self.pid)
            .ok()
            .and_then(|pid| fs::read(format!("/proc/{}/cmdline", pid.trim())).ok())
            // A zombie's command line is empty.
            .is_some_and(|cmdline| !cmdline.is_empty())
    }

    /// The last line of the server's log, for error messages.
    fn log_tail(&self) -> String {
        let log: String = fs::read_to_string(&self.log).unwrap_or_default();
        let last: &str = log
            .lines()
            .rev()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("");
        format!("{last} (log: {})", self.log.display())
    }
}

/// Installs the model's dependencies and downloads it, showing progress.
pub fn prepare(model: &'static Model) -> Result<()> {
    let status: std::process::ExitStatus = uv(model)?
        .arg("--prepare")
        .status()
        .context("failed to start uv; is it installed?")?;
    if !status.success() {
        bail!("Downloading the {} model failed", model.name);
    }
    Ok(())
}

/// `uv run` on the model's script, written out to stt's cache directory: uv
/// keeps the script's environment keyed on its path.
fn uv(model: &'static Model) -> Result<Command> {
    let base: PathBuf = match std::env::var_os("XDG_CACHE_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".cache"),
    };
    let dir: PathBuf = base.join("stt");
    fs::create_dir_all(&dir)?;
    write_if_changed(&dir.join("serve.py"), SERVE)?;
    let script: PathBuf = dir.join(format!("{}.py", model.name));
    write_if_changed(&script, model.script)?;
    let mut command: Command = Command::new("uv");
    command.args(["run", "--quiet", "--script"]).arg(script);
    Ok(command)
}

fn write_if_changed(path: &Path, contents: &str) -> Result<()> {
    if fs::read_to_string(path).ok().as_deref() != Some(contents) {
        fs::write(path, contents)?;
    }
    Ok(())
}

fn parse(reply: &str) -> Result<String> {
    let reply: Reply = serde_json::from_str(reply.trim())
        .with_context(|| format!("The local model sent an unexpected reply: {reply:?}"))?;
    match (reply.text, reply.error) {
        (_, Some(error)) => bail!("Local model: {error}"),
        (Some(text), None) => Ok(text.trim().to_owned()),
        (None, None) => bail!("The local model sent no transcript"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transcript() {
        assert_eq!(parse("{\"text\": \" hi \"}\n").unwrap(), "hi");
    }

    #[test]
    fn surfaces_errors() {
        let error: anyhow::Error = parse(r#"{"error": "ValueError: bad audio"}"#).unwrap_err();
        assert_eq!(error.to_string(), "Local model: ValueError: bad audio");
    }
}

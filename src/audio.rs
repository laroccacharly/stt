//! Microphone capture through PipeWire's `pw-record`.

use std::process::Stdio;

use anyhow::{Context, Result};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStdout, Command},
};

pub const SAMPLE_RATE: u32 = 16_000;
/// 100 ms of 16-bit mono audio.
const CHUNK_BYTES: usize = SAMPLE_RATE as usize / 10 * 2;

pub struct Microphone {
    child: Child,
    stdout: ChildStdout,
}

impl Microphone {
    pub fn start() -> Result<Self> {
        let mut child = Command::new("pw-record")
            .args(["--raw", "--format", "s16", "--channels", "1", "--rate"])
            .arg(SAMPLE_RATE.to_string())
            .arg("-")
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start pw-record")?;
        let stdout = child.stdout.take().context("pw-record has no stdout")?;
        Ok(Self { child, stdout })
    }

    /// Next chunk of little-endian `i16` samples, or `None` when capture ends.
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        let mut chunk = vec![0; CHUNK_BYTES];
        let mut filled = 0;
        while filled < CHUNK_BYTES {
            match self.stdout.read(&mut chunk[filled..]).await? {
                0 => break,
                n => filled += n,
            }
        }
        // Keep whole samples only.
        chunk.truncate(filled & !1);
        Ok((!chunk.is_empty()).then_some(chunk))
    }

    pub async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

/// Perceptual loudness of a chunk, mapped to `0.0..=1.0` for display.
pub fn level(chunk: &[u8]) -> f32 {
    let samples = chunk
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| f32::from(i16::from_le_bytes([b[0], b[1]])) / 32768.0);
    let (sum, count) = samples.fold((0.0, 0usize), |(sum, n), s| (sum + s * s, n + 1));
    let rms = (sum / count.max(1) as f32).sqrt();
    (rms.sqrt() * 2.2).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn silence_is_zero() {
        assert_eq!(level(&[0; 320]), 0.0);
    }

    #[test]
    fn full_scale_is_clamped() {
        let loud: Vec<u8> = std::iter::repeat_n(i16::MAX.to_le_bytes(), 160)
            .flatten()
            .collect();
        assert_eq!(level(&loud), 1.0);
    }
}

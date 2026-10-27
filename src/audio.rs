//! Microphone capture through PipeWire's `pw-record`.

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use tokio::io::AsyncReadExt;
use tokio::process::{Child, ChildStdout, Command};

pub const SAMPLE_RATE: u32 = 16_000;
/// 100 ms of 16-bit mono audio.
const CHUNK_BYTES: usize = SAMPLE_RATE as usize / 10 * 2;
/// `split_at_pauses` looks for a pause this far back from each cut.
const PAUSE_SEARCH: Duration = Duration::from_secs(5);
/// 20 ms, the steps in which `split_at_pauses` measures loudness.
const FRAME_BYTES: usize = SAMPLE_RATE as usize / 50 * 2;

pub struct Microphone {
    child: Child,
    stdout: ChildStdout,
}

impl Microphone {
    pub fn start() -> Result<Self> {
        let mut child: Child = Command::new("pw-record")
            .args(["--raw", "--format", "s16", "--channels", "1", "--rate"])
            .arg(SAMPLE_RATE.to_string())
            .arg("-")
            .stdout(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .context("failed to start pw-record")?;
        let stdout: ChildStdout = child.stdout.take().context("pw-record has no stdout")?;
        Ok(Self { child, stdout })
    }

    /// Next chunk of little-endian `i16` samples, or `None` when capture ends.
    pub async fn next_chunk(&mut self) -> Result<Option<Vec<u8>>> {
        let mut chunk: Vec<u8> = vec![0; CHUNK_BYTES];
        let mut filled: usize = 0;
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

/// Decodes any audio or video file ffmpeg reads into 16-bit mono PCM at
/// `SAMPLE_RATE`, the format the microphone records.
pub async fn decode(file: &Path) -> Result<Vec<u8>> {
    let output: Output = Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "error", "-i"])
        .arg(file)
        .args(["-vn", "-f", "s16le", "-ac", "1", "-ar"])
        .arg(SAMPLE_RATE.to_string())
        .arg("-")
        .stdin(Stdio::null())
        .output()
        .await
        .context("failed to start ffmpeg; is it installed?")?;
    if !output.status.success() {
        bail!(
            "ffmpeg could not decode {}: {}",
            file.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

/// Splits PCM into pieces of at most `max`, each ending at the quietest
/// 20 ms of its last `PAUSE_SEARCH`, so cuts fall between words.
pub fn split_at_pauses(pcm: &[u8], max: Duration) -> Vec<&[u8]> {
    let max_bytes: usize = bytes_in(max).max(FRAME_BYTES);
    let search_bytes: usize = bytes_in(PAUSE_SEARCH).min(max_bytes / 2);
    let mut pieces: Vec<&[u8]> = Vec::new();
    let mut rest: &[u8] = pcm;
    while rest.len() > max_bytes {
        let window: &[u8] = &rest[max_bytes - search_bytes..max_bytes];
        let quietest: usize = window
            .as_chunks::<FRAME_BYTES>()
            .0
            .iter()
            .enumerate()
            .min_by_key(|(_, frame)| energy(*frame))
            .map_or(0, |(i, _)| i);
        let cut: usize = max_bytes - search_bytes + quietest * FRAME_BYTES + FRAME_BYTES / 2;
        pieces.push(&rest[..cut & !1]);
        rest = &rest[cut & !1..];
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

/// Sum of squared samples: unlike `level`, it doesn't clip loud audio.
fn energy(frame: &[u8]) -> u64 {
    frame
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i64::from(i16::from_le_bytes(*b)).unsigned_abs().pow(2))
        .sum()
}

/// The size of `duration` of 16-bit mono PCM at `SAMPLE_RATE`.
fn bytes_in(duration: Duration) -> usize {
    (duration.as_millis() as usize * SAMPLE_RATE as usize / 1000) * 2
}

/// Perceptual loudness of a chunk, mapped to `0.0..=1.0` for display.
pub fn level(chunk: &[u8]) -> f32 {
    let samples: &[[u8; 2]] = chunk.as_chunks::<2>().0;
    let sum: f32 = samples
        .iter()
        .map(|b| f32::from(i16::from_le_bytes(*b)) / 32768.0)
        .map(|s| s * s)
        .sum();
    let rms: f32 = (sum / samples.len().max(1) as f32).sqrt();

    (rms.sqrt() * 2.2).min(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_in_the_pause_before_the_limit() {
        // 30 s of tone with 100 ms of silence at 27 s.
        let tone: [u8; 2] = 8000_i16.to_le_bytes();
        let mut pcm: Vec<u8> = std::iter::repeat_n(tone, SAMPLE_RATE as usize * 30)
            .flatten()
            .collect();
        let pause: usize = bytes_in(Duration::from_secs(27));
        pcm[pause..pause + CHUNK_BYTES].fill(0);
        let pieces: Vec<&[u8]> = split_at_pauses(&pcm, Duration::from_secs(28));
        assert_eq!(pieces.len(), 2, "should cut once");
        assert!(
            (pause..pause + CHUNK_BYTES).contains(&pieces[0].len()),
            "should cut inside the pause, not at {}",
            pieces[0].len()
        );
        assert_eq!(pieces.concat(), pcm, "should keep every sample");
    }

    #[test]
    fn splits_at_the_quietest_of_loud_audio() {
        // Loud enough that `level` reads 1.0 throughout, with a quieter stretch at 27 s.
        let loud: [u8; 2] = 16000_i16.to_le_bytes();
        let mut pcm: Vec<u8> = std::iter::repeat_n(loud, SAMPLE_RATE as usize * 30)
            .flatten()
            .collect();
        let quieter: usize = bytes_in(Duration::from_secs(27));
        for sample in pcm[quieter..quieter + CHUNK_BYTES].as_chunks_mut::<2>().0 {
            *sample = 8000_i16.to_le_bytes();
        }
        let pieces: Vec<&[u8]> = split_at_pauses(&pcm, Duration::from_secs(28));
        assert!(
            (quieter..quieter + CHUNK_BYTES).contains(&pieces[0].len()),
            "should cut in the quieter stretch, not at {}",
            pieces[0].len()
        );
    }

    #[test]
    fn short_audio_is_one_piece() {
        assert_eq!(
            split_at_pauses(&[0; 640], Duration::from_secs(30)).len(),
            1,
            "one piece"
        );
    }

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

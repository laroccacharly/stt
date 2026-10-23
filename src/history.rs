//! Dictation history: every recording, FLAC-compressed, with its transcript,
//! the provider and model that produced it, and when.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use flacenc::bitsink::ByteSink;
use flacenc::component::BitRepr;
use flacenc::error::Verify;
use flacenc::source::MemSource;
use rusqlite::Connection;

use crate::audio::SAMPLE_RATE;
use crate::provider::Provider;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS transcriptions (
    id INTEGER PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    audio BLOB NOT NULL,
    transcript TEXT NOT NULL
)";

pub struct History(Connection);

/// A saved session, without its audio.
#[derive(Debug, PartialEq, Eq)]
pub struct Entry {
    /// Local time, `YYYY-MM-DD HH:MM:SS`.
    pub time: String,
    pub provider: String,
    pub model: String,
    pub transcript: String,
}

impl History {
    /// `$XDG_DATA_HOME/stt/history.sqlite`.
    pub fn open_default() -> Result<Self> {
        let base: PathBuf = match std::env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
            Some(dir) => PathBuf::from(dir),
            None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?)
                .join(".local/share"),
        };
        Self::open(&base.join("stt/history.sqlite"))
    }

    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let connection: Connection =
            Connection::open(path).with_context(|| format!("opening {}", path.display()))?;
        connection.execute_batch(SCHEMA)?;
        Ok(Self(connection))
    }

    /// Saves a recording of 16-bit mono PCM at `SAMPLE_RATE` and its transcript.
    pub fn save(&self, provider: Provider, pcm: &[u8], transcript: &str) -> Result<()> {
        self.0.execute(
            "INSERT INTO transcriptions (provider, model, audio, transcript) VALUES (?1, ?2, ?3, ?4)",
            (provider.name(), provider.model(), flac(pcm)?, transcript),
        )?;
        Ok(())
    }

    /// The `limit` most recent sessions, newest first.
    pub fn recent(&self, limit: usize) -> Result<Vec<Entry>> {
        let mut statement: rusqlite::Statement<'_> = self.0.prepare(
            "SELECT strftime('%Y-%m-%d %H:%M:%S', created_at, 'localtime'), provider, model, transcript
             FROM transcriptions ORDER BY id DESC LIMIT ?1",
        )?;
        let entries: Vec<Entry> = statement
            .query_map([limit as i64], |row| {
                Ok(Entry {
                    time: row.get(0)?,
                    provider: row.get(1)?,
                    model: row.get(2)?,
                    transcript: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(entries)
    }
}

/// Losslessly compresses 16-bit mono PCM at `SAMPLE_RATE` to FLAC.
fn flac(pcm: &[u8]) -> Result<Vec<u8>> {
    let samples: Vec<i32> = pcm
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i32::from(i16::from_le_bytes(*b)))
        .collect();
    let config: flacenc::config::Encoder = flacenc::config::Encoder::default();
    let config: flacenc::error::Verified<flacenc::config::Encoder> = config
        .into_verified()
        .map_err(|error| anyhow!("FLAC config: {error:?}"))?;
    let source: MemSource = MemSource::from_samples(&samples, 1, 16, SAMPLE_RATE as usize);
    let stream: flacenc::component::Stream =
        flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
            .map_err(|error| anyhow!("FLAC encoding: {error:?}"))?;
    let mut sink: ByteSink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|error| anyhow!("FLAC encoding: {error:?}"))?;
    Ok(sink.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_compressed_audio_and_transcript() {
        let dir: tempfile::TempDir = tempfile::tempdir().unwrap();
        let path: PathBuf = dir.path().join("nested/history.sqlite");
        // One second of a quiet sine, compressible like speech.
        let samples: Vec<i16> = (0..SAMPLE_RATE)
            .map(|i| ((i as f32 * 0.05).sin() * 3000.0) as i16)
            .collect();
        let pcm: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();

        let history: History = History::open(&path).unwrap();
        history
            .save(Provider::Cartesia, &pcm, "hello world")
            .unwrap();
        history.save(Provider::OpenRouter, &[], "").unwrap();
        drop(history);

        let db: Connection = Connection::open(&path).unwrap();
        let rows: Vec<(String, String, Vec<u8>, String, String)> = db
            .prepare("SELECT provider, model, audio, transcript, created_at FROM transcriptions ORDER BY id")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?))
            })
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(rows.len(), 2);

        let first: &(String, String, Vec<u8>, String, String) = &rows[0];
        assert_eq!(first.0, "cartesia");
        assert_eq!(first.1, "ink-2");
        assert_eq!(first.3, "hello world");
        assert!(
            first.4.starts_with("20") && first.4.ends_with('Z'),
            "{}",
            first.4
        );
        assert!(first.2.starts_with(b"fLaC"));
        assert!(first.2.len() < pcm.len() / 2, "{} bytes", first.2.len());
        let mut reader: claxon::FlacReader<&[u8]> =
            claxon::FlacReader::new(first.2.as_slice()).unwrap();
        assert_eq!(reader.streaminfo().sample_rate, SAMPLE_RATE);
        let decoded: Vec<i16> = reader.samples().map(|s| s.unwrap() as i16).collect();
        assert_eq!(decoded, samples);

        let second: &(String, String, Vec<u8>, String, String) = &rows[1];
        assert_eq!(second.0, "openrouter");
        assert_eq!(second.1, "microsoft/mai-transcribe-2");
        assert_eq!(second.3, "");
    }

    #[test]
    fn recent_lists_newest_first_up_to_the_limit() {
        let dir: tempfile::TempDir = tempfile::tempdir().unwrap();
        let history: History = History::open(&dir.path().join("history.sqlite")).unwrap();
        for text in ["one", "two", "three"] {
            history.save(Provider::Cartesia, &[0; 320], text).unwrap();
        }
        let recent: Vec<Entry> = history.recent(2).unwrap();
        let texts: Vec<&str> = recent
            .iter()
            .map(|entry| entry.transcript.as_str())
            .collect();
        assert_eq!(texts, ["three", "two"]);
        assert_eq!(recent[0].provider, "cartesia");
        assert_eq!(recent[0].model, "ink-2");
        assert_eq!(recent[0].time.len(), "2026-01-01 00:00:00".len());
    }
}

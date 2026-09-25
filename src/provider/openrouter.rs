//! OpenRouter batch transcription (MAI-Transcribe): the whole recording is
//! uploaded once dictation stops.

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Deserialize;
use serde_json::json;

use crate::audio::SAMPLE_RATE;

const MODEL: &str = "microsoft/mai-transcribe-2";

/// Reads `OPENROUTER_API_KEY` from the environment or the keyring.
pub async fn api_key() -> Result<String> {
    super::api_key("OPENROUTER_API_KEY", "OpenRouter").await
}

#[derive(Deserialize)]
struct Transcription {
    text: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: ErrorDetail,
}

#[derive(Deserialize)]
struct ErrorDetail {
    message: String,
}

/// Transcribes 16-bit mono PCM at `SAMPLE_RATE`; the language is auto-detected.
pub async fn transcribe(api_key: &str, pcm: &[u8]) -> Result<String> {
    super::install_crypto();
    // Overridable so end-to-end tests can point at a local server.
    let base =
        std::env::var("STT_OPENROUTER_URL").unwrap_or_else(|_| "https://openrouter.ai".into());
    let body = json!({
        "model": MODEL,
        "input_audio": { "data": STANDARD.encode(wav(pcm)), "format": "wav" },
        "provider": { "options": { "azure": {
            // Drop fillers and false starts: this is dictation, not a transcript.
            "enhancedMode": { "modelOptions": { "transcribeStyle": "clean" } }
        } } },
    });
    let response = reqwest::Client::new()
        .post(format!("{base}/api/v1/audio/transcriptions"))
        .bearer_auth(api_key)
        .header("X-OpenRouter-Title", "stt")
        .json(&body)
        .send()
        .await
        .context("OpenRouter request failed")?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let message = serde_json::from_str::<ErrorBody>(&text)
            .map(|body| body.error.message)
            .unwrap_or(text);
        bail!("OpenRouter: {status}: {message}");
    }
    let transcription: Transcription =
        serde_json::from_str(&text).context("OpenRouter sent an unexpected response")?;
    Ok(transcription.text.trim().to_owned())
}

/// Wraps raw PCM in a WAV header.
fn wav(pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let byte_rate = SAMPLE_RATE * 2;
    let mut out = Vec::with_capacity(44 + pcm.len());
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes()); // block align
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    out.extend_from_slice(pcm);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_describes_pcm() {
        let wav = wav(&[1, 2, 3, 4]);
        assert_eq!(wav.len(), 48);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(wav[4..8].try_into().unwrap()), 40);
        assert_eq!(u32::from_le_bytes(wav[24..28].try_into().unwrap()), 16_000);
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 4);
        assert_eq!(&wav[44..], &[1, 2, 3, 4]);
    }
}

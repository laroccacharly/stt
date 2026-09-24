//! ElevenLabs Scribe realtime speech-to-text over WebSocket.

use std::collections::HashMap;

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use secret_service::{EncryptionType, SecretService};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use crate::audio::SAMPLE_RATE;

const MODEL: &str = "scribe_v2_realtime";

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Reads `ELEVENLABS_API_KEY`, falling back to the `cterm` entry in the
/// Secret Service keyring (the one Bun.secrets writes).
pub async fn api_key() -> Result<String> {
    if let Ok(key) = std::env::var("ELEVENLABS_API_KEY")
        && !key.is_empty()
    {
        return Ok(key);
    }
    let service = SecretService::connect(EncryptionType::Dh).await?;
    let attributes = HashMap::from([("service", "cterm"), ("account", "ELEVENLABS_API_KEY")]);
    let found = service.search_items(attributes).await?;
    let item = match (found.unlocked.first(), found.locked.first()) {
        (Some(item), _) => item,
        (None, Some(item)) => {
            item.unlock().await?;
            item
        }
        (None, None) => bail!("No ElevenLabs key: set ELEVENLABS_API_KEY or run cterm login"),
    };
    Ok(String::from_utf8(item.get_secret().await?)?)
}

pub struct Sender(SplitSink<Socket, Message>);
pub struct Receiver(SplitStream<Socket>);

pub async fn connect(api_key: &str) -> Result<(Sender, Receiver)> {
    // Pin ring explicitly so a dependency enabling aws-lc-rs can't make rustls panic.
    let _ = rustls::crypto::ring::default_provider().install_default();
    // Overridable so end-to-end tests can point at a local server.
    let base =
        std::env::var("STT_ELEVENLABS_URL").unwrap_or_else(|_| "wss://api.elevenlabs.io".into());
    let url = format!(
        "{base}/v1/speech-to-text/realtime?model_id={MODEL}&audio_format=pcm_{SAMPLE_RATE}&commit_strategy=vad"
    );
    let mut request = url.into_client_request()?;
    request.headers_mut().insert("xi-api-key", api_key.parse()?);
    let (socket, _) = connect_async(request)
        .await
        .context("ElevenLabs connection failed")?;
    let (sink, stream) = socket.split();
    Ok((Sender(sink), Receiver(stream)))
}

#[derive(Serialize)]
struct AudioChunk {
    message_type: &'static str,
    audio_base_64: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    commit: bool,
}

impl Sender {
    /// Streams audio; `commit` asks the server to finalise everything so far.
    pub async fn send_audio(&mut self, pcm: &[u8], commit: bool) -> Result<()> {
        let chunk = AudioChunk {
            message_type: "input_audio_chunk",
            audio_base_64: STANDARD.encode(pcm),
            commit,
        };
        self.0
            .send(Message::text(serde_json::to_string(&chunk)?))
            .await?;
        Ok(())
    }

    pub async fn close(mut self) {
        let _ = self.0.close().await;
    }
}

#[derive(Debug, Deserialize)]
struct ServerMessage {
    message_type: String,
    text: Option<String>,
    error: Option<String>,
}

#[derive(Debug)]
pub enum Update {
    /// A finalised piece of transcript.
    Committed(String),
    /// A message we don't act on (session start, partial transcripts, …).
    Other,
}

impl Receiver {
    /// Next server update, or `None` when the socket closes.
    pub async fn next(&mut self) -> Option<Result<Update>> {
        loop {
            let message = match self.0.next().await? {
                Ok(message) => message,
                Err(error) => return Some(Err(error.into())),
            };
            let Message::Text(text) = message else {
                if message.is_close() {
                    return None;
                }
                continue;
            };
            return Some(parse(&text));
        }
    }
}

fn parse(text: &str) -> Result<Update> {
    let message: ServerMessage = serde_json::from_str(text)?;
    match message.message_type.as_str() {
        "committed_transcript" => Ok(Update::Committed(
            message.text.unwrap_or_default().trim().to_owned(),
        )),
        kind if kind.ends_with("error") || kind == "rate_limited" => {
            bail!("ElevenLabs: {}", message.error.as_deref().unwrap_or(kind))
        }
        "session_started" => {
            eprintln!("ElevenLabs transcription session started");
            Ok(Update::Other)
        }
        _ => Ok(Update::Other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_committed_transcript() {
        let update = parse(r#"{"message_type":"committed_transcript","text":" hello "}"#).unwrap();
        assert!(matches!(update, Update::Committed(text) if text == "hello"));
    }

    #[test]
    fn surfaces_errors() {
        let error = parse(r#"{"message_type":"auth_error","error":"bad key"}"#).unwrap_err();
        assert_eq!(error.to_string(), "ElevenLabs: bad key");
    }

    #[test]
    fn omits_commit_when_false() {
        let json = serde_json::to_string(&AudioChunk {
            message_type: "input_audio_chunk",
            audio_base_64: String::new(),
            commit: false,
        })
        .unwrap();
        assert!(!json.contains("commit"));
    }
}

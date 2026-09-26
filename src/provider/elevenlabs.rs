//! ElevenLabs Scribe realtime speech-to-text over WebSocket.

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use super::{Receive, Transmit, Update};
use crate::audio::SAMPLE_RATE;

const MODEL: &str = "scribe_v2_realtime";

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Reads `ELEVENLABS_API_KEY` from the environment or the keyring.
pub async fn api_key() -> Result<String> {
    super::api_key(super::Provider::ElevenLabs).await
}

pub struct Sender {
    sink: SplitSink<Socket, Message>,
    /// Held back one chunk so the last one can carry the commit flag.
    pending: Option<Vec<u8>>,
}
pub struct Receiver(SplitStream<Socket>);

pub async fn connect(api_key: &str) -> Result<(Sender, Receiver)> {
    super::install_crypto();
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
    Ok((
        Sender {
            sink,
            pending: None,
        },
        Receiver(stream),
    ))
}

#[derive(Serialize)]
struct AudioChunk {
    message_type: &'static str,
    audio_base_64: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    commit: bool,
}

impl Sender {
    async fn send_chunk(&mut self, pcm: &[u8], commit: bool) -> Result<()> {
        let chunk = AudioChunk {
            message_type: "input_audio_chunk",
            audio_base_64: STANDARD.encode(pcm),
            commit,
        };
        self.sink
            .send(Message::text(serde_json::to_string(&chunk)?))
            .await?;
        Ok(())
    }
}

impl Transmit for Sender {
    async fn send_audio(&mut self, pcm: &[u8]) -> Result<()> {
        if let Some(previous) = self.pending.replace(pcm.to_vec()) {
            self.send_chunk(&previous, false).await?;
        }
        Ok(())
    }

    async fn commit(&mut self) -> Result<()> {
        let last = self.pending.take().unwrap_or_default();
        self.send_chunk(&last, true).await
    }

    async fn close(mut self) {
        let _ = self.sink.close().await;
    }
}

#[derive(Debug, Deserialize)]
struct ServerMessage {
    message_type: String,
    text: Option<String>,
    error: Option<String>,
}

impl Receive for Receiver {
    const ONE_TRANSCRIPT_PER_COMMIT: bool = true;

    async fn next(&mut self) -> Option<Result<Update>> {
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

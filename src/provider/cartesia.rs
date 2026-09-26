//! Cartesia Ink realtime speech-to-text over WebSocket. Audio streams as raw
//! PCM; `finalize` flushes the transcript of everything sent so far.

use anyhow::{Context, Result, bail};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use serde::Deserialize;
use tokio::net::TcpStream;
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};

use super::{Receive, Transmit, Update};
use crate::audio::SAMPLE_RATE;

const MODEL: &str = "ink-2";
const API_VERSION: &str = "2026-03-01";

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Reads `CARTESIA_API_KEY` from the environment or the keyring.
pub async fn api_key() -> Result<String> {
    super::api_key(super::Provider::Cartesia).await
}

pub struct Sender(SplitSink<Socket, Message>);
pub struct Receiver(SplitStream<Socket>);

pub async fn connect(api_key: &str) -> Result<(Sender, Receiver)> {
    super::install_crypto();
    // Overridable so end-to-end tests can point at a local server.
    let base = std::env::var("STT_CARTESIA_URL").unwrap_or_else(|_| "wss://api.cartesia.ai".into());
    let url = format!(
        "{base}/stt/websocket?model={MODEL}&encoding=pcm_s16le&sample_rate={SAMPLE_RATE}&cartesia_version={API_VERSION}"
    );
    let mut request = url.into_client_request()?;
    request.headers_mut().insert("X-API-Key", api_key.parse()?);
    let (socket, _) = connect_async(request)
        .await
        .context("Cartesia connection failed")?;
    let (sink, stream) = socket.split();
    Ok((Sender(sink), Receiver(stream)))
}

impl Transmit for Sender {
    async fn send_audio(&mut self, pcm: &[u8]) -> Result<()> {
        self.0.send(Message::binary(pcm.to_vec())).await?;
        Ok(())
    }

    async fn commit(&mut self) -> Result<()> {
        self.0.send(Message::text("finalize")).await?;
        Ok(())
    }

    async fn close(mut self) {
        let _ = self.0.close().await;
    }
}

#[derive(Debug, Deserialize)]
struct ServerMessage {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
    #[serde(default)]
    is_final: bool,
    message: Option<String>,
}

impl Receive for Receiver {
    const ONE_TRANSCRIPT_PER_COMMIT: bool = false;

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
    match message.kind.as_str() {
        // Deltas carry their own spacing (" quick brown", ","): keep it.
        "transcript" if message.is_final => Ok(Update::Delta(message.text.unwrap_or_default())),
        "flush_done" => Ok(Update::Flushed),
        "error" => bail!(
            "Cartesia: {}",
            message.message.as_deref().unwrap_or("unknown error")
        ),
        _ => Ok(Update::Other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_final_transcript() {
        let update = parse(r#"{"type":"transcript","is_final":true,"text":" hello"}"#).unwrap();
        assert!(matches!(update, Update::Delta(text) if text == " hello"));
    }

    #[test]
    fn ignores_interim_transcript() {
        let update = parse(r#"{"type":"transcript","is_final":false,"text":"hel"}"#).unwrap();
        assert!(matches!(update, Update::Other));
    }

    #[test]
    fn flush_done_ends_the_commit() {
        let update = parse(r#"{"type":"flush_done","request_id":"x"}"#).unwrap();
        assert!(matches!(update, Update::Flushed));
    }

    #[test]
    fn surfaces_errors() {
        let error = parse(
            r#"{"type":"error","status_code":401,"title":"Unauthorized","message":"bad key"}"#,
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "Cartesia: bad key");
    }
}

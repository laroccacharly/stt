//! Speech-to-text backends.

pub mod elevenlabs;
pub mod openrouter;

use std::collections::HashMap;

use anyhow::{Result, bail};
use secret_service::{EncryptionType, SecretService};

/// Which backend transcribes, chosen with `STT_PROVIDER`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    /// Realtime: text is typed while you speak.
    ElevenLabs,
    /// Batch: the recording is transcribed once you stop.
    OpenRouter,
}

impl Provider {
    pub fn from_env() -> Result<Self> {
        match std::env::var("STT_PROVIDER").unwrap_or_default().as_str() {
            "" | "elevenlabs" => Ok(Self::ElevenLabs),
            "openrouter" => Ok(Self::OpenRouter),
            other => bail!("Unknown STT_PROVIDER {other:?}; use elevenlabs or openrouter"),
        }
    }
}

/// Pins ring explicitly so a dependency enabling aws-lc-rs can't make rustls panic.
fn install_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Reads the `name` environment variable, falling back to the `cterm` entry in
/// the Secret Service keyring (the one Bun.secrets writes).
async fn api_key(name: &str, label: &str) -> Result<String> {
    if let Ok(key) = std::env::var(name)
        && !key.is_empty()
    {
        return Ok(key);
    }
    let service = SecretService::connect(EncryptionType::Dh).await?;
    let attributes = HashMap::from([("service", "cterm"), ("account", name)]);
    let found = service.search_items(attributes).await?;
    let item = match (found.unlocked.first(), found.locked.first()) {
        (Some(item), _) => item,
        (None, Some(item)) => {
            item.unlock().await?;
            item
        }
        (None, None) => bail!("No {label} key: set {name} or run cterm login"),
    };
    Ok(String::from_utf8(item.get_secret().await?)?)
}

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
    pub const ALL: [Self; 2] = [Self::ElevenLabs, Self::OpenRouter];

    pub fn from_env() -> Result<Self> {
        match std::env::var("STT_PROVIDER").unwrap_or_default().as_str() {
            "" | "elevenlabs" => Ok(Self::ElevenLabs),
            "openrouter" => Ok(Self::OpenRouter),
            other => bail!("Unknown STT_PROVIDER {other:?}; use elevenlabs or openrouter"),
        }
    }

    /// The environment variable and keyring account holding the API key.
    pub fn key_name(self) -> &'static str {
        match self {
            Self::ElevenLabs => "ELEVENLABS_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ElevenLabs => "ElevenLabs",
            Self::OpenRouter => "OpenRouter",
        }
    }
}

/// Keyring service for keys saved with `stt login`.
const SERVICE: &str = "stt";

/// Pins ring explicitly so a dependency enabling aws-lc-rs can't make rustls panic.
fn install_crypto() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

/// Reads the provider's key from its environment variable, else from the
/// keyring entry saved by `stt login`.
async fn api_key(provider: Provider) -> Result<String> {
    let name = provider.key_name();
    if let Ok(key) = std::env::var(name)
        && !key.is_empty()
    {
        return Ok(key);
    }
    let service = SecretService::connect(EncryptionType::Dh).await?;
    let attributes = HashMap::from([("service", SERVICE), ("account", name)]);
    let found = service.search_items(attributes).await?;
    let item = match (found.unlocked.first(), found.locked.first()) {
        (Some(item), _) => item,
        (None, Some(item)) => {
            item.unlock().await?;
            item
        }
        (None, None) => bail!(
            "No {} key: set {name} (then run `stt login` to save it)",
            provider.label()
        ),
    };
    Ok(String::from_utf8(item.get_secret().await?)?)
}

/// Saves `key` in the keyring for `stt` to find, replacing any earlier one.
pub async fn save_api_key(provider: Provider, key: &str) -> Result<()> {
    let name = provider.key_name();
    let service = SecretService::connect(EncryptionType::Dh).await?;
    let collection = service.get_default_collection().await?;
    if collection.is_locked().await? {
        collection.unlock().await?;
    }
    let attributes = HashMap::from([("service", SERVICE), ("account", name)]);
    collection
        .create_item(
            &format!("stt {} API key", provider.label()),
            attributes,
            key.as_bytes(),
            true,
            "text/plain",
        )
        .await?;
    Ok(())
}

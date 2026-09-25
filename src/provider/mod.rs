//! Speech-to-text backends.

pub mod elevenlabs;
pub mod openrouter;

use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use secret_service::{EncryptionType, SecretService};
use serde::{Deserialize, Serialize};

/// Which backend transcribes: the one chosen with `stt provider set`, else
/// ElevenLabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Realtime: text is typed while you speak.
    #[value(name = "elevenlabs")]
    ElevenLabs,
    /// Batch: the recording is transcribed once you stop.
    #[value(name = "openrouter")]
    OpenRouter,
}

impl Provider {
    pub const ALL: [Self; 2] = [Self::ElevenLabs, Self::OpenRouter];

    /// The provider in use now.
    pub fn current() -> Result<Self> {
        let config: Option<Config> =
            json_store::read_opt(&config_path()?).context("reading the stt config")?;
        Ok(config.map_or(Self::ElevenLabs, |config| config.provider))
    }

    /// Makes this the provider used from now on.
    pub fn save(self) -> Result<()> {
        json_store::write(&config_path()?, &Config { provider: self })
            .context("saving the stt config")?;
        Ok(())
    }

    /// The name used on the command line and in the config.
    pub fn name(self) -> &'static str {
        match self {
            Self::ElevenLabs => "elevenlabs",
            Self::OpenRouter => "openrouter",
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

/// Settings saved by `stt provider set`.
#[derive(Serialize, Deserialize)]
struct Config {
    provider: Provider,
}

/// `$XDG_CONFIG_HOME/stt/config.json`.
fn config_path() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(std::env::var_os("HOME").context("HOME is not set")?).join(".config"),
    };
    Ok(base.join("stt/config.json"))
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

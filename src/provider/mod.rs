//! Speech-to-text backends.

pub mod cartesia;
pub mod elevenlabs;
pub mod openrouter;

use std::{collections::HashMap, path::PathBuf};

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use secret_service::{Collection, EncryptionType, Item, SearchItemsResult, SecretService};
use serde::{Deserialize, Serialize};

/// Which backend transcribes: the one chosen with `stt provider set`, else
/// ElevenLabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    /// Realtime: text is typed while you speak.
    #[value(name = "elevenlabs")]
    ElevenLabs,
    /// Realtime, with Cartesia Ink 2.
    #[value(name = "cartesia")]
    Cartesia,
    /// Batch: the recording is transcribed once you stop.
    #[value(name = "openrouter")]
    OpenRouter,
}

impl Provider {
    pub const ALL: [Self; 3] = [Self::ElevenLabs, Self::Cartesia, Self::OpenRouter];

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
            Self::Cartesia => "cartesia",
            Self::OpenRouter => "openrouter",
        }
    }

    /// The environment variable and keyring account holding the API key.
    pub fn key_name(self) -> &'static str {
        match self {
            Self::ElevenLabs => "ELEVENLABS_API_KEY",
            Self::Cartesia => "CARTESIA_API_KEY",
            Self::OpenRouter => "OPENROUTER_API_KEY",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::ElevenLabs => "ElevenLabs",
            Self::Cartesia => "Cartesia",
            Self::OpenRouter => "OpenRouter",
        }
    }
}

/// What a realtime backend reports while dictating.
#[derive(Debug)]
pub enum Update {
    /// A finalised segment of transcript, to be set apart with a space.
    Committed(String),
    /// A finalised piece of transcript carrying its own spacing, to be typed
    /// as is.
    Delta(String),
    /// Everything sent before the final commit has been transcribed.
    Flushed,
    /// A message we don't act on (session start, partial transcripts, …).
    Other,
}

/// The sending half of a realtime transcription session.
pub trait Transmit {
    /// Streams a chunk of 16-bit mono PCM at `SAMPLE_RATE`.
    async fn send_audio(&mut self, pcm: &[u8]) -> Result<()>;
    /// Asks the server to finalise everything sent so far.
    async fn commit(&mut self) -> Result<()>;
    async fn close(self);
}

/// The receiving half of a realtime transcription session.
pub trait Receive {
    /// Whether the final commit is answered by a single `Committed`, rather
    /// than by transcripts followed by `Flushed`.
    const ONE_TRANSCRIPT_PER_COMMIT: bool;
    /// Next server update, or `None` when the socket closes.
    async fn next(&mut self) -> Option<Result<Update>>;
}

/// Both halves of a connected realtime transcription session.
pub struct Connection<T: Transmit, R: Receive> {
    pub sender: T,
    pub receiver: R,
}

/// Settings saved by `stt provider set`.
#[derive(Serialize, Deserialize)]
struct Config {
    provider: Provider,
}

/// `$XDG_CONFIG_HOME/stt/config.json`.
fn config_path() -> Result<PathBuf> {
    let base: PathBuf = match std::env::var_os("XDG_CONFIG_HOME").filter(|dir| !dir.is_empty()) {
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
    let name: &str = provider.key_name();
    if let Ok(key) = std::env::var(name)
        && !key.is_empty()
    {
        return Ok(key);
    }
    let service: SecretService<'_> = SecretService::connect(EncryptionType::Dh).await?;
    let attributes: HashMap<&str, &str> = HashMap::from([("service", SERVICE), ("account", name)]);
    let found: SearchItemsResult<Item<'_>> = service.search_items(attributes).await?;
    let item: &Item<'_> = match (found.unlocked.first(), found.locked.first()) {
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
    let name: &str = provider.key_name();
    let service: SecretService<'_> = SecretService::connect(EncryptionType::Dh).await?;
    let collection: Collection<'_> = service.get_default_collection().await?;
    if collection.is_locked().await? {
        collection.unlock().await?;
    }
    let attributes: HashMap<&str, &str> = HashMap::from([("service", SERVICE), ("account", name)]);
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

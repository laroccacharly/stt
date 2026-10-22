//! Speech-to-text backends.

pub mod cartesia;
pub mod elevenlabs;
pub mod local;
pub mod openrouter;

use std::{collections::HashMap, future::Future, path::PathBuf, pin::Pin};

use anyhow::{Context, Result, bail};
use clap::{ValueEnum, builder::PossibleValue};
use secret_service::{Collection, EncryptionType, Item, SearchItemsResult, SecretService};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

/// A boxed future, so provider functions can be stored in the registry.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

/// Connects a realtime provider with an API key.
pub type Connect = for<'a> fn(&'a str) -> BoxFuture<'a, Result<Live>>;

/// Transcribes a whole recording with an API key.
pub type Transcribe = for<'a> fn(&'a str, &'a [u8]) -> BoxFuture<'a, Result<String>>;

/// What a provider needs from the dictation session.
#[derive(Clone, Copy)]
pub enum Backend {
    Realtime(Connect),
    Batch {
        transcribe: Transcribe,
        /// Fall back to ElevenLabs batch when transcription fails.
        fallback: bool,
    },
    Local(&'static local::Model),
}

/// Everything about a provider, declared once in `providers!`.
pub struct Definition {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    pub label: &'static str,
    pub company: &'static str,
    pub key: Option<&'static str>,
    pub backend: Backend,
}

/// Declares the provider list once: the enum, `ALL`, and the metadata behind
/// `name`, `label`, `company`, `key_name` and `backend` are all generated.
macro_rules! providers {
    ($(
        $(#[$meta:meta])*
        $variant:ident {
            name: $name:literal,
            aliases: $aliases:expr,
            label: $label:literal,
            company: $company:literal,
            key: $key:expr,
            backend: $backend:expr $(,)?
        }
    ),* $(,)?) => {
        /// Which backend transcribes: the one chosen with `stt provider set`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Provider {
            $($(#[$meta])* $variant),*
        }

        impl Provider {
            pub const ALL: &'static [Self] = &[$(Self::$variant),*];

            fn definition(self) -> &'static Definition {
                static DEFINITIONS: &[Definition] = &[
                    $(Definition {
                        name: $name,
                        aliases: $aliases,
                        label: $label,
                        company: $company,
                        key: $key,
                        backend: $backend,
                    }),*
                ];
                &DEFINITIONS[self as usize]
            }
        }
    };
}

providers! {
    /// Realtime: text is typed while you speak.
    // `elevenlabs` is its name from before the batch variant existed.
    ElevenLabsRealtime {
        name: "elevenlabs-realtime",
        aliases: &["elevenlabs"],
        label: "ElevenLabs realtime",
        company: "ElevenLabs",
        key: Some("ELEVENLABS_API_KEY"),
        backend: Backend::Realtime(elevenlabs::connect_boxed),
    },
    /// Batch: the recording is transcribed once you stop, with Scribe v2.
    ElevenLabsBatch {
        name: "elevenlabs-batch",
        aliases: &[],
        label: "ElevenLabs batch",
        company: "ElevenLabs",
        key: Some("ELEVENLABS_API_KEY"),
        backend: Backend::Batch {
            transcribe: elevenlabs::transcribe_boxed,
            fallback: false,
        },
    },
    /// Realtime, with Cartesia Ink 2.
    Cartesia {
        name: "cartesia",
        aliases: &[],
        label: "Cartesia",
        company: "Cartesia",
        key: Some("CARTESIA_API_KEY"),
        backend: Backend::Realtime(cartesia::connect_boxed),
    },
    /// Batch: the recording is transcribed once you stop; falls back to
    /// ElevenLabs batch if that fails.
    OpenRouter {
        name: "openrouter",
        aliases: &[],
        label: "OpenRouter",
        company: "OpenRouter",
        key: Some("OPENROUTER_API_KEY"),
        backend: Backend::Batch {
            transcribe: openrouter::transcribe_boxed,
            fallback: true,
        },
    },
    /// Local: Phonon-2 on this machine's CPU, English only.
    Phonon2 {
        name: "phonon-2",
        aliases: &[],
        label: "Phonon-2 (local)",
        company: "Fermion Research",
        key: None,
        backend: Backend::Local(&local::Model {
            name: "phonon",
            script: include_str!("local/phonon.py"),
        }),
    },
    /// Local: Parakeet Redux on this machine's CPU, 25 languages.
    ParakeetRedux {
        name: "parakeet-redux",
        aliases: &[],
        label: "Parakeet Redux (local)",
        company: "Moondream",
        key: None,
        backend: Backend::Local(&local::Model {
            name: "parakeet_redux",
            script: include_str!("local/parakeet_redux.py"),
        }),
    },
    /// Local: Underdog Bark 3.0 Clean on this machine's CPU, English only,
    /// with fillers and false starts removed.
    Bark3Clean {
        name: "bark-3.0-clean",
        aliases: &[],
        label: "Underdog Bark 3.0 Clean (local)",
        company: "Conway Research",
        key: None,
        backend: Backend::Local(&local::Model {
            name: "bark",
            script: include_str!("local/bark.py"),
        }),
    },
    /// Local: Underdog Bark 3.0 Clean on this machine's GPU through tinybark's
    /// ONNX Runtime backend, from the tinybark checkout next to stt's.
    OnnxBark {
        name: "onnxbark",
        aliases: &[],
        label: "Underdog Bark 3.0 Clean (local GPU, ONNX)",
        company: "Conway Research",
        key: None,
        backend: Backend::Local(&local::Model {
            name: "onnxbark",
            script: include_str!("local/onnxbark.py"),
        }),
    },
}

impl Provider {
    /// The provider in use now.
    pub fn current() -> Result<Self> {
        let config: Option<Config> =
            json_store::read_opt(&config_path()?).context("reading the stt config")?;
        Ok(config.map_or(Self::ElevenLabsRealtime, |config| config.provider))
    }

    /// Makes this the provider used from now on.
    pub fn save(self) -> Result<()> {
        json_store::write(&config_path()?, &Config { provider: self })
            .context("saving the stt config")?;
        Ok(())
    }

    /// The name used on the command line and in the config.
    pub fn name(self) -> &'static str {
        self.definition().name
    }

    /// The environment variable and keyring account holding the API key;
    /// local models have none.
    pub fn key_name(self) -> Option<&'static str> {
        self.definition().key
    }

    /// The model run on this machine, for local providers.
    pub fn local_model(self) -> Option<&'static local::Model> {
        match self.definition().backend {
            Backend::Local(model) => Some(model),
            _ => None,
        }
    }

    /// How this provider transcribes.
    pub fn backend(self) -> Backend {
        self.definition().backend
    }

    /// Whether `name` is this provider's name or one of its aliases.
    pub fn matches(self, name: &str) -> bool {
        let definition: &Definition = self.definition();
        definition.name.eq_ignore_ascii_case(name)
            || definition
                .aliases
                .iter()
                .any(|alias| alias.eq_ignore_ascii_case(name))
    }

    /// The company behind the provider, for API key messages.
    pub fn company(self) -> &'static str {
        self.definition().company
    }

    pub fn label(self) -> &'static str {
        self.definition().label
    }
}

impl ValueEnum for Provider {
    fn value_variants<'a>() -> &'a [Self] {
        Self::ALL
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        let definition: &Definition = self.definition();
        let mut value: PossibleValue = PossibleValue::new(definition.name).help(definition.label);
        for alias in definition.aliases {
            value = value.alias(*alias);
        }
        Some(value)
    }
}

impl Serialize for Provider {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl<'de> Deserialize<'de> for Provider {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let name: String = String::deserialize(deserializer)?;
        Self::ALL
            .iter()
            .copied()
            .find(|provider| provider.matches(&name))
            .ok_or_else(|| de::Error::custom(format!("unknown provider: {name}")))
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

/// A realtime connection with its concrete types erased, so the provider
/// registry can hand one to the session without a per-provider match.
pub struct Live {
    pub sender: Box<dyn TransmitDyn>,
    pub receiver: Box<dyn ReceiveDyn>,
}

impl Live {
    pub fn new<T, R>(connection: Connection<T, R>) -> Self
    where
        T: Transmit + 'static,
        R: Receive + 'static,
    {
        Self {
            sender: Box::new(connection.sender),
            receiver: Box::new(connection.receiver),
        }
    }
}

/// Object-safe version of `Transmit`, with boxed futures.
pub trait TransmitDyn {
    fn send_audio<'a>(&'a mut self, pcm: &'a [u8]) -> BoxFuture<'a, Result<()>>;
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<()>>;
    fn close(self: Box<Self>) -> BoxFuture<'static, ()>;
}

impl<T: Transmit + 'static> TransmitDyn for T {
    fn send_audio<'a>(&'a mut self, pcm: &'a [u8]) -> BoxFuture<'a, Result<()>> {
        Box::pin(Transmit::send_audio(self, pcm))
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<()>> {
        Box::pin(Transmit::commit(self))
    }

    fn close(self: Box<Self>) -> BoxFuture<'static, ()> {
        Box::pin(async move { Transmit::close(*self).await })
    }
}

/// Object-safe version of `Receive`, with boxed futures.
pub trait ReceiveDyn {
    fn one_transcript_per_commit(&self) -> bool;
    fn next<'a>(&'a mut self) -> BoxFuture<'a, Option<Result<Update>>>;
}

impl<T: Receive> ReceiveDyn for T {
    fn one_transcript_per_commit(&self) -> bool {
        T::ONE_TRANSCRIPT_PER_COMMIT
    }

    fn next<'a>(&'a mut self) -> BoxFuture<'a, Option<Result<Update>>> {
        Box::pin(Receive::next(self))
    }
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
pub(crate) async fn api_key(provider: Provider) -> Result<String> {
    let name: &str = provider
        .key_name()
        .with_context(|| format!("{} needs no API key", provider.label()))?;
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
            provider.company()
        ),
    };
    Ok(String::from_utf8(item.get_secret().await?)?)
}

/// Saves `key` in the keyring for `stt` to find, replacing any earlier one.
pub async fn save_api_key(provider: Provider, key: &str) -> Result<()> {
    let name: &str = provider
        .key_name()
        .with_context(|| format!("{} needs no API key", provider.label()))?;
    let service: SecretService<'_> = SecretService::connect(EncryptionType::Dh).await?;
    let collection: Collection<'_> = service.get_default_collection().await?;
    if collection.is_locked().await? {
        collection.unlock().await?;
    }
    let attributes: HashMap<&str, &str> = HashMap::from([("service", SERVICE), ("account", name)]);
    collection
        .create_item(
            &format!("stt {} API key", provider.company()),
            attributes,
            key.as_bytes(),
            true,
            "text/plain",
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_names_and_key_requirements_are_consistent() {
        let mut names: Vec<&str> = Vec::new();
        for &provider in Provider::ALL {
            let name: &str = provider.name();
            assert!(!names.contains(&name), "duplicate provider name: {name}");
            names.push(name);
            assert!(provider.matches(name));
            for alias in provider.definition().aliases {
                assert!(provider.matches(alias), "{name} alias {alias}");
            }
            let has_key: bool = provider.key_name().is_some();
            let is_local: bool = matches!(provider.backend(), Backend::Local(_));
            assert_ne!(has_key, is_local, "{name}");
        }
    }

    #[test]
    fn serializes_by_name_and_reads_aliases() {
        let json: String = serde_json::to_string(&Provider::Bark3Clean).unwrap();
        assert_eq!(json, "\"bark-3.0-clean\"");
        let provider: Provider = serde_json::from_str("\"elevenlabs\"").unwrap();
        assert_eq!(provider, Provider::ElevenLabsRealtime);
    }
}

# Migration plan: Rust to Effect

## Why

stt is mostly glue: it records with `pw-record`, decodes with `ffmpeg`, types
with `wtype`/`wl-copy`, talks HTTP/WebSocket to cloud providers and a Unix
socket to the local Python servers, and stores history in SQLite. None of that
needs Rust's performance. Rewriting it on Effect (Bun) lets the transcription
core be imported by other projects as a library, with typed errors,
retries/timeouts and the same stack as everything else.

The one piece that genuinely benefits from Rust is the waveform overlay, a wlr
layer-shell surface (`smithay-client-toolkit` + `tiny-skia`) with no good
TypeScript equivalent. It stays in Rust as a separate sidecar binary.

## Target layout

```
packages/
  stt/                 # Effect library + CLI (TypeScript, Bun)
    src/
      audio/           # Microphone, decode, splitAtPauses, level
      provider/        # Provider registry + one Layer per backend
      history/         # History service (bun:sqlite + migrations/)
      desktop/         # Typist (wtype), paste (wl-copy), Notifier, Hyprland install
      keyring/         # API keys via secret-tool
      session/         # realtime / batch / local dictation sessions
      cli/             # commands, built into the `stt` binary
      index.ts         # public library API
  overlay/             # Rust sidecar: `stt-overlay` (today's src/overlay.rs)
```

`effect` and `@effect/*` are `peerDependencies` (`^4.0.0`) with the exact
latest version (4.1.1 at the time of writing) pinned in `devDependencies`.
Lint with oxlint (typeCheck + typeAware) and `github:laroccacharly/ts-lint`.

## Services

Each is an Effect service with a live Layer and, where useful, a test Layer.

| Service | Replaces | Implementation |
|---|---|---|
| `Microphone` | `audio.rs` | `pw-record` child process → `Stream<Uint8Array>` of 100 ms PCM chunks |
| `Audio` | `audio::decode`, `split_at_pauses`, `level` | `ffmpeg` decode; pure functions for pauses and level |
| `Transcriber` | `provider/*` | Tagged union of backends: `Realtime` (stream in, `Update` stream out), `Batch` (bytes → text, optional ElevenLabs fallback), `Local` (Unix socket to the `uv` server) |
| `ProviderConfig` | `Provider::current/save`, json-store | JSON file under `$XDG_CONFIG_HOME/stt` |
| `Keyring` | `secret-service` crate | `secret-tool store/lookup`, with env var override as today |
| `History` | `history.rs` | `bun:sqlite`, reusing `migrations/*.sql` unchanged so existing data carries over |
| `Typist` | `Typist`, `wtype`, `paste` | Queue that types segments in order; tracks the full transcript |
| `Notifier` | `notify-rust` | `notify-send` |
| `Overlay` | `overlay.rs` handle | Spawns `stt-overlay`, writes events to its stdin; no-op if it fails to start |

Providers stay declared once in a registry (name, aliases, label, company,
model, key, backend), like the `providers!` macro does today. Error cases
(`ConnectionClosed`, `NoFinalTranscript`, `MicrophoneStopped`, `TypingFailed`,
`MissingApiKey`, ...) become tagged errors so callers can match on them.

The local Python servers (`src/provider/local/*.py`) move as they are; only the
launcher (start via `uv`, PID/socket/log files, stop others) is ported.

## Overlay sidecar

`stt-overlay` is today's `overlay.rs` plus a small `main` that reads one JSON
event per line on stdin:

```
{"type":"level","value":0.42}
{"type":"phase","phase":"listening"}   # connecting | listening | finishing | transcribing
{"type":"close"}
```

It exits on `close` or when stdin closes, so it never outlives the session.
The Effect side treats it as optional: if the binary is missing or crashes,
dictation keeps working, as it does now.

## CLI

Same commands and behaviour: `toggle` (default), `status`, `record`,
`transcribe <file>`, `login`, `provider ls|set`, `history ls`, `install`,
`uninstall`, `demo`. `toggle` keeps the PID file + `SIGUSR2` protocol and
spawns `record` detached in its own process group. Same env vars
(`STT_ELEVENLABS_URL`, `STT_CARTESIA_URL`, `STT_OPENROUTER_URL`, `STT_TYPED`,
XDG dirs) so the tests can drive it the same way.

Built with `bun build --compile` into a single `stt` binary; `stt install`
writes the Hyprland binding with its absolute path, as today.

## Testing

`tests/e2e.rs` is already black-box: it runs the `stt` binary with fake
`pw-record`/`wtype` on `PATH` and fake provider servers. Port it to `bun test`
first, against the Rust binary, so the suite is proven before the rewrite, then
point it at the new binary. Keep the fixtures (`hello.pcm`, `quilter.pcm`).
Add a few library-level tests through the public API (e.g. `Transcriber` with a
fake server Layer). Whole suite under 20 s.

## Steps

1. **Scaffold**: Bun workspace, `packages/stt` with Effect, oxlint, ts-lint;
   move the Rust crate to `packages/overlay` reduced to the overlay.
2. **Overlay sidecar**: add the stdin JSON `main`, `cargo install` it as
   `stt-overlay`; check it with `demo`.
3. **Port the e2e tests** to `bun test`, still against the Rust `stt`.
4. **Core services**: `Audio`, `Microphone`, `ProviderConfig`, `Keyring`,
   `History`, `Notifier`, `Typist`, `Overlay`.
5. **Providers**: ElevenLabs realtime + batch, Cartesia, OpenRouter, then the
   local `uv` servers.
6. **Sessions + CLI**: realtime, batch and local sessions, `toggle`/`record`
   and the remaining commands.
7. **Switch over**: e2e green against the new binary; `stt install` to rebind
   End; use it daily for a while.
8. **Clean up**: delete the old Rust app code (keep only the overlay crate),
   update `AGENTS.md` (Bun build/test/lint steps instead of cargo) and the
   README; publish the library entry point for other projects
   (`github:laroccacharly/stt`).

## Open questions

- Library scope: is the public API just `Transcriber` + `History`, or the whole
  dictation session (mic → provider → typing)?
- Version: continue from 0.19.x or start the TypeScript app at 1.0.0.

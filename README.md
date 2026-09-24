# stt

Live ElevenLabs Scribe dictation for Omarchy / Hyprland. Press **End** to start; press it again to finish. Committed text is typed into the focused window as it arrives, and a waveform overlay at the bottom of the screen shows your voice while recording (grey while connecting, blue→purple while listening, green while finishing).

## Requirements

- Rust (`rustup` or `pacman -S rust`)
- PipeWire `pw-record`, `wtype`, a notification daemon (mako on Omarchy)
- A Wayland compositor with `wlr-layer-shell` (Hyprland)
- An ElevenLabs API key with speech-to-text access, from `ELEVENLABS_API_KEY` or the `cterm` entry in the Secret Service keyring (gnome-keyring)

## Install

```sh
cargo install --path .    # builds a release binary into ~/.cargo/bin/stt
stt install               # binds End in Hyprland (~/.config/hypr/stt.lua)
```

`~/.cargo/bin` must be on your `PATH` (it is on Omarchy). After changing the code, run `cargo install --path .` again.

To remove: `stt uninstall && cargo uninstall stt`.

## Usage

| Command         | What it does                                   |
| --------------- | ---------------------------------------------- |
| `stt` / `stt toggle` | Start dictation, or stop the running one  |
| `stt status`    | Print `recording` or `idle`                    |
| `stt record`    | Run a session in the foreground (for debugging) |
| `stt demo`      | Show the overlay with fake audio               |

Audio is streamed to ElevenLabs while recording; nothing is saved locally. Logs are at `$XDG_RUNTIME_DIR/stt/stt.log`.

## Testing

```sh
cargo test
```

Unit tests cover audio levels and the protocol messages. `tests/e2e.rs` runs the real `stt` binary end to end: a local WebSocket server plays ElevenLabs (via `STT_ELEVENLABS_URL`), and fake `pw-record` / `wtype` scripts on `PATH` stand in for the microphone and keyboard. The tests run headless, with no overlay window or notifications.

## Layout

- `src/main.rs`: CLI, toggle via PID file and `SIGUSR2`, the recording session
- `src/audio.rs`: microphone capture (`pw-record`) and loudness
- `src/elevenlabs.rs`: API key lookup and the realtime WebSocket protocol
- `src/overlay.rs`: layer-shell waveform (smithay-client-toolkit + tiny-skia) on its own thread
- `src/hyprland.rs`: installing the key binding

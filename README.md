# stt

Live ElevenLabs dictation for Hyprland. Press **End** to start and again to stop. Text is typed into the focused window as you speak, and a waveform overlay shows you're recording.

## Requirements

- Rust
- `pw-record` (PipeWire), `wtype`, a notification daemon
- Hyprland (or another compositor with `wlr-layer-shell`)
- An API key in `ELEVENLABS_API_KEY` or `OPENROUTER_API_KEY`; run `stt login` to save it in the keyring

## Install

```sh
cargo install --path .   # installs ~/.cargo/bin/stt
stt install              # binds End in Hyprland
```

Remove with `stt uninstall && cargo uninstall stt`. Logs: `$XDG_RUNTIME_DIR/stt/stt.log`.

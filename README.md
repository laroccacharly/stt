# stt

Live dictation for Hyprland. Press **End** to start and again to stop. Your speech is transcribed by a cloud speech-to-text provider or a local model (Phonon-2, Parakeet Redux, Underdog Bark 3.0 Clean, on the CPU or on the GPU through tinybark), the text is typed into the focused window, and a waveform overlay shows you're recording.

## Demo

[![stt demo](https://img.youtube.com/vi/emDnKxrjTmU/maxresdefault.jpg)](https://youtu.be/emDnKxrjTmU)

## Requirements

- Rust
- `pw-record` (PipeWire), `wtype`, `wl-clipboard`, a notification daemon
- Hyprland (or another compositor with `wlr-layer-shell`)
- An API key for a cloud provider, or [uv](https://docs.astral.sh/uv/) for the local models

## Install

```sh
cargo install --path .   # installs ~/.cargo/bin/stt
stt install              # binds End in Hyprland
```

Remove with `stt uninstall && cargo uninstall stt`. Logs: `$XDG_RUNTIME_DIR/stt/stt.log`.

## Providers

```sh
stt provider ls          # list providers; * marks the one in use
stt provider set         # choose one (or: stt provider set <name>)
```

Each provider reads its key from an environment variable named `<PROVIDER>_API_KEY` (see `.env.example` for the full list). Set it, then run `stt login` to save it in the keyring so the End binding can find it.

## License

MIT, see [LICENSE](LICENSE).

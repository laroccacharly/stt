# stt

Speech to text for Hyprland. Press **End** to start dictating and again to stop: the recording is transcribed by a local model (Phonon-2.5 English) or by OpenRouter, the text is pasted into the focused window, and a waveform overlay shows you're recording. Built with Bun and [Effect](https://effect.website).

## Demo

[![stt demo](https://img.youtube.com/vi/emDnKxrjTmU/maxresdefault.jpg)](https://youtu.be/emDnKxrjTmU)

## Requirements

- [Bun](https://bun.sh), and [`overlay`](https://github.com/laroccacharly/overlay) built next to this repository
- `pw-record` (PipeWire), `wtype`, `wl-clipboard`, `ffmpeg`, `notify-send`
- Hyprland (or another compositor with `wlr-layer-shell`), systemd
- [uv](https://docs.astral.sh/uv/) for the local model, or an OpenRouter API key

## Install

```sh
(cd ../overlay && bun install && bun run build && bun link)
bun install && bun link
stt service install
stt install
```

## License

MIT, see [LICENSE](LICENSE).

## Local transcription API

Run `stt service install` to enable and start the systemd user service at login. Use `stt service status`, `stt service reinstall`, and `stt service uninstall` to manage it. `stt serve` runs the server in the foreground. It listens on `127.0.0.1:4318`. Desktop dictation, `stt transcribe`, and API clients all use this service and the provider selected by `stt provider set`. `stt-server` reads the OpenRouter key from the OS keychain, so store it with `stt login`. The service starts the local Python model as a child when needed; it does not require a separate model service. The API accepts browser recordings and decodes them with ffmpeg before transcription. See [client/README.md](client/README.md) for the client contract.

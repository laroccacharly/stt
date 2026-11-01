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
stt install
```

## License

MIT, see [LICENSE](LICENSE).

## Local transcription API

Run `stt serve` to listen on `127.0.0.1:4318`. The API accepts browser recordings and decodes them with ffmpeg before transcription using the configured provider. See [client/README.md](client/README.md) for the client contract.

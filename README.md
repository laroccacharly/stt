# stt

Live ElevenLabs Scribe dictation for Omarchy. Press **End** to start; press it again to finish. Notifications show when dictation starts and finishes. Committed text is typed into the focused application as it arrives.

## Requirements

- Bun, PipeWire `pw-record`, `wtype`, and `notify-send`.
- An ElevenLabs API key with speech-to-text access. The app reads `ELEVENLABS_API_KEY` or the existing `cterm` Bun keychain entry.

Run `bun install && bun link`, then `stt install` to bind End in Hyprland (`stt uninstall` removes it). Other commands: `stt toggle`, `stt status`.

Audio is streamed to ElevenLabs while recording. No audio is saved locally. Logs are at `$XDG_RUNTIME_DIR/stt/stt.log`.

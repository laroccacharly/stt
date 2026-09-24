# stt

Live ElevenLabs Scribe dictation for Omarchy. Press **End** to start; press it again to finish. Partial text appears in a replaceable notification. Committed text is typed into the focused application as it arrives.

## Requirements

- Bun, PipeWire `pw-record`, `wtype`, and `notify-send`.
- An ElevenLabs API key with speech-to-text access. The app reads `ELEVENLABS_API_KEY` or the existing `cterm` Bun keychain entry.

Run `bun install`, then `bun stt toggle` or `bun stt status`. The local shortcut runs `bun /home/clarocca/Work/stt/src/main.ts toggle`.

Audio is streamed to ElevenLabs while recording. No audio is saved locally. Logs are at `$XDG_RUNTIME_DIR/stt/stt.log`.

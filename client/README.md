# stt client

Effect client for the stt HTTP transcription API. Run `stt serve` (or `stt service install`) to start the local server.

Provide `SttClient.layer(baseUrl)` and call `prepare` before recording, then `transcribe({ data, contentType })` or `transcribeDetailed` for the transcript with the provider and model the server used.

The client sends raw audio bytes to `POST /v1/transcribe` with the audio MIME type as `Content-Type`; the server decodes browser audio (including WebM and MP4) with ffmpeg and chooses the configured provider. `POST /v1/prepare` has no body and loads the local model or checks the provider's key ahead of time. Success is JSON `{ "text": "..." }`, plus `provider` and `model` when the server knows them; errors are JSON `{ "message": "..." }` with a non-2xx status. Run the server on a trusted local interface.

The package requires Bun (it exports TypeScript) and Effect 4.1.1 or a compatible 4.x release. It does not include the server, models, or desktop dependencies.

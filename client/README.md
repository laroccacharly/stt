# stt client

Effect client for the proposed stt HTTP transcription API. Run `stt serve` to start the local server.

Provide `SttClient.layer(baseUrl)` and call `transcribe({ data, contentType })`. The client sends raw audio bytes to `POST /v1/transcribe` with the audio MIME type as `Content-Type`. Success is JSON `{ "text": "..." }`; errors are JSON `{ "message": "..." }` with a non-2xx status. The server decodes browser audio (including WebM and MP4) with ffmpeg and chooses the configured provider. Run the server on a trusted local interface.

The package requires Bun (it exports TypeScript) and Effect 4.1.1 or a compatible 4.x release. It does not include the server, models, or desktop dependencies.

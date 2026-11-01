# stt client

Effect client for the stt HTTP transcription API. Run `stt serve` (or `stt service install`) to start the local server.

Provide `SttClient.layer(baseUrl)` and call `prepare` before recording, then `transcribe({ data, contentType })` or `transcribeDetailed` for the transcript with the provider and model the server used. Pass `{ timeout }` to the layer to bound each call; without it a call waits as long as the server works, since a long file or the first local model load can take many minutes.

The client sends raw audio or video bytes to `POST /v1/transcribe` with the file's MIME type as `Content-Type` (`audio/*`, `video/*`, or `application/octet-stream`); the server decodes browser recordings and other media with ffmpeg and chooses the configured provider. `POST /v1/prepare` has no body and loads the local model or checks the provider's key ahead of time. `GET /health` answers `{ "ok": true }` when the server is up. Success is JSON `{ "text": "..." }`, plus `provider` and `model` when the server knows them; errors are JSON `{ "message": "..." }` with a non-2xx status. Run the server on a trusted local interface.

A `SttClientError` has a `message` and a `reason`: `"unreachable"` when the server cannot be contacted, `"timeout"` when it did not answer in time, `"server"` for a non-2xx reply, and `"invalid-response"` when the reply body cannot be decoded.

The package requires Bun (it exports TypeScript) and Effect 4.1.1 or a compatible 4.x release. It does not include the server, models, or desktop dependencies.

import { Clock, Effect, Stream } from "effect"
import { Overlay, layer as overlayLayer } from "overlay"
import { Microphone } from "../audio/microphone.ts"
import { concatenate, level, wav } from "../audio/pcm.ts"
import { SttClient, SttClientError } from "../../client/index.ts"
import { Schema } from "effect"
import { Notifier, notifyingErrors } from "../desktop/notifier.ts"
import { Paster } from "../desktop/paster.ts"
import { History } from "../history/history.ts"
import { ProviderName } from "../provider/provider.ts"
import { holdPidFile } from "./pid-file.ts"
import { stopRequested } from "./stop-signal.ts"

// Records until `stop` completes, showing the loudness on the overlay.
const recordUntil = Effect.fnUntraced(function* (stop: Effect.Effect<void>) {
  const overlay = yield* Overlay
  const microphone = yield* Microphone
  const chunks: Uint8Array[] = []
  yield* overlay.phase("listening")
  yield* microphone.chunks.pipe(
    Stream.interruptWhen(stop),
    Stream.runForEach((chunk) => {
      chunks.push(chunk)
      return overlay.level(level(chunk))
    })
  )
  yield* overlay.phase("finishing")
  return concatenate(chunks)
})

// Pastes the transcript; a failure is notified but does not fail the dictation.
const insert = Effect.fnUntraced(function* (text: string) {
  if (text === "") {
    return
  }
  const paster = yield* Paster
  const notifier = yield* Notifier
  yield* paster.paste(text).pipe(Effect.catch((error) => notifier.notify("STT typing failed", error.message, 4000)))
})

// One dictation, run by `stt record` in the background: record until `stt toggle`, transcribe, paste, save to the history.
// Errors are notified while the PID file is still held, so whoever waits for it to go sees the notification.
export const record = Effect.gen(function* () {
  const stop = yield* stopRequested
  yield* holdPidFile
  // So the model loads while you speak, and a missing key fails before the recording is made.
  yield* (yield* SttClient).prepare
  const pcm = yield* recordUntil(stop)
  const started = yield* Clock.currentTimeMillis
  const result = yield* (yield* SttClient).transcribeDetailed({ data: wav(pcm).buffer, contentType: "audio/wav" })
  const provider = yield* Schema.decodeUnknownEffect(ProviderName)(result.provider).pipe(
    Effect.mapError(() => new SttClientError({ message: "Server did not report the transcription provider" }))
  )
  if (result.model === undefined) return yield* new SttClientError({ message: "Server did not report the transcription model" })
  const transcript = result.text
  const transcriptionMs = (yield* Clock.currentTimeMillis) - started
  yield* insert(transcript)
  const history = yield* History
  const notifier = yield* Notifier
  yield* history
    .save({ provider, model: result.model, pcm, transcript, transcriptionMs })
    .pipe(Effect.catch((error) => notifier.notify("STT history failed", error.message, 4000)))
}).pipe(notifyingErrors, Effect.scoped, Effect.provide(overlayLayer()))

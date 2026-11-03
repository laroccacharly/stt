import { Clock, Deferred, Effect, Exit, Fiber, Option, Schema, Scope, Stream } from "effect"
import { Overlay, layer as overlayLayer } from "overlay"
import { Microphone } from "../audio/microphone.ts"
import { concatenate, level } from "../audio/pcm.ts"
import { Notifier, notifyingErrors } from "../desktop/notifier.ts"
import { Paster } from "../desktop/paster.ts"
import { History } from "../history/history.ts"
import type { ProviderName } from "../provider/provider.ts"

// What End toggles: nothing, a recording, or a recording being transcribed and pasted.
export const DictationState = Schema.Literals(["idle", "recording", "transcribing"])
export type DictationState = typeof DictationState.Type

export interface DictationTranscript {
  readonly text: string
  readonly provider: ProviderName
  readonly model: string
}

// How a dictation is transcribed: `prepare` runs as recording starts, so the model loads while you speak.
export interface DictationTranscriber<E, R> {
  readonly prepare: Effect.Effect<void, E, R>
  readonly transcribe: (pcm: Uint8Array) => Effect.Effect<DictationTranscript, E, R>
}

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
const insert = Effect.fnUntraced(function* (text: string, display: Option.Option<string>) {
  if (text === "") {
    return
  }
  const paster = yield* Paster
  const notifier = yield* Notifier
  yield* paster.paste(text, display).pipe(Effect.catch((error) => notifier.notify("STT typing failed", error.message, 4000)))
})

// One dictation: record until `stop`, transcribe, paste into the window on `display`, save to the history. Errors are notified.
export const dictate = <E extends { readonly message: string }, R>(
  stop: Effect.Effect<void>,
  display: Option.Option<string>,
  transcriber: DictationTranscriber<E, R>
) =>
  Effect.gen(function* () {
    // Alongside the recording, so the model loads while you speak; a missing key interrupts the recording at once.
    const [, pcm] = yield* Effect.all([transcriber.prepare, recordUntil(stop)], { concurrency: 2 })
    const started = yield* Clock.currentTimeMillis
    const result = yield* transcriber.transcribe(pcm)
    const transcriptionMs = (yield* Clock.currentTimeMillis) - started
    // Paste first: a failure to save does not lose the dictation.
    yield* insert(result.text, display)
    const notifier = yield* Notifier
    const history = yield* History
    yield* history
      .save({ provider: result.provider, model: result.model, pcm, transcript: result.text, transcriptionMs })
      .pipe(Effect.catch((error) => notifier.notify("STT history failed", error.message, 4000)))
  }).pipe(
    notifyingErrors,
    Effect.scoped,
    Effect.provide(overlayLayer(Option.match(display, { onNone: () => ({}), onSome: (name) => ({ display: name }) }))),
    Effect.ignore
  )

// The one dictation End controls, run in the background of `scope` so it outlives the request that started it.
export const makeDictations = Effect.fnUntraced(function* <R>(
  session: (stop: Effect.Effect<void>, display: Option.Option<string>) => Effect.Effect<void, never, R>
) {
  const scope = yield* Scope.Scope
  const context = yield* Effect.context<R>()
  let state: DictationState = "idle"
  let stop = Deferred.makeUnsafe<void>()
  let running: Option.Option<Fiber.Fiber<void>> = Option.none()

  // Starts a dictation, or stops the recording so it is transcribed; pressing End while transcribing does nothing.
  const toggle = (display: Option.Option<string>): Effect.Effect<DictationState> =>
    Effect.suspend(() => {
      if (state === "recording") {
        state = "transcribing"
        return Deferred.done(stop, Exit.void).pipe(Effect.as(state))
      }
      if (state === "transcribing") {
        return Effect.succeed(state)
      }
      state = "recording"
      stop = Deferred.makeUnsafe<void>()
      return session(Deferred.await(stop), display).pipe(
        Effect.ensuring(
          Effect.sync(() => {
            state = "idle"
            running = Option.none()
          })
        ),
        Effect.provide(context),
        Effect.forkIn(scope),
        Effect.map((fiber) => {
          running = Option.some(fiber)
          return state
        })
      )
    })

  // Drops the dictation without transcribing, pasting or saving it; resolves once the overlay is gone.
  const cancel: Effect.Effect<DictationState> = Effect.suspend(() =>
    Option.match(running, { onNone: () => Effect.void, onSome: Fiber.interrupt }).pipe(Effect.map(() => state))
  )

  return { toggle, cancel, state: Effect.sync(() => state) }
})

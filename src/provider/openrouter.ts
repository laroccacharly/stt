import { Effect, Layer, Redacted } from "effect"
import { Keyring } from "effect-lib/credentials"
import * as Stt from "effect-lib/stt"
import { FetchHttpClient } from "effect/http"
import { wav } from "../audio/pcm.ts"
import { PROVIDERS } from "./provider.ts"

const TRANSCRIBE_OPTIONS: Stt.TranscribeOptions = {
  format: "wav",
  // Drop fillers and false starts: this is dictation, not a transcript.
  provider: { options: { azure: { enhancedMode: { modelOptions: { transcribeStyle: "clean" } } } } },
}

// effect-lib's OpenRouter client with MAI-Transcribe 2; STT_OPENROUTER_URL points it at a test server.
export const openRouterLayer: Layer.Layer<Stt.Stt, never, Keyring> = Stt.layer({
  origin: process.env["STT_OPENROUTER_URL"] ?? Stt.ORIGIN,
  model: PROVIDERS.openrouter.model,
  title: "stt",
}).pipe(Layer.provide(FetchHttpClient.layer))

export const transcribeWithOpenRouter = (pcm: Uint8Array) =>
  Stt.Stt.use((stt) => stt.transcribe(new Blob([wav(pcm)], { type: "audio/wav" }), TRANSCRIBE_OPTIONS)).pipe(Effect.map((text) => text.trim()))

// Fails before recording when there is no key, rather than after.
export const checkOpenRouterKey = Stt.ApiKey.resolve.pipe(Effect.map(Redacted.value), Effect.asVoid)

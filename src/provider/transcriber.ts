import { Effect } from "effect"
import type { CredentialsError } from "effect-lib/credentials"
import { OpenRouter, type OpenRouterError } from "../openrouter/openrouter.ts"
import { Phonon, type PhononError } from "../phonon/phonon.ts"
import { Qwen, type QwenError } from "../qwen/qwen.ts"
import type { ProviderName } from "./provider.ts"

export type TranscribeError = PhononError | QwenError | OpenRouterError | CredentialsError

export interface Transcriber {
  // Run before recording: checks the API key, or starts the local model so it loads while you speak.
  readonly prepare: Effect.Effect<void, TranscribeError>
  // PCM at SAMPLE_RATE to text.
  readonly transcribe: (pcm: Uint8Array) => Effect.Effect<string, TranscribeError>
}

export const transcriberFor = (provider: ProviderName): Effect.Effect<Transcriber, never, Phonon | Qwen | OpenRouter> =>
  Effect.gen(function* () {
    if (provider === "openrouter") {
      const openRouter = yield* OpenRouter
      return { prepare: openRouter.checkKey, transcribe: openRouter.transcribe }
    }
    if (provider === "qwen3-asr-1.7b") {
      const qwen = yield* Qwen
      return { prepare: qwen.ensureRunning, transcribe: qwen.transcribe }
    }
    const phonon = yield* Phonon
    return { prepare: phonon.ensureRunning, transcribe: phonon.transcribe }
  })

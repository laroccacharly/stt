import { Effect } from "effect"
import type { CredentialsError, Keyring } from "effect-lib/credentials"
import type { Stt, SttError } from "effect-lib/stt"
import { Phonon, type PhononError } from "../phonon/phonon.ts"
import { Qwen, type QwenError } from "../qwen/qwen.ts"
import { checkOpenRouterKey, transcribeWithOpenRouter } from "./openrouter.ts"
import type { ProviderName } from "./provider.ts"

export type TranscribeError = PhononError | QwenError | SttError | CredentialsError

export interface Transcriber {
  // Run before recording: checks the API key, or starts the local model so it loads while you speak.
  readonly prepare: Effect.Effect<void, TranscribeError>
  // PCM at SAMPLE_RATE to text.
  readonly transcribe: (pcm: Uint8Array) => Effect.Effect<string, TranscribeError>
}

export const transcriberFor = (provider: ProviderName): Effect.Effect<Transcriber, never, Phonon | Qwen | Stt | Keyring> =>
  Effect.gen(function* () {
    if (provider === "openrouter") {
      const context = yield* Effect.context<Stt | Keyring>()
      return {
        prepare: checkOpenRouterKey.pipe(Effect.provide(context)),
        transcribe: (pcm) => transcribeWithOpenRouter(pcm).pipe(Effect.provide(context)),
      }
    }
    if (provider === "qwen3-asr-1.7b") {
      const qwen = yield* Qwen
      return { prepare: qwen.ensureRunning, transcribe: qwen.transcribe }
    }
    const phonon = yield* Phonon
    return { prepare: phonon.ensureRunning, transcribe: phonon.transcribe }
  })

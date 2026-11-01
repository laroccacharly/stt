import { Effect, type FileSystem } from "effect"
import type { Keyring } from "effect-lib/credentials"
import type { JsonStoreError } from "effect-lib/json-store"
import type { Stt } from "effect-lib/stt"
import { Ffmpeg, type FfmpegError } from "../audio/ffmpeg.ts"
import { splitAtPauses } from "../audio/pcm.ts"
import type { ProviderName } from "../provider/provider.ts"
import { Phonon } from "../phonon/phonon.ts"
import { transcriberFor, type TranscribeError } from "../provider/transcriber.ts"

// Files are sent in pieces this long, since the models are built for short dictations.
const PIECE_MS = 30_000

// The transcript of an audio or video file with the current provider, without the microphone, overlay, pasting or history.
export const transcribeFile: (
  path: string, provider: ProviderName
) => Effect.Effect<string, TranscribeError | FfmpegError | JsonStoreError, Ffmpeg | Phonon | Stt | Keyring | FileSystem.FileSystem> = Effect.fn("transcribeFile")(function* (path: string, provider: ProviderName) {
  // OpenRouter needs no local model, so let go of its memory.
  if (provider === "openrouter") yield* (yield* Phonon).stop
  const transcriber = yield* transcriberFor(provider)
  // Before decoding, so the local model loads meanwhile.
  yield* transcriber.prepare
  const pcm = yield* (yield* Ffmpeg).decodeFile(path)
  const texts = yield* Effect.forEach(splitAtPauses(pcm, PIECE_MS), transcriber.transcribe)
  return texts
    .map((text) => text.trim())
    .filter((text) => text !== "")
    .join(" ")
})

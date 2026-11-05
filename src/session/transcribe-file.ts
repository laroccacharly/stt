import { Effect, Option, type FileSystem } from "effect"
import type { Keyring } from "effect-lib/credentials"
import type { JsonStoreError } from "effect-lib/json-store"
import type { Stt } from "effect-lib/stt"
import { Ffmpeg, type FfmpegError } from "../audio/ffmpeg.ts"
import { isOwnWav, splitAtPauses, WAV_HEADER_BYTES } from "../audio/pcm.ts"
import type { Phonon } from "../phonon/phonon.ts"
import type { ProviderName } from "../provider/provider.ts"
import { transcriberFor, type TranscribeError, type Transcriber } from "../provider/transcriber.ts"
import type { Qwen } from "../qwen/qwen.ts"

// A dictation arrives as the WAV `wav` wrote, so its samples are read as is; anything else goes through ffmpeg.
const ownWavSamples = (path: string) =>
  Effect.tryPromise(async () => {
    const file = Bun.file(path)
    const header = await file.slice(0, WAV_HEADER_BYTES).bytes()
    if (!isOwnWav(header, file.size)) {
      return Option.none<Uint8Array>()
    }
    return Option.some(await file.slice(WAV_HEADER_BYTES).bytes())
  }).pipe(Effect.orElseSucceed(() => Option.none<Uint8Array>()))

const decode = (path: string) =>
  ownWavSamples(path).pipe(Effect.flatMap(Option.match({ onNone: () => Ffmpeg.use((ffmpeg) => ffmpeg.decodeFile(path)), onSome: Effect.succeed })))

// Files are sent in pieces this long, since the models are built for short dictations.
const PIECE_MS = 30_000

type TranscribeServices = Phonon | Qwen | Stt | Keyring

const transcribePieces = (pcm: Uint8Array, transcriber: Transcriber) =>
  Effect.forEach(splitAtPauses(pcm, PIECE_MS), transcriber.transcribe).pipe(
    Effect.map((texts) =>
      texts
        .map((text) => text.trim())
        .filter((text) => text !== "")
        .join(" ")
    )
  )

// The transcript of PCM at SAMPLE_RATE, such as a dictation, with `provider`.
export const transcribePcm: (pcm: Uint8Array, provider: ProviderName) => Effect.Effect<string, TranscribeError, TranscribeServices> = Effect.fn(
  "transcribePcm"
)(function* (pcm: Uint8Array, provider: ProviderName) {
  const transcriber = yield* transcriberFor(provider)
  yield* transcriber.prepare
  return yield* transcribePieces(pcm, transcriber)
})

// The transcript of an audio or video file with `provider`, without the microphone, overlay, pasting or history.
export const transcribeFile: (
  path: string,
  provider: ProviderName
) => Effect.Effect<string, TranscribeError | FfmpegError | JsonStoreError, Ffmpeg | TranscribeServices | FileSystem.FileSystem> = Effect.fn("transcribeFile")(
  function* (path: string, provider: ProviderName) {
    const transcriber = yield* transcriberFor(provider)
    // Before decoding, so the local model loads meanwhile.
    yield* transcriber.prepare
    return yield* transcribePieces(yield* decode(path), transcriber)
  }
)

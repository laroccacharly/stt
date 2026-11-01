import { Context, Effect, Layer, Schema, Stream } from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/process"
import { SAMPLE_RATE } from "./pcm.ts"

export class FfmpegError extends Schema.TaggedError<FfmpegError>()("FfmpegError", {
  message: Schema.String,
}) {}

const PCM_FORMAT = ["-f", "s16le", "-ar", String(SAMPLE_RATE), "-ac", "1"]

// Every chunk is kept as it arrives and copied once, so a long recording is not re-copied per chunk.
const collectBytes = <E>(stream: Stream.Stream<Uint8Array, E>) =>
  Stream.runFold(stream, () => [] as Array<Uint8Array>, (chunks, chunk) => {
    chunks.push(chunk)
    return chunks
  }).pipe(Effect.map((chunks) => Buffer.concat(chunks)))

// Every conversion goes through an ffmpeg child: PCM in or a file, bytes out on stdout.
export class Ffmpeg extends Context.Service<
  Ffmpeg,
  {
    // Any audio or video file ffmpeg reads, as PCM at SAMPLE_RATE.
    readonly decodeFile: (path: string) => Effect.Effect<Uint8Array, FfmpegError>
    // Lossless FLAC of PCM at SAMPLE_RATE, for the history.
    readonly encodeFlac: (pcm: Uint8Array) => Effect.Effect<Uint8Array, FfmpegError>
  }
>()("stt/audio/Ffmpeg") {
  static readonly layer = Layer.effect(
    Ffmpeg,
    Effect.gen(function* () {
      const spawner = yield* ChildProcessSpawner.ChildProcessSpawner

      const run = Effect.fnUntraced(function* (what: string, args: ReadonlyArray<string>, stdin: ChildProcess.CommandInput) {
        const command = ChildProcess.make("ffmpeg", ["-hide_banner", "-loglevel", "error", ...args], { stdin, stdout: "pipe", stderr: "pipe" })
        const [stdout, stderr, code] = yield* Effect.scoped(
          Effect.gen(function* () {
            const handle = yield* spawner.spawn(command)
            return yield* Effect.all([collectBytes(handle.stdout), Stream.mkString(Stream.decodeText(handle.stderr)), handle.exitCode], { concurrency: "unbounded" })
          })
        ).pipe(Effect.mapError((cause) => new FfmpegError({ message: `could not run ffmpeg to ${what}; is it installed? (${cause.message})` })))
        if (code !== 0) {
          return yield* new FfmpegError({ message: `ffmpeg could not ${what}: ${stderr.trim() || `exit code ${code}`}` })
        }
        return stdout
      })

      return Ffmpeg.of({
        decodeFile: (path) => run(`decode ${path}`, ["-i", path, "-vn", ...PCM_FORMAT, "pipe:1"], "ignore"),
        encodeFlac: (pcm) => run("encode FLAC", [...PCM_FORMAT, "-i", "pipe:0", "-c:a", "flac", "-f", "flac", "pipe:1"], Stream.make(pcm)),
      })
    })
  )
}

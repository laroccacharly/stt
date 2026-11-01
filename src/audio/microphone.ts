import { Context, Effect, Layer, Schema, Stream } from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/process"
import { SAMPLE_RATE } from "./pcm.ts"

export class MicrophoneError extends Schema.TaggedError<MicrophoneError>()("MicrophoneError", {
  message: Schema.String,
}) {}

const PW_RECORD = ChildProcess.make("pw-record", ["--raw", "--format", "s16", "--channels", "1", "--rate", String(SAMPLE_RATE), "-"], {
  stdin: "ignore",
  stdout: "pipe",
  stderr: "inherit",
})

// Capture through PipeWire's pw-record, which runs while the stream is pulled.
export class Microphone extends Context.Service<
  Microphone,
  {
    // PCM chunks at SAMPLE_RATE until interrupted; fails if pw-record exits first.
    readonly chunks: Stream.Stream<Uint8Array, MicrophoneError>
  }
>()("stt/audio/Microphone") {
  static readonly layer = Layer.effect(
    Microphone,
    Effect.gen(function* () {
      const spawner = yield* ChildProcessSpawner.ChildProcessSpawner
      const failed = (message: string) => new MicrophoneError({ message })
      const chunks = Stream.unwrap(
        spawner.spawn(PW_RECORD).pipe(
          Effect.mapError((cause) => failed(`could not start pw-record: ${cause.message}`)),
          Effect.map((handle) =>
            handle.stdout.pipe(
              Stream.mapError((cause) => failed(`could not read pw-record: ${cause.message}`)),
              Stream.concat(Stream.fail(failed("Microphone stopped unexpectedly (pw-record exited)")))
            )
          )
        )
      )
      return Microphone.of({ chunks })
    })
  )
}

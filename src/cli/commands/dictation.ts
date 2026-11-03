import { Console, Effect, Schema } from "effect"
import { Argument, Command } from "effect/cli"
import { SttClient } from "../../../client/index.ts"
import { toggle } from "../toggle.ts"

class ReadFileError extends Schema.TaggedError<ReadFileError>()("ReadFileError", {
  message: Schema.String,
}) {}

// `stt` and `stt toggle` normally skip the CLI (see index.ts); this runs for `stt toggle` with flags.
export const toggleDictation = Effect.promise(toggle).pipe(
  Effect.flatMap((code) =>
    Effect.sync(() => {
      process.exitCode = code
    })
  )
)

export const toggleCommand = Command.make("toggle", {}, () => toggleDictation).pipe(
  Command.withDescription("Ask stt-server to start dictation, or stop it if it is running (default)")
)

export const transcribeCommand = Command.make("transcribe", { file: Argument.Path("file", { pathType: "file", mustExist: true }) }, ({ file }) =>
  Effect.gen(function* () {
    const audio = Bun.file(file)
    const data = yield* Effect.tryPromise({
      try: () => audio.arrayBuffer(),
      catch: (error) => new ReadFileError({ message: `could not read ${file}: ${String(error)}` }),
    })
    const text = yield* (yield* SttClient).transcribe({ data, contentType: audio.type || "application/octet-stream" })
    yield* Console.log(text)
  })
).pipe(Command.withDescription("Print the transcript of an audio or video file, using the current provider"))

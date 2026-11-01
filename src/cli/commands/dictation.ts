import { Console, Effect, Schema } from "effect"
import { Argument, Command } from "effect/cli"
import { SttClient } from "../../../client/index.ts"
import { notifyingErrors } from "../../desktop/notifier.ts"
import { record } from "../../session/record.ts"
import { toggle } from "../../session/toggle.ts"

class ReadFileError extends Schema.TaggedError<ReadFileError>()("ReadFileError", {
  message: Schema.String,
}) {}

export const toggleDictation = notifyingErrors(toggle)

export const toggleCommand = Command.make("toggle", {}, () => toggleDictation).pipe(
  Command.withDescription("Start dictation, or stop it if it is running (default)")
)

export const recordCommand = Command.make("record", {}, () => record).pipe(Command.withDescription("Run a dictation in the foreground until SIGUSR2"))

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

import { Console, Effect } from "effect"
import { Argument, Command } from "effect/cli"
import { notifyingErrors } from "../../desktop/notifier.ts"
import { record } from "../../session/record.ts"
import { toggle } from "../../session/toggle.ts"
import { SttClient, SttClientError } from "../../../client/index.ts"

export const toggleDictation = notifyingErrors(toggle)

export const toggleCommand = Command.make("toggle", {}, () => toggleDictation).pipe(Command.withDescription("Start dictation, or stop it if it is running (default)"))

export const recordCommand = Command.make("record", {}, () => record).pipe(Command.withDescription("Run a dictation in the foreground until SIGUSR2"))

export const transcribeCommand = Command.make("transcribe", { file: Argument.Path("file", { pathType: "file", mustExist: true }) }, ({ file }) =>
  Effect.gen(function* () {
    const audio = yield* Effect.tryPromise({ try: () => Bun.file(file).arrayBuffer(), catch: (error) => new SttClientError({ message: String(error) }) })
    const text = yield* (yield* SttClient).transcribe({ data: audio, contentType: "audio/wav" })
    yield* Console.log(text)
  })
).pipe(Command.withDescription("Print the transcript of an audio or video file, using the current provider"))

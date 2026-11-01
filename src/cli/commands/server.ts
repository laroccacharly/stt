import { Effect } from "effect"
import { Command } from "effect/cli"
import { startServer } from "../../server/server.ts"
import { transcribeFile } from "../../session/transcribe-file.ts"

export const serverCommand = Command.make("serve", {}, () => Effect.gen(function* () {
  const context = yield* Effect.context<Effect.Services<ReturnType<typeof transcribeFile>>>()
  const server = startServer((path) => Effect.runPromiseWith(context)(transcribeFile(path)))
  yield* Effect.logInfo(`stt server listening at ${server.url.href}`)
  return yield* Effect.never.pipe(Effect.ensuring(Effect.promise(() => server.stop(true))))
})).pipe(Command.withDescription("Serve the local transcription API on 127.0.0.1:4318"))

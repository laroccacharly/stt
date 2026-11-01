import { Console, Effect } from "effect"
import { Command } from "effect/cli"
import { History } from "../../history/history.ts"

const LIST_LIMIT = 10

const list = Command.make("ls", {}, () =>
  Effect.gen(function* () {
    const history = yield* History
    for (const entry of yield* history.recent(LIST_LIMIT)) {
      yield* Console.log(`${entry.time}  ${entry.provider} (${entry.model})  ${entry.transcript || "(no speech)"}`)
    }
  })
).pipe(Command.withDescription(`List the ${LIST_LIMIT} most recent dictations, newest first`))

export const historyCommand = Command.make("history").pipe(Command.withDescription("Browse saved dictations"), Command.withSubcommands([list]))

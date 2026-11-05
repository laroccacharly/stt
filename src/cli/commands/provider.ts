import { Console, Effect, Option } from "effect"
import { Argument, Command, Prompt } from "effect/cli"
import { currentProvider, saveProvider } from "../../config/config.ts"
import { Phonon } from "../../phonon/phonon.ts"
import { PROVIDERS, ProviderName } from "../../provider/provider.ts"
import { Qwen } from "../../qwen/qwen.ts"

const NAMES = ProviderName.literals
const NAME_WIDTH = Math.max(...NAMES.map((name) => name.length))

const marker = (name: ProviderName, current: ProviderName) => {
  if (name === current) {
    return "*"
  }
  return " "
}

const list = Command.make("ls", {}, () =>
  Effect.gen(function* () {
    const current = yield* currentProvider
    for (const name of NAMES) {
      yield* Console.log(`${marker(name, current)} ${name.padEnd(NAME_WIDTH)}  ${PROVIDERS[name].label}`)
    }
  })
).pipe(Command.withDescription("List the providers, marking the one in use"))

const choose = Effect.gen(function* () {
  const current = yield* currentProvider
  const choices = NAMES.map((name) => ({ title: name, value: name, description: PROVIDERS[name].label }))
  return yield* Prompt.run(Prompt.Select({ message: `Provider (now ${current})`, choices }))
})

// The server reads the selected provider for each transcription.
const switchTo = Effect.fnUntraced(function* (provider: ProviderName) {
  if (provider === "phonon-2.5-en") {
    // Downloads the model and its dependencies now, where their progress shows, rather than during a dictation.
    yield* (yield* Phonon).prepare
  }
  if (provider === "qwen3-asr-1.7b") yield* (yield* Qwen).prepare
  yield* saveProvider(provider)
  yield* Console.log(`Using ${PROVIDERS[provider].label}`)
})

const set = Command.make("set", { provider: Argument.Literals("provider", NAMES).pipe(Argument.optional) }, ({ provider }) =>
  Option.match(provider, { onNone: () => choose, onSome: Effect.succeed }).pipe(Effect.flatMap(switchTo))
).pipe(Command.withDescription("Use this provider from now on; asks which one if not given"))

export const providerCommand = Command.make("provider").pipe(
  Command.withDescription("List or choose the transcription provider"),
  Command.withSubcommands([list, set])
)

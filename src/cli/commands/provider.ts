import { Console, Effect, Option } from "effect"
import * as Systemd from "effect-lib/systemd"
import { Argument, Command, Prompt } from "effect/cli"
import { currentProvider, saveProvider } from "../../config/config.ts"
import { Phonon, SERVICE } from "../../phonon/phonon.ts"
import { PROVIDERS, ProviderName } from "../../provider/provider.ts"

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

// Phonon runs as a service that holds the model in memory, so it starts with the choice and stops when another is chosen.
const switchTo = Effect.fnUntraced(function* (provider: ProviderName) {
  const phonon = yield* Phonon
  if (provider === "phonon-2.5-en") {
    yield* phonon.prepare
    yield* saveProvider(provider)
    yield* phonon.ensureRunning
    return yield* Console.log(`Using ${PROVIDERS[provider].label}; its server is ${Systemd.unitName(SERVICE)}`)
  }
  yield* saveProvider(provider)
  yield* phonon.stop
  yield* Console.log(`Using ${PROVIDERS[provider].label}`)
})

const set = Command.make("set", { provider: Argument.Literals("provider", NAMES).pipe(Argument.optional) }, ({ provider }) =>
  Option.match(provider, { onNone: () => choose, onSome: Effect.succeed }).pipe(Effect.flatMap(switchTo))
).pipe(Command.withDescription("Use this provider from now on; asks which one if not given"))

export const providerCommand = Command.make("provider").pipe(Command.withDescription("List or choose the transcription provider"), Command.withSubcommands([list, set]))

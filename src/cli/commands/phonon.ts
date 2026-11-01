import { Command } from "effect/cli"
import { Phonon } from "../../phonon/phonon.ts"

const serve = Command.make("serve", {}, () => Phonon.use((phonon) => phonon.serve)).pipe(
  Command.withDescription("Run the local model process in the foreground")
)

export const phononCommand = Command.make("phonon").pipe(Command.withDescription("The local Phonon model"), Command.withSubcommands([serve]))

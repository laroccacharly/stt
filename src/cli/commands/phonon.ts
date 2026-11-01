import * as Systemd from "effect-lib/systemd"
import { Command } from "effect/cli"
import { Phonon, SERVICE } from "../../phonon/phonon.ts"

const serve = Command.make("serve", {}, () => Phonon.use((phonon) => phonon.serve)).pipe(
  Command.withDescription(`Run the model server in the foreground; ${Systemd.unitName(SERVICE)} runs this`)
)

const status = Command.make("status", {}, () => Systemd.status(SERVICE)).pipe(Command.withDescription(`Show whether ${Systemd.unitName(SERVICE)} is running, with its latest log lines`))

export const phononCommand = Command.make("phonon").pipe(Command.withDescription("The local Phonon-2.5 English model server"), Command.withSubcommands([serve, status]))

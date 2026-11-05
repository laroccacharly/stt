import { BunRuntime, BunServices } from "@effect/platform-bun"
import { Effect, Layer, Logger } from "effect"
import * as Credentials from "effect-lib/credentials"
import { ApiKey } from "effect-lib/stt"
import { Command } from "effect/cli"
import { SttClient } from "../../client/index.ts"
import packageJson from "../../package.json" with { type: "json" }
import { Ffmpeg } from "../audio/ffmpeg.ts"
import { Microphone } from "../audio/microphone.ts"
import { Notifier } from "../desktop/notifier.ts"
import { Paster } from "../desktop/paster.ts"
import { History } from "../history/history.ts"
import { Phonon } from "../phonon/phonon.ts"
import { openRouterLayer } from "../provider/openrouter.ts"
import { Qwen } from "../qwen/qwen.ts"
import { toggleCommand, toggleDictation, transcribeCommand } from "./commands/dictation.ts"
import { historyCommand } from "./commands/history.ts"
import { installCommand, uninstallCommand } from "./commands/hyprland.ts"
import { phononCommand } from "./commands/phonon.ts"
import { providerCommand } from "./commands/provider.ts"
import { serverCommand, serviceCommand } from "./commands/server.ts"
import { serverUrl } from "./toggle.ts"

const secrets = Credentials.make([ApiKey])

const stt = Command.make("stt", {}, () => toggleDictation).pipe(
  Command.withDescription("Speech to text for Hyprland; with no command, toggles dictation"),
  Command.withSubcommands([
    toggleCommand,
    transcribeCommand,
    providerCommand,
    historyCommand,
    ...secrets.commands,
    installCommand,
    uninstallCommand,
    phononCommand,
    serverCommand,
    serviceCommand,
  ])
)

// Logs go to stderr, so `stt transcribe` prints the transcript alone and the server's journal has both.
const AppLive = Layer.mergeAll(
  SttClient.layer(serverUrl()),
  Ffmpeg.layer,
  Microphone.layer,
  Notifier.layer,
  Paster.layer,
  Phonon.layer,
  Qwen.layer,
  openRouterLayer,
  Layer.succeed(Logger.LogToStderr, true)
).pipe(
  Layer.provideMerge(History.layer.pipe(Layer.provide(Ffmpeg.layer))),
  Layer.provideMerge(secrets.layer(Credentials.bunKeyring())),
  Layer.provideMerge(BunServices.layer)
)

BunRuntime.runMain(Command.run(stt, { version: packageJson.version }).pipe(Effect.provide(AppLive)))

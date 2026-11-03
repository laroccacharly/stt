import { Console, Effect, Option, Semaphore } from "effect"
import { CredentialsError } from "effect-lib/credentials"
import * as Systemd from "effect-lib/systemd"
import { Command, Flag } from "effect/cli"
import { currentProvider } from "../../config/config.ts"
import { cliScript } from "../../paths.ts"
import { Phonon } from "../../phonon/phonon.ts"
import { PROVIDERS, type ProviderName } from "../../provider/provider.ts"
import { transcriberFor } from "../../provider/transcriber.ts"
import { startServer } from "../../server/server.ts"
import { dictate, makeDictations } from "../../session/dictation.ts"
import { transcribeFile, transcribePcm } from "../../session/transcribe-file.ts"

// A service does not inherit the shell's environment, so a key exported there never reaches it.
const withLoginHint = <A, E, R>(effect: Effect.Effect<A, E, R>): Effect.Effect<A, E | CredentialsError, R> =>
  Effect.catchIf(
    effect,
    (error): error is E & CredentialsError => error instanceof CredentialsError && error.reason === "missing",
    (error) =>
      Effect.fail(
        new CredentialsError({
          message: `${error.name} is not set for the transcription server: run \`stt login\` to store it, since the service does not inherit your shell's environment`,
          name: error.name,
          reason: error.reason,
        })
      )
  )

export const serverCommand = Command.make("serve", { port: Flag.Int("port").pipe(Flag.withDefault(4318)) }, ({ port }) =>
  Effect.gen(function* () {
    const context = yield* Effect.context<Effect.Services<ReturnType<typeof transcribeFile>>>()
    const lock = yield* Semaphore.make(1)
    // The provider the previous transcription used, so the local model is unloaded only when it changes.
    let lastProvider: ProviderName | null = null

    // One transcription at a time, with the current provider.
    const transcribing = <A, E, R>(run: (provider: ProviderName) => Effect.Effect<A, E, R>) =>
      withLoginHint(
        Effect.gen(function* () {
          const provider = yield* currentProvider
          if (provider !== lastProvider) {
            // OpenRouter needs no local model, so let go of its memory; keep it while the provider stays.
            if (provider === "openrouter") yield* (yield* Phonon).stop
            lastProvider = provider
          }
          const text = yield* run(provider)
          return { text, provider, model: PROVIDERS[provider].model }
        })
      ).pipe((effect) => lock.withPermit(effect))

    // Called while the caller is still speaking, so the model loads meanwhile and a missing key is reported early.
    const prepare = withLoginHint(
      Effect.gen(function* () {
        const transcriber = yield* transcriberFor(yield* currentProvider)
        yield* transcriber.prepare
      })
    )

    const dictations = yield* makeDictations((stop, display) =>
      dictate(stop, display, { prepare, transcribe: (pcm) => transcribing((provider) => transcribePcm(pcm, provider)) })
    )

    const server = startServer(
      {
        prepare: prepare.pipe(Effect.provide(context)),
        transcribe: (path) => transcribing((provider) => transcribeFile(path, provider)).pipe(Effect.provide(context)),
      },
      port,
      Option.some(dictations)
    )
    yield* Effect.logInfo(`stt server listening at ${server.url.href}`)
    return yield* Effect.never.pipe(Effect.ensuring(Effect.promise(() => server.stop())))
  }).pipe(Effect.scoped)
).pipe(Command.withDescription("Serve the local transcription API and the End dictation on 127.0.0.1"))

export const SERVER_SERVICE: Systemd.ServiceConfig = {
  name: "stt-server",
  description: "STT local transcription API",
  script: cliScript,
  args: ["serve"],
  restartSec: 5,
}

export const serviceCommand = Command.make("service").pipe(
  Command.withDescription("Manage the STT systemd user service"),
  Command.withSubcommands([
    Command.make("install", {}, () => Systemd.install(SERVER_SERVICE).pipe(Effect.andThen(Console.log("Installed and started stt-server.service")))),
    Command.make("reinstall", {}, () => Systemd.reinstall(SERVER_SERVICE).pipe(Effect.andThen(Console.log("Rewrote and restarted stt-server.service")))),
    Command.make("uninstall", {}, () => Systemd.uninstall(SERVER_SERVICE).pipe(Effect.andThen(Console.log("Stopped and removed stt-server.service")))),
    Command.make("status", {}, () =>
      Systemd.isInstalled(SERVER_SERVICE).pipe(
        Effect.flatMap((installed) => {
          if (installed) return Systemd.status(SERVER_SERVICE)
          return Console.log("stt-server.service is not installed")
        })
      )
    ),
  ])
)

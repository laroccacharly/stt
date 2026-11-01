import { join } from "node:path"
import { Context, Duration, Effect, FileSystem, Layer, Schedule, Schema, Stream } from "effect"
import * as Systemd from "effect-lib/systemd"
import { ChildProcess, ChildProcessSpawner } from "effect/process"
import { cacheDirectory, cliScript, runtimeDirectory } from "../paths.ts"
import { exchange, isListening, type NotListening } from "./socket.ts"

export class PhononError extends Schema.TaggedError<PhononError>()("PhononError", {
  message: Schema.String,
}) {}

// The model server runs as this systemd user service, so it loads once and outlives each dictation.
export const SERVICE: Systemd.ServiceConfig = {
  name: "stt-phonon",
  description: "Phonon-2.5 English speech-to-text server for stt",
  script: cliScript,
  args: ["phonon", "serve"],
  restartSec: 5,
}

const SCRIPTS = ["serve.py", "phonon.py"]
const PYTHON_DIRECTORY = join(import.meta.dir, "..", "..", "python")
// The very first start installs the dependencies and downloads the model.
const START_TIMEOUT = Duration.minutes(10)
const LOGS_HINT = `see \`journalctl --user -u ${SERVICE.name}\``

export const socketPath = (): string => join(runtimeDirectory(), "phonon.sock")

const Reply = Schema.fromJsonString(Schema.Struct({ text: Schema.optional(Schema.String), error: Schema.optional(Schema.String) }))

// The transcript in the server's JSON reply, or its error.
export const parseReply = (reply: string): Effect.Effect<string, PhononError> =>
  Schema.decodeEffect(Reply)(reply.trim()).pipe(
    Effect.mapError(() => new PhononError({ message: `The phonon server sent an unexpected reply: ${JSON.stringify(reply)}` })),
    Effect.flatMap((parsed) => {
      if (parsed.error !== undefined) {
        return Effect.fail(new PhononError({ message: `Phonon: ${parsed.error}` }))
      }
      if (parsed.text === undefined) {
        return Effect.fail(new PhononError({ message: "The phonon server sent no transcript" }))
      }
      return Effect.succeed(parsed.text.trim())
    })
  )

// Phonon-2.5 English on this machine's CPU: a Python server run through uv, reached over a Unix socket.
export class Phonon extends Context.Service<
  Phonon,
  {
    // Downloads the model and its dependencies, showing progress.
    readonly prepare: Effect.Effect<void, PhononError>
    // Runs the server in the foreground: what the systemd service runs.
    readonly serve: Effect.Effect<void, PhononError>
    // Starts the service unless it is serving or loading, so the model loads while you speak.
    readonly ensureRunning: Effect.Effect<void, PhononError>
    // Stops and removes the service, freeing the model's memory.
    readonly stop: Effect.Effect<void, PhononError>
    // PCM at SAMPLE_RATE to text, waiting for the server to come up if it is still loading.
    readonly transcribe: (pcm: Uint8Array) => Effect.Effect<string, PhononError>
  }
>()("stt/phonon/Phonon") {
  static readonly layer = Layer.effect(
    Phonon,
    Effect.gen(function* () {
      const fs = yield* FileSystem.FileSystem
      const spawner = yield* ChildProcessSpawner.ChildProcessSpawner
      const services = Layer.mergeAll(Layer.succeed(FileSystem.FileSystem)(fs), Layer.succeed(ChildProcessSpawner.ChildProcessSpawner)(spawner))
      const systemd = <A>(effect: Effect.Effect<A, Systemd.SystemdServiceError, FileSystem.FileSystem | ChildProcessSpawner.ChildProcessSpawner>) =>
        effect.pipe(
          Effect.provide(services),
          Effect.mapError((error) => new PhononError({ message: error.message }))
        )

      // Copies the scripts to the cache directory, rewriting only what changed so uv keeps their environment.
      const writeScripts = Effect.gen(function* () {
        yield* fs.makeDirectory(cacheDirectory(), { recursive: true })
        for (const name of SCRIPTS) {
          const source = yield* fs.readFileString(join(PYTHON_DIRECTORY, name))
          const target = join(cacheDirectory(), name)
          const current = yield* fs.readFileString(target).pipe(Effect.orElseSucceed(() => ""))
          if (current !== source) {
            yield* fs.writeFileString(target, source)
          }
        }
        return join(cacheDirectory(), "phonon.py")
      }).pipe(Effect.mapError((cause) => new PhononError({ message: `could not write the phonon scripts: ${cause.message}` })))

      const runUv = Effect.fnUntraced(function* (args: ReadonlyArray<string>) {
        const script = yield* writeScripts
        const command = ChildProcess.make("uv", ["run", "--quiet", "--script", script, ...args], { stdin: "ignore", stdout: "inherit", stderr: "inherit" })
        const code = yield* spawner.exitCode(command).pipe(Effect.mapError((cause) => new PhononError({ message: `could not start uv; is it installed? (${cause.message})` })))
        if (code !== 0) {
          return yield* new PhononError({ message: `uv ${args.join(" ")} exited with ${code}` })
        }
      })

      // `systemctl --user is-active`: active, activating, inactive, failed…
      const serviceState = Effect.scoped(
        Effect.gen(function* () {
          const handle = yield* spawner.spawn(ChildProcess.make("systemctl", ["--user", "is-active", Systemd.unitName(SERVICE)], { stdout: "pipe", stderr: "ignore" }))
          return (yield* Stream.mkString(Stream.decodeText(handle.stdout))).trim()
        })
      ).pipe(Effect.orElseSucceed(() => "unknown"))

      const isStarting = serviceState.pipe(Effect.map((state) => state === "active" || state === "activating" || state === "reloading"))

      const ensureRunning = Effect.gen(function* () {
        if ((yield* isListening(socketPath())) || (yield* isStarting)) {
          return
        }
        yield* Effect.logInfo(`starting ${Systemd.unitName(SERVICE)}`)
        yield* systemd(Systemd.reinstall(SERVICE))
      })

      // NotListening while the service is still starting, so it is retried; a PhononError once it has stopped.
      const explainNotListening = Effect.fnUntraced(function* (error: NotListening) {
        if (yield* isStarting) {
          return yield* error
        }
        return yield* new PhononError({ message: `The phonon server is not running; ${LOGS_HINT}` })
      })

      const transcribe: (pcm: Uint8Array) => Effect.Effect<string, PhononError> = Effect.fn("Phonon.transcribe")(function* (pcm: Uint8Array) {
        // The server takes an empty request for a readiness probe.
        if (pcm.length === 0) {
          return ""
        }
        const reply = yield* exchange(socketPath(), pcm).pipe(
          Effect.catchTags({
            SocketError: (error) => Effect.fail(new PhononError({ message: `The phonon server hung up: ${error.message}; ${LOGS_HINT}` })),
            NotListening: explainNotListening,
          }),
          Effect.retry({ schedule: Schedule.spaced("100 millis"), while: (error) => error._tag === "NotListening" }),
          Effect.timeoutOrElse({
            duration: START_TIMEOUT,
            orElse: () => Effect.fail(new PhononError({ message: `The phonon server did not start in time; ${LOGS_HINT}` })),
          }),
          Effect.catchTag("NotListening", (error) => Effect.fail(new PhononError({ message: error.message })))
        )
        return yield* parseReply(reply)
      })

      return Phonon.of({
        prepare: runUv(["--prepare"]),
        serve: fs.makeDirectory(runtimeDirectory(), { recursive: true }).pipe(
          Effect.mapError((cause) => new PhononError({ message: cause.message })),
          Effect.andThen(runUv([socketPath()]))
        ),
        ensureRunning,
        stop: systemd(Systemd.uninstall(SERVICE)).pipe(Effect.asVoid),
        transcribe,
      })
    })
  )
}

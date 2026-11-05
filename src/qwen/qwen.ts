import { join } from "node:path"
import { Context, Duration, Effect, FileSystem, Layer, Option, Schedule, Schema, Semaphore } from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/process"
import { cacheDirectory, runtimeDirectory } from "../paths.ts"
import { exchange, isListening, type NotListening } from "../phonon/socket.ts"

export class QwenError extends Schema.TaggedError<QwenError>()("QwenError", {
  message: Schema.String,
}) {}

const SCRIPTS = ["serve.py", "qwen.py"]
const PYTHON_DIRECTORY = join(import.meta.dir, "..", "..", "python")
// The very first start installs the dependencies and downloads the model.
const START_TIMEOUT = Duration.minutes(10)
const LOGS_HINT = "see `journalctl --user -u stt-server`"

export const socketPath = (): string => join(runtimeDirectory(), "qwen.sock")

// The model's process and the server that started it: a model left by a server that is gone can be adopted.
const ModelPid = Schema.fromJsonString(Schema.Struct({ model: Schema.Finite, server: Schema.Finite }))

const Reply = Schema.fromJsonString(Schema.Struct({ text: Schema.optional(Schema.String), error: Schema.optional(Schema.String) }))

// The transcript in the server's JSON reply, or its error.
export const parseReply = (reply: string): Effect.Effect<string, QwenError> =>
  Schema.decodeEffect(Reply)(reply.trim()).pipe(
    Effect.mapError(() => new QwenError({ message: `The qwen server sent an unexpected reply: ${JSON.stringify(reply)}` })),
    Effect.flatMap((parsed) => {
      if (parsed.error !== undefined) {
        return Effect.fail(new QwenError({ message: `Qwen: ${parsed.error}` }))
      }
      if (parsed.text === undefined) {
        return Effect.fail(new QwenError({ message: "The qwen server sent no transcript" }))
      }
      return Effect.succeed(parsed.text.trim())
    })
  )

// Qwen3-ASR on an NVIDIA GPU, served through a Unix socket.
export class Qwen extends Context.Service<
  Qwen,
  {
    // Downloads the model and its dependencies, showing progress.
    readonly prepare: Effect.Effect<void, QwenError>
    // Runs the model server in the foreground, for `stt qwen serve`.
    readonly serve: Effect.Effect<void, QwenError>
    // Starts the model process unless it is already serving, so it loads while you speak.
    readonly ensureRunning: Effect.Effect<void, QwenError>
    // Stops the model process, freeing its memory.
    readonly stop: Effect.Effect<void, QwenError>
    // PCM at SAMPLE_RATE to text, waiting for the server to come up if it is still loading.
    readonly transcribe: (pcm: Uint8Array) => Effect.Effect<string, QwenError>
  }
>()("stt/qwen/Qwen") {
  static readonly layer = Layer.effect(
    Qwen,
    Effect.gen(function* () {
      const fs = yield* FileSystem.FileSystem
      const spawner = yield* ChildProcessSpawner.ChildProcessSpawner
      const lock = yield* Semaphore.make(1)
      const pidPath = join(runtimeDirectory(), "qwen.pid")
      // The model's process group, whether this server started it or adopted it from an earlier one.
      let pid: number | null = null

      const alive = (id: number) => {
        try {
          process.kill(id, 0)
          return true
        } catch {
          return false
        }
      }

      // SIGTERM to the model's process group, or to the process alone if it left the group.
      const terminate = (id: number) => {
        try {
          process.kill(-id, "SIGTERM")
          return
        } catch {
          process.kill(id, "SIGTERM")
        }
      }

      // Records who owns the model, so a command that merely exits leaves it alone.
      const claim = (model: number) => fs.writeFileString(pidPath, JSON.stringify({ model, server: process.pid }))

      // The model's pid when this process started it, or when the server that did is gone; another live
      // server's model is left alone, so a dictation exiting cannot stop the service's model.
      const recordedPid = Effect.gen(function* () {
        const text = yield* fs.readFileString(pidPath).pipe(Effect.orElseSucceed(() => ""))
        const recorded = Schema.decodeOption(ModelPid)(text)
        if (Option.isNone(recorded)) {
          return null
        }
        const { model, server } = recorded.value
        if (server === process.pid || !alive(server)) {
          return model
        }
        return null
      })

      const stop = Effect.gen(function* () {
        // A model an earlier server started is only known from its pid file.
        const running = pid ?? (yield* recordedPid)
        pid = null
        if (running === null) {
          return
        }
        if (alive(running)) {
          // uv runs the python model in the group it leads, so one signal stops both.
          terminate(running)
          for (let attempt = 0; attempt < 50 && alive(running); attempt++) {
            yield* Effect.sleep("100 millis")
          }
        }
        yield* fs.remove(pidPath).pipe(Effect.ignore)
      })
      yield* Effect.addFinalizer(() => stop)

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
        return join(cacheDirectory(), "qwen.py")
      }).pipe(Effect.mapError((cause) => new QwenError({ message: `could not write the qwen scripts: ${cause.message}` })))

      const runUv = Effect.fnUntraced(function* (args: ReadonlyArray<string>) {
        const script = yield* writeScripts
        const command = ChildProcess.make("uv", ["run", "--quiet", "--script", script, ...args], { stdin: "ignore", stdout: "inherit", stderr: "inherit" })
        const code = yield* spawner
          .exitCode(command)
          .pipe(Effect.mapError((cause) => new QwenError({ message: `could not start uv; is it installed? (${cause.message})` })))
        if (code !== 0) {
          return yield* new QwenError({ message: `uv ${args.join(" ")} exited with ${code}` })
        }
      })

      // Adopts a model an earlier server left behind, so this one can still stop it.
      const adopt = Effect.gen(function* () {
        if (pid !== null) {
          return
        }
        const recorded = yield* recordedPid
        if (recorded === null) {
          return
        }
        pid = recorded
        yield* claim(recorded).pipe(Effect.ignore)
      })

      const ensureRunning = Effect.gen(function* () {
        if (yield* isListening(socketPath())) {
          yield* adopt
          return
        }
        if (pid !== null && alive(pid)) return
        pid = null
        yield* fs.makeDirectory(runtimeDirectory(), { recursive: true }).pipe(Effect.mapError((cause) => new QwenError({ message: cause.message })))
        const script = yield* writeScripts
        const spawned = yield* Effect.try({
          try: () =>
            Bun.spawn(["uv", "run", "--quiet", "--script", script, socketPath()], { detached: true, stdin: "ignore", stdout: "inherit", stderr: "inherit" }),
          catch: (cause) => new QwenError({ message: `could not start uv: ${String(cause)}` }),
        })
        pid = spawned.pid
        yield* claim(spawned.pid).pipe(Effect.mapError((cause) => new QwenError({ message: `could not record the model's pid: ${cause.message}` })))
      }).pipe((effect) => lock.withPermit(effect))

      const explainNotListening = Effect.fnUntraced(function* (error: NotListening) {
        if (pid !== null && alive(pid)) {
          return yield* error
        }
        return yield* new QwenError({ message: `The qwen process stopped; ${LOGS_HINT}` })
      })

      const transcribe: (pcm: Uint8Array) => Effect.Effect<string, QwenError> = Effect.fn("Qwen.transcribe")(function* (pcm: Uint8Array) {
        // The server takes an empty request for a readiness probe.
        if (pcm.length === 0) {
          return ""
        }
        const reply = yield* exchange(socketPath(), pcm).pipe(
          Effect.catchTags({
            SocketError: (error) => Effect.fail(new QwenError({ message: `The qwen server hung up: ${error.message}; ${LOGS_HINT}` })),
            NotListening: explainNotListening,
          }),
          Effect.retry({ schedule: Schedule.spaced("100 millis"), while: (error) => error._tag === "NotListening" }),
          Effect.timeoutOrElse({
            duration: START_TIMEOUT,
            orElse: () => Effect.fail(new QwenError({ message: `The qwen server did not start in time; ${LOGS_HINT}` })),
          }),
          Effect.catchTag("NotListening", (error) => Effect.fail(new QwenError({ message: error.message })))
        )
        return yield* parseReply(reply)
      })

      return Qwen.of({
        prepare: runUv(["--prepare"]),
        serve: fs.makeDirectory(runtimeDirectory(), { recursive: true }).pipe(
          Effect.mapError((cause) => new QwenError({ message: cause.message })),
          Effect.andThen(runUv([socketPath()]))
        ),
        ensureRunning,
        stop: stop.pipe((effect) => lock.withPermit(effect)),
        transcribe,
      })
    })
  )
}

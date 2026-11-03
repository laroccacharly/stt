import { Context, Effect, Layer, Option, Schema, Stream } from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/process"

export class PasteError extends Schema.TaggedError<PasteError>()("PasteError", {
  message: Schema.String,
}) {}

// Shift+Insert pastes in terminals and GUI apps alike (Omarchy's universal paste).
const SHIFT_INSERT = ["-M", "shift", "-k", "Insert", "-m", "shift"]

// Inserts text into the focused window through the clipboard, overwriting it.
export class Paster extends Context.Service<
  Paster,
  {
    // `display` is the Wayland socket of the session to paste into, when it is not this process's own.
    readonly paste: (text: string, display: Option.Option<string>) => Effect.Effect<void, PasteError>
  }
>()("stt/desktop/Paster") {
  static readonly layer = Layer.effect(
    Paster,
    Effect.gen(function* () {
      const spawner = yield* ChildProcessSpawner.ChildProcessSpawner

      const run = Effect.fnUntraced(function* (command: ChildProcess.Command, name: string) {
        const [stderr, code] = yield* Effect.scoped(
          Effect.gen(function* () {
            const handle = yield* spawner.spawn(command)
            return yield* Effect.all([Stream.mkString(Stream.decodeText(handle.stderr)), handle.exitCode], { concurrency: "unbounded" })
          })
        ).pipe(Effect.mapError((cause) => new PasteError({ message: `could not run ${name}: ${cause.message}` })))
        if (code !== 0) {
          return yield* new PasteError({ message: stderr.trim() || `${name} failed; check the focused window` })
        }
      })

      const paste: (text: string, display: Option.Option<string>) => Effect.Effect<void, PasteError> = Effect.fn("Paster.paste")(function* (
        text: string,
        display: Option.Option<string>
      ) {
        const env = Option.match(display, { onNone: () => ({}), onSome: (name) => ({ env: { WAYLAND_DISPLAY: name }, extendEnv: true }) })
        // wl-copy forks a server that keeps the clipboard, so its output is not piped: reading it would wait for that server.
        // Not detached: a detached child's process group is killed once it exits, and with it that server.
        const copy = ChildProcess.make("wl-copy", [], {
          stdin: Stream.make(new TextEncoder().encode(text)),
          stdout: "ignore",
          stderr: "ignore",
          detached: false,
          ...env,
        })
        const copied = yield* spawner.exitCode(copy).pipe(Effect.mapError((cause) => new PasteError({ message: `could not run wl-copy: ${cause.message}` })))
        if (copied !== 0) {
          return yield* new PasteError({ message: `wl-copy exited with ${copied}` })
        }
        yield* run(ChildProcess.make("wtype", SHIFT_INSERT, { stdin: "ignore", stdout: "ignore", stderr: "pipe", ...env }), "wtype")
      })
      return Paster.of({ paste })
    })
  )
}

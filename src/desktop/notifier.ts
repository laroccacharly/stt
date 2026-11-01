import { Context, Effect, Layer } from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/process"

// Notifications share one id so each replaces the previous.
const NOTIFICATION_ID = "47511"

// Desktop notifications through notify-send. Failing to notify is logged, never raised.
export class Notifier extends Context.Service<
  Notifier,
  {
    readonly notify: (summary: string, body: string, timeoutMs?: number) => Effect.Effect<void>
  }
>()("stt/desktop/Notifier") {
  static readonly layer = Layer.effect(
    Notifier,
    Effect.gen(function* () {
      const spawner = yield* ChildProcessSpawner.ChildProcessSpawner
      const notify: (summary: string, body: string, timeoutMs?: number) => Effect.Effect<void> = Effect.fn("Notifier.notify")(function* (summary: string, body: string, timeoutMs = 5000) {
        yield* Effect.logInfo(`notify: ${summary}: ${body}`)
        const command = ChildProcess.make("notify-send", ["--app-name=stt", `--replace-id=${NOTIFICATION_ID}`, `--expire-time=${timeoutMs}`, summary, body], {
          stdout: "ignore",
          stderr: "ignore",
        })
        yield* spawner.exitCode(command).pipe(Effect.catch((error) => Effect.logWarning("notification failed", error)))
      })
      return Notifier.of({ notify })
    })
  )
}

// Notifies an error before failing with it: the End binding and the background recorder have no terminal.
export const notifyingErrors = <A, E extends { readonly message: string }, R>(effect: Effect.Effect<A, E, R>): Effect.Effect<A, E, R | Notifier> =>
  effect.pipe(Effect.tapError((error) => Notifier.use((notifier) => notifier.notify("STT error", error.message))))

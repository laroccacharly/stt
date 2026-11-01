import { Deferred, Effect, Exit } from "effect"

// `stt toggle` stops a recording with SIGUSR2.
const STOP_SIGNAL = "SIGUSR2"

// Completes on the first stop request. Listens until the scope closes; call it before anything slow, so an early request is not fatal.
export const stopRequested = Effect.acquireRelease(
  Effect.sync(() => {
    const requested = Deferred.makeUnsafe<void>()
    const onSignal = () => {
      Deferred.doneUnsafe(requested, Exit.void)
    }
    process.on(STOP_SIGNAL, onSignal)
    return { requested, onSignal }
  }),
  ({ onSignal }) => Effect.sync(() => process.off(STOP_SIGNAL, onSignal))
).pipe(Effect.map(({ requested }) => Deferred.await(requested)))

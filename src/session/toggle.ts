import { spawn } from "node:child_process"
import { closeSync, mkdirSync, openSync, rmSync } from "node:fs"
import { join } from "node:path"
import { Effect, Option, Schedule, Schema } from "effect"
import { cliScript, runtimeDirectory } from "../paths.ts"
import { pidFilePath, recordingPid } from "./pid-file.ts"

export class ToggleError extends Schema.TaggedError<ToggleError>()("ToggleError", {
  message: Schema.String,
}) {}

export const logPath = (): string => join(runtimeDirectory(), "stt.log")

// `stt record` in its own process group, logging to stt.log, so it outlives this command. Resolves to whether it has exited yet.
const startRecorder = Effect.try({
  try: () => {
    mkdirSync(runtimeDirectory(), { recursive: true })
    rmSync(pidFilePath(), { force: true })
    const log = openSync(logPath(), "w")
    const recorder = spawn(process.execPath, [cliScript, "record"], { detached: true, stdio: ["ignore", log, log] })
    closeSync(log)
    const exited = { value: false }
    recorder.on("exit", () => {
      exited.value = true
    })
    recorder.unref()
    return () => exited.value
  },
  catch: (cause) => new ToggleError({ message: `failed to start the recorder: ${String(cause)}` }),
})

// Until the recorder holds the PID file, or has already exited: then it has notified its own error.
const recorderStarted = (hasExited: () => boolean) => Option.isSome(recordingPid()) || hasExited()

const awaitRecorder = (hasExited: () => boolean) =>
  Effect.suspend(() => {
    if (recorderStarted(hasExited)) {
      return Effect.void
    }
    return Effect.fail("starting")
  }).pipe(
    Effect.retry({ schedule: Schedule.spaced("50 millis"), times: 100 }),
    Effect.mapError(() => new ToggleError({ message: `STT did not start; inspect ${logPath()}` }))
  )

// Starts a dictation, or stops the running one so it is transcribed.
export const toggle = Effect.gen(function* () {
  const running = recordingPid()
  if (Option.isSome(running)) {
    process.kill(running.value, "SIGUSR2")
    return
  }
  const hasExited = yield* startRecorder
  yield* awaitRecorder(hasExited)
})

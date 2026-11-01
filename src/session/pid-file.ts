import { mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { Effect, Option, Schema } from "effect"
import { cliScript, runtimeDirectory } from "../paths.ts"

export class PidFileError extends Schema.TaggedError<PidFileError>()("PidFileError", {
  message: Schema.String,
}) {}

export const pidFilePath = (): string => join(runtimeDirectory(), "recording.pid")

const readOrNone = (path: string) => Option.liftThrowable(() => readFileSync(path, "utf8"))()

// Whether process `pid` is this CLI's recorder, not whatever reused the PID of one that died.
const isRecorder = (pid: number) =>
  readOrNone(`/proc/${pid}/cmdline`).pipe(
    Option.map((cmdline) => cmdline.split("\0")),
    Option.exists((args) => args.includes(cliScript) && args.includes("record"))
  )

// The PID of the running recorder, ignoring a stale file.
export const recordingPid = (): Option.Option<number> =>
  readOrNone(pidFilePath()).pipe(
    Option.map((text) => Number.parseInt(text.trim(), 10)),
    Option.filter((pid) => Number.isInteger(pid) && isRecorder(pid))
  )

// Claims the PID file for this process until the scope closes, however the session ends.
export const holdPidFile = Effect.acquireRelease(
  Effect.try({
    try: () => {
      mkdirSync(runtimeDirectory(), { recursive: true })
      writeFileSync(pidFilePath(), String(process.pid), { flag: "wx" })
    },
    catch: (cause) => new PidFileError({ message: `another recording is running, or ${pidFilePath()} is stale: ${String(cause)}` }),
  }),
  () => Effect.sync(() => rmSync(pidFilePath(), { force: true }))
)

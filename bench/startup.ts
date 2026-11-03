#!/usr/bin/env bun
// Time from "End pressed" to "microphone capturing", without the key binding.
//
// Runs `stt toggle` exactly as Hyprland does, then watches /proc to timestamp each milestone:
//   toggle exit   the `stt toggle` process has returned
//   pw-record     stt-server spawned pw-record for the dictation
//   first audio   pw-record wrote its first bytes (recording has really started)
// Each run is then dropped with POST /v1/dictation/cancel: nothing is transcribed, pasted or saved.
//
// Usage: bun bench/startup.ts [runs=10]
// The overlay flashes on screen for each run.
import { readdirSync, readFileSync } from "node:fs"

const RUNS = Number(process.argv[2] ?? 10)
const SERVER = process.env["STT_SERVER_URL"] ?? "http://127.0.0.1:4318"
const TIMEOUT_MS = 15_000

const read = (path: string): string => {
  try {
    return readFileSync(path, "utf8")
  } catch {
    return ""
  }
}

const childrenOf = (pid: number): number[] => {
  try {
    return readdirSync(`/proc/${pid}/task`).flatMap((task) =>
      read(`/proc/${pid}/task/${task}/children`)
        .trim()
        .split(/\s+/)
        .filter((id) => id !== "")
        .map(Number)
    )
  } catch {
    return []
  }
}

const pwRecordOf = (pid: number) => childrenOf(pid).find((child) => read(`/proc/${child}/comm`).trim() === "pw-record") ?? 0
const bytesWritten = (pid: number) => Number(/wchar:\s+(\d+)/.exec(read(`/proc/${pid}/io`))?.[1] ?? 0)

const serverPid = () => {
  const shown = Bun.spawnSync(["systemctl", "--user", "show", "--property=MainPID", "--value", "stt-server"]).stdout.toString().trim()
  const pid = Number(shown)
  if (!(pid > 0)) throw new Error("stt-server is not running; start it with `stt service install`")
  return pid
}

const state = async () => ((await (await fetch(`${SERVER}/v1/dictation`)).json()) as { state: string }).state

interface Milestones {
  toggleExit: number
  pwRecord: number
  firstAudio: number
}

const runOnce = async (server: number): Promise<Milestones> => {
  if ((await state()) !== "idle") throw new Error("a dictation is already running")
  const marks: Milestones = { toggleExit: 0, pwRecord: 0, firstAudio: 0 }
  const t0 = performance.now()
  const toggle = Bun.spawn(["stt", "toggle"], { stdout: "ignore", stderr: "inherit" })
  void toggle.exited.then(() => (marks.toggleExit = performance.now() - t0))

  let pw = 0
  while (marks.firstAudio === 0) {
    const now = performance.now() - t0
    if (now > TIMEOUT_MS) throw new Error(`timed out; marks so far: ${JSON.stringify(marks)}`)
    if (pw === 0) {
      pw = pwRecordOf(server)
      if (pw !== 0) marks.pwRecord = now
    } else if (bytesWritten(pw) > 0) {
      marks.firstAudio = now
    }
    await Bun.sleep(0.5)
  }
  await toggle.exited
  await fetch(`${SERVER}/v1/dictation/cancel`, { method: "POST" })
  return marks
}

const stats = (values: number[]) => {
  const sorted = [...values].sort((a, b) => a - b)
  const at = (q: number) => sorted[Math.min(sorted.length - 1, Math.floor(q * sorted.length))] ?? Number.NaN
  return [at(0), at(0.5), at(0.9), at(1)]
}

const server = serverPid()
const results: Milestones[] = []
for (let i = 0; i < RUNS; i++) {
  const marks = await runOnce(server)
  results.push(marks)
  console.log(
    `run ${String(i + 1).padStart(2)}: pw-record ${marks.pwRecord.toFixed(0)} ms, first audio ${marks.firstAudio.toFixed(0)} ms, toggle exit ${marks.toggleExit.toFixed(0)} ms`
  )
  // Let the overlay fade out before the next press.
  await Bun.sleep(800)
}

console.log("\nmilestone       min    p50    p90    max   (ms since `stt toggle` spawned)")
for (const key of ["pwRecord", "firstAudio", "toggleExit"] as const) {
  console.log(
    `${key.padEnd(12)} ${stats(results.map((r) => r[key]))
      .map((v) => v.toFixed(0).padStart(6))
      .join(" ")}`
  )
}

import { expect, test } from "bun:test"
import { rmSync } from "node:fs"
import { homedir } from "node:os"
import { join } from "node:path"
import { FIXTURES, Sandbox } from "./sandbox.ts"

// Real models and the real OpenRouter API: run with `bun run test:live`.
const live = test.skipIf(process.env["LIVE"] !== "1")

const SPEECH = {
  "hello.pcm": "the quick brown fox jumps over the lazy dog",
  "quilter.pcm": "mister quilter is the apostle of the middle classes and we are glad to welcome his gospel",
}

const words = (text: string) =>
  text
    .toLowerCase()
    .replaceAll("mr.", "mister")
    .replaceAll(/[^a-z ]/gu, "")
    .split(/\s+/u)
    .filter((word) => word !== "")
    .join(" ")

const wavOf = (sandbox: Sandbox, fixture: string) => {
  const file = sandbox.path(`${fixture}.wav`)
  Bun.spawnSync(["ffmpeg", "-loglevel", "error", "-f", "s16le", "-ar", "16000", "-ac", "1", "-i", join(FIXTURES, fixture), file])
  return file
}

live(
  "phonon transcribes real speech",
  async () => {
    // The real uv, home and cache, so the model's environment and download are reused.
    using sandbox = new Sandbox({ HOME: homedir(), XDG_CACHE_HOME: process.env["XDG_CACHE_HOME"] ?? join(homedir(), ".cache") })
    rmSync(join(sandbox.bin, "uv"))
    await sandbox.startServer()
    try {
      for (const [fixture, expected] of Object.entries(SPEECH)) {
        const transcribed = await sandbox.stt("transcribe", wavOf(sandbox, fixture))
        expect(words(transcribed.stdout)).toBe(expected)
      }
      const socket = sandbox.path("runtime", "stt", "phonon.sock")
      const modelProcesses = () => Bun.spawnSync(["ps", "-eo", "pid=,args="]).stdout.toString().split("\n").filter((line) => line.includes(socket))
      expect(modelProcesses().length).toBeGreaterThan(0)
      await sandbox.stopServer()
      for (let attempt = 0; attempt < 100 && modelProcesses().length > 0; attempt++) await Bun.sleep(50)
      expect(modelProcesses()).toEqual([])
    } finally {
      await sandbox.stopServer()
    }
  },
  360_000
)

live(
  "OpenRouter transcribes real speech with the key from the keyring",
  async () => {
    using sandbox = new Sandbox()
    delete sandbox.env["OPENROUTER_API_KEY"]
    await sandbox.stt("provider", "set", "openrouter")
    await sandbox.startServer()

    const transcribed = await sandbox.stt("transcribe", wavOf(sandbox, "hello.pcm"))

    expect(transcribed.stderr).toBe("")
    expect(words(transcribed.stdout)).toBe(SPEECH["hello.pcm"])
    await sandbox.stopServer()
  },
  60_000
)

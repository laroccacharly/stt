import { expect, test } from "bun:test"
import { existsSync, rmSync } from "node:fs"
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

const waitFor = async (path: string, timeoutMs: number) => {
  const deadline = Date.now() + timeoutMs
  while (!existsSync(path)) {
    if (Date.now() > deadline) {
      throw new Error(`${path} did not appear`)
    }
    await Bun.sleep(200)
  }
}

live(
  "phonon transcribes real speech",
  async () => {
    // The real uv, home and cache, so the model's environment and download are reused.
    using sandbox = new Sandbox({ HOME: homedir(), XDG_CACHE_HOME: process.env["XDG_CACHE_HOME"] ?? join(homedir(), ".cache") })
    rmSync(join(sandbox.bin, "uv"))
    const server = Bun.spawn(["bun", join(import.meta.dir, "..", "src", "cli", "index.ts"), "phonon", "serve"], { env: sandbox.env, stdout: "inherit", stderr: "inherit" })
    try {
      await waitFor(sandbox.path("runtime", "stt", "phonon.sock"), 300_000)
      for (const [fixture, expected] of Object.entries(SPEECH)) {
        const transcribed = await sandbox.stt("transcribe", wavOf(sandbox, fixture))
        expect(words(transcribed.stdout)).toBe(expected)
      }
    } finally {
      server.kill()
      await server.exited
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

    const transcribed = await sandbox.stt("transcribe", wavOf(sandbox, "hello.pcm"))

    expect(transcribed.stderr).toBe("")
    expect(words(transcribed.stdout)).toBe(SPEECH["hello.pcm"])
  },
  60_000
)

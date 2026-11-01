import { expect, test } from "bun:test"
import { Effect } from "effect"
import { SttClient } from "../client/index.ts"
import { fakeOpenRouter, Sandbox } from "./sandbox.ts"

const waitForServer = async () => {
  for (let attempt = 0; attempt < 100; attempt++) {
    try { await fetch("http://127.0.0.1:4318/"); return true } catch { await Bun.sleep(50) }
  }
  return false
}

test("client sends browser audio through the server and transcription pipeline", async () => {
  using upstream = fakeOpenRouter(200, { text: "browser speech" })
  using sandbox = new Sandbox({ STT_OPENROUTER_URL: upstream.url })
  await sandbox.stt("provider", "set", "openrouter")
  // Run the actual CLI in its isolated environment so its config and credentials are used.
  const process = Bun.spawn(["bun", "src/cli/index.ts", "serve"], { env: sandbox.env, stdout: "ignore", stderr: "pipe" })
  try {
    expect(await waitForServer()).toBe(true)
    const encoded = Bun.spawnSync(["ffmpeg", "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=0.1", "-c:a", "libopus", "-f", "webm", "pipe:1"])
    expect(encoded.exitCode).toBe(0)
    const data = Uint8Array.from(encoded.stdout).buffer
    const text = await Effect.runPromise(Effect.gen(function* () {
      return yield* (yield* SttClient).transcribe({ data, contentType: "audio/webm" })
    }).pipe(Effect.provide(SttClient.layer("http://127.0.0.1:4318"))))
    expect(text).toBe("browser speech")
    expect(upstream.requests).toHaveLength(1)
    expect(upstream.requests[0]?.body.input_audio.format).toBe("wav")
  } finally {
    process.kill()
    await process.exited
  }
})

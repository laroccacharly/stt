import { expect, test } from "bun:test"
import { existsSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { Effect } from "effect"
import { SttClient } from "../client/index.ts"
import { fakeOpenRouter, Sandbox } from "./sandbox.ts"

test("client prepares, then sends browser audio through the server and transcription pipeline", async () => {
  using upstream = fakeOpenRouter(200, { text: "browser speech" })
  using sandbox = new Sandbox({ STT_OPENROUTER_URL: upstream.url })
  await sandbox.stt("provider", "set", "openrouter")
  // Runs the actual CLI in its isolated environment, so its config and credentials are used.
  const url = await sandbox.startServer()
  const client = SttClient.layer(url)

  await Effect.runPromise(Effect.gen(function* () {
    yield* (yield* SttClient).prepare
  }).pipe(Effect.provide(client)))
  expect(upstream.requests).toHaveLength(0)

  const encoded = Bun.spawnSync(["ffmpeg", "-loglevel", "error", "-f", "lavfi", "-i", "sine=frequency=440:duration=0.1", "-c:a", "libopus", "-f", "webm", "pipe:1"])
  expect(encoded.exitCode).toBe(0)
  const data = Uint8Array.from(encoded.stdout).buffer
  const transcript = await Effect.runPromise(Effect.gen(function* () {
    return yield* (yield* SttClient).transcribeDetailed({ data, contentType: "audio/webm" })
  }).pipe(Effect.provide(client)))
  expect(transcript).toEqual({ model: "microsoft/mai-transcribe-2", provider: "openrouter", text: "browser speech" })
  expect(upstream.requests).toHaveLength(1)
  expect(upstream.requests[0]?.body.input_audio.format).toBe("wav")
})

test("uploads longer than a short recording reach the decoder", async () => {
  using upstream = fakeOpenRouter(200, { text: "unused" })
  using sandbox = new Sandbox({ STT_OPENROUTER_URL: upstream.url })
  await sandbox.stt("provider", "set", "openrouter")
  const url = await sandbox.startServer()

  // 26 MB of noise is not decodable audio, but it must fail in ffmpeg rather than be rejected for its size.
  const response = await fetch(`${url}/v1/transcribe`, {
    body: new Uint8Array(26 * 1024 * 1024),
    headers: { "content-type": "audio/webm" },
    method: "POST",
  })

  expect(response.status).toBe(502)
  expect(await response.json()).toMatchObject({ message: expect.any(String) })
})

test("a model an earlier server left behind is stopped when OpenRouter is used", async () => {
  using upstream = fakeOpenRouter(200, { text: "remote" })
  using sandbox = new Sandbox({ STT_OPENROUTER_URL: upstream.url })
  await sandbox.stt("provider", "set", "openrouter")
  // A detached process, like the one uv runs, recorded the way a killed server leaves it.
  const model = Bun.spawn(["sleep", "60"], { detached: true, stdin: "ignore", stdout: "ignore", stderr: "ignore" })
  // The server that started it is gone, as after a SIGKILL.
  writeFileSync(join(sandbox.path("runtime", "stt"), "phonon.pid"), JSON.stringify({ model: model.pid, server: 2_147_483_647 }))
  const url = await sandbox.startServer()
  try {
    const file = sandbox.path("hello.wav")
    Bun.spawnSync(["ffmpeg", "-loglevel", "error", "-f", "s16le", "-ar", "16000", "-ac", "1", "-i", join(import.meta.dir, "fixtures", "hello.pcm"), file])
    const response = await fetch(`${url}/v1/transcribe`, { body: new Uint8Array(await Bun.file(file).arrayBuffer()), headers: { "content-type": "audio/wav" }, method: "POST" })
    expect(await response.json()).toMatchObject({ text: "remote" })
    expect(await Promise.race([model.exited.then(() => true), Bun.sleep(3_000).then(() => false)])).toBe(true)
  } finally {
    model.kill()
  }
})

test("a client leaves a model another server owns alone", async () => {
  using upstream = fakeOpenRouter(200, { text: "remote" })
  using sandbox = new Sandbox({ STT_OPENROUTER_URL: upstream.url })
  await sandbox.stt("provider", "set", "openrouter")
  const spawn = (args: string[]) => Bun.spawn(args, { detached: true, stdin: "ignore", stdout: "ignore", stderr: "ignore" })
  const model = spawn(["sleep", "60"])
  const owner = spawn(["sleep", "60"])
  const pidFile = join(sandbox.path("runtime", "stt"), "phonon.pid")
  writeFileSync(pidFile, JSON.stringify({ model: model.pid, server: owner.pid }))
  await sandbox.startServer()
  try {
    const file = sandbox.path("hello.wav")
    Bun.spawnSync(["ffmpeg", "-loglevel", "error", "-f", "s16le", "-ar", "16000", "-ac", "1", "-i", join(import.meta.dir, "fixtures", "hello.pcm"), file])
    // Both the server and this client's own exit run the model's cleanup; neither may touch the owner's model.
    expect((await sandbox.stt("transcribe", file)).stdout.trim()).toBe("remote")
    expect(existsSync(pidFile)).toBe(true)
    expect(model.exitCode).toBeNull()
  } finally {
    model.kill()
    owner.kill()
  }
})

test("an adopted model is owned by the adopting server", async () => {
  using sandbox = new Sandbox()
  // A model an earlier server left behind: listening on the socket, owned by a server that is gone.
  const model = Bun.spawn(["sleep", "60"], { detached: true, stdin: "ignore", stdout: "ignore", stderr: "ignore" })
  const listener = Bun.listen({ unix: sandbox.path("runtime", "stt", "phonon.sock"), socket: { data() {} } })
  const pidFile = sandbox.path("runtime", "stt", "phonon.pid")
  const ownerOf = () => JSON.parse(readFileSync(pidFile, "utf8")) as { server: number }
  writeFileSync(pidFile, JSON.stringify({ model: model.pid, server: 2_147_483_647 }))
  const url = await sandbox.startServer()
  try {
    expect((await fetch(`${url}/v1/prepare`, { method: "POST" })).status).toBe(200)
    expect(ownerOf().server).not.toBe(2_147_483_647)
    // A command that only builds the layer and exits must leave the server's model alone.
    expect((await sandbox.stt("provider", "ls")).code).toBe(0)
    expect(model.exitCode).toBeNull()
    expect(ownerOf().server).not.toBe(2_147_483_647)
  } finally {
    listener.stop(true)
    model.kill()
  }
})

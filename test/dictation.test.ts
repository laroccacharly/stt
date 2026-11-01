import { expect, test } from "bun:test"
import { existsSync, rmSync, writeFileSync } from "node:fs"
import { createServer, type Server } from "node:net"
import { savedDictations } from "./history.ts"
import { CLI, fakeOpenRouter, KEY, Sandbox } from "./sandbox.ts"

// A sandbox using a fake OpenRouter; dispose of both with `using`.
const openRouterSandbox = async (status: number, reply: object) => {
  const server = fakeOpenRouter(status, reply)
  const sandbox = new Sandbox({ STT_OPENROUTER_URL: server.url })
  expect((await sandbox.stt("provider", "set", "openrouter")).code).toBe(0)
  return {
    sandbox,
    server,
    [Symbol.dispose]: () => {
      server[Symbol.dispose]()
      sandbox[Symbol.dispose]()
    },
  }
}

test.concurrent("toggle records, transcribes with OpenRouter, pastes and saves the dictation", async () => {
  using fixture = await openRouterSandbox(200, { text: " hello world " })
  const { sandbox, server } = fixture

  await sandbox.dictate()

  expect(sandbox.read("typed")).toBe("hello world|")
  expect(server.requests).toHaveLength(1)
  const [request] = server.requests
  expect(request?.path).toBe("/api/v1/audio/transcriptions")
  expect(request?.headers.get("authorization")).toBe(`Bearer ${KEY}`)
  expect(request?.headers.get("x-openrouter-title")).toBe("stt")
  expect(request?.body.model).toBe("microsoft/mai-transcribe-2")
  expect(request?.body.input_audio.format).toBe("wav")
  const audio = Buffer.from(request?.body.input_audio.data ?? "", "base64")
  expect(audio.subarray(0, 4).toString()).toBe("RIFF")
  const [saved] = savedDictations(sandbox)
  expect(saved).toMatchObject({ provider: "openrouter", model: "microsoft/mai-transcribe-2", transcript: "hello world" })
  expect(saved?.samples).toBe((audio.length - 44) / 2)
  expect(sandbox.read("notified")).toBe("")
})

test.concurrent("an OpenRouter error is notified and nothing is pasted", async () => {
  using fixture = await openRouterSandbox(401, { error: { message: "No auth credentials found" } })
  const { sandbox } = fixture

  await sandbox.dictate()

  expect(sandbox.read("notified")).toContain("STT error: OpenRouter answered 401: No auth credentials found")
  expect(sandbox.read("typed")).toBe("")
  expect(savedDictations(sandbox)).toHaveLength(0)
})

test.concurrent("silence pastes nothing but is still saved", async () => {
  using fixture = await openRouterSandbox(200, { text: "" })
  const { sandbox } = fixture

  await sandbox.dictate()

  expect(sandbox.read("typed")).toBe("")
  expect((await sandbox.stt("history", "ls")).stdout).toContain("openrouter (microsoft/mai-transcribe-2)  (no speech)")
})

test.concurrent("the microphone dying is notified", async () => {
  using fixture = await openRouterSandbox(200, { text: "unused" })
  const { sandbox } = fixture
  sandbox.script("pw-record", String.raw`printf '\x00\x10%.0s' $(seq 1600)`)

  expect((await sandbox.stt("toggle")).code).toBe(0)
  await sandbox.recorderExit()

  expect(sandbox.read("notified")).toContain("STT error: Microphone stopped unexpectedly (pw-record exited)")
})

test.concurrent("a paste failure is notified and the dictation still saved", async () => {
  using fixture = await openRouterSandbox(200, { text: "hello" })
  const { sandbox } = fixture
  sandbox.script("wtype", "echo 'Compositor does not support the virtual keyboard protocol' >&2; exit 1")

  await sandbox.dictate()

  expect(sandbox.read("notified")).toContain("STT typing failed: Compositor does not support the virtual keyboard protocol")
  expect(savedDictations(sandbox).map((saved) => saved.transcript)).toEqual(["hello"])
})

test.concurrent("a stale PID file does not block a new dictation", async () => {
  using fixture = await openRouterSandbox(200, { text: "fresh" })
  const { sandbox } = fixture
  writeFileSync(sandbox.pidFile, String(process.pid))

  await sandbox.dictate()

  expect(sandbox.read("typed")).toBe("fresh|")
})

test.concurrent("stt record works on its own, with no runtime directory yet", async () => {
  using fixture = await openRouterSandbox(200, { text: "direct" })
  const { sandbox } = fixture
  rmSync(sandbox.path("runtime", "stt"), { recursive: true, force: true })

  const recorder = Bun.spawn(["bun", CLI, "record"], { env: sandbox.env, stdout: "pipe", stderr: "pipe" })
  const deadline = Date.now() + 5000
  while (!existsSync(sandbox.pidFile)) {
    if (Date.now() > deadline) {
      throw new Error(`record did not claim the PID file:\n${sandbox.read("runtime/stt/stt.log")}`)
    }
    await Bun.sleep(25)
  }
  await Bun.sleep(300)
  process.kill(recorder.pid, "SIGUSR2")
  await recorder.exited

  expect(sandbox.read("typed")).toBe("direct|")
})

// A fake phonon server on the socket stt expects, speaking python/serve.py's protocol.
const fakePhonon = (path: string, reply: object) => {
  const received: number[] = []
  const server: Server = createServer({ allowHalfOpen: true }, (socket) => {
    let bytes = 0
    socket.on("data", (data) => {
      bytes += data.length
    })
    socket.on("end", () => {
      if (bytes === 0) {
        return socket.end()
      }
      received.push(bytes)
      socket.end(`${JSON.stringify(reply)}\n`)
    })
  })
  server.listen(path)
  return { received, [Symbol.dispose]: () => void server.close() }
}

test.concurrent("phonon transcribes through its socket without touching systemd when it is serving", async () => {
  using sandbox = new Sandbox()
  using phonon = fakePhonon(sandbox.path("runtime", "stt", "phonon.sock"), { text: " local words " })

  await sandbox.dictate()

  expect(sandbox.read("typed")).toBe("local words|")
  const [saved] = savedDictations(sandbox)
  expect(saved).toMatchObject({ provider: "phonon-2.5-en", model: "FermionResearch/Phonon-2.5-English", transcript: "local words" })
  expect(phonon.received).toEqual([(saved?.samples ?? 0) * 2])
  expect(sandbox.read("systemctl")).toBe("")
})

test.concurrent("a phonon error is notified", async () => {
  using sandbox = new Sandbox()
  using _phonon = fakePhonon(sandbox.path("runtime", "stt", "phonon.sock"), { error: "ValueError: bad audio" })

  await sandbox.dictate()

  expect(sandbox.read("notified")).toContain("STT error: Phonon: ValueError: bad audio")
})

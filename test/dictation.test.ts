import { expect, test } from "bun:test"
import { writeFileSync } from "node:fs"
import { createServer, type Server } from "node:net"
import { savedDictations } from "./history.ts"
import { fakeOpenRouter, KEY, Sandbox } from "./sandbox.ts"

// A sandbox using a fake OpenRouter; dispose of both with `using`.
const openRouterSandbox = async (status: number, reply: object) => {
  const server = fakeOpenRouter(status, reply)
  const sandbox = new Sandbox({ STT_OPENROUTER_URL: server.url })
  expect((await sandbox.stt("provider", "set", "openrouter")).code).toBe(0)
  await sandbox.startServer()
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
  await sandbox.dictationDone()

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

test.concurrent("stt transcribe sends the file's real MIME type", async () => {
  const contentTypes: Array<string | null> = []
  const server = Bun.serve({
    port: 0,
    fetch: async (request) => {
      contentTypes.push(request.headers.get("content-type"))
      await request.arrayBuffer()
      return Response.json({ text: "file words" })
    },
  })
  using _fake = { [Symbol.dispose]: () => void server.stop(true) }
  using sandbox = new Sandbox({ STT_SERVER_URL: server.url.origin })
  const file = sandbox.path("sample.wav")
  writeFileSync(file, "not really audio")

  const transcribed = await sandbox.stt("transcribe", file)

  expect(transcribed.code).toBe(0)
  expect(transcribed.stdout).toBe("file words\n")
  expect(contentTypes).toEqual([Bun.file(file).type])
  expect(Bun.file(file).type).toBe("audio/x-wav")
})

test.concurrent("toggle without the server notifies how to start it", async () => {
  using sandbox = new Sandbox({ STT_SERVER_URL: "http://127.0.0.1:9" })

  const toggled = await sandbox.stt("toggle")

  expect(toggled.code).toBe(1)
  expect(sandbox.read("notified")).toBe("STT error: stt-server is not reachable at http://127.0.0.1:9; start it with `stt service install`\n")
})

test.concurrent("End while transcribing does nothing, and the next press starts a new dictation", async () => {
  using fixture = await openRouterSandbox(200, { text: "again" })
  const { sandbox, server } = fixture
  // A slow paste keeps the dictation busy when End is pressed again.
  sandbox.script("wtype", `sleep 0.5; cat "$STT_TYPED.clipboard" >> "$STT_TYPED"; printf '|' >> "$STT_TYPED"`)

  expect((await sandbox.stt("toggle")).code).toBe(0)
  expect(await sandbox.dictationState()).toBe("recording")
  await Bun.sleep(300)
  expect((await sandbox.stt("toggle")).code).toBe(0)
  expect((await sandbox.stt("toggle")).code).toBe(0)
  expect(await sandbox.dictationState()).toBe("transcribing")
  await sandbox.dictationDone()
  await sandbox.dictate()

  expect(sandbox.read("typed")).toBe("again|again|")
  expect(server.requests).toHaveLength(2)
})

test.concurrent("cancel drops the recording without transcribing, pasting or saving it", async () => {
  using fixture = await openRouterSandbox(200, { text: "unwanted" })
  const { sandbox, server } = fixture

  expect((await sandbox.stt("toggle")).code).toBe(0)
  await Bun.sleep(300)
  const cancelled = await fetch(new URL("/v1/dictation/cancel", sandbox.env["STT_SERVER_URL"]), { method: "POST" })

  expect(await cancelled.json()).toEqual({ state: "idle" })
  expect(server.requests).toHaveLength(0)
  expect(sandbox.read("typed")).toBe("")
  expect(savedDictations(sandbox)).toHaveLength(0)
  expect(sandbox.read("notified")).toBe("")
})

test.concurrent("a web page cannot toggle or cancel the dictation", async () => {
  using fixture = await openRouterSandbox(200, { text: "unwanted" })
  const { sandbox } = fixture

  for (const path of ["/v1/dictation/toggle", "/v1/dictation/cancel"]) {
    const response = await fetch(new URL(path, sandbox.env["STT_SERVER_URL"]), {
      method: "POST",
      headers: { origin: "https://evil.example", "content-type": "text/plain" },
      body: "{}",
    })
    expect(response.status).toBe(403)
  }
  expect(await sandbox.dictationState()).toBe("idle")
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
  await sandbox.startServer()

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
  await sandbox.startServer()

  await sandbox.dictate()

  expect(sandbox.read("notified")).toContain("STT error: Phonon: ValueError: bad audio")
})

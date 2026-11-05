import { Database } from "bun:sqlite"
import { expect, test } from "bun:test"
import { existsSync, mkdirSync, readFileSync, writeFileSync } from "node:fs"
import { join } from "node:path"
import { savedDictations } from "./history.ts"
import { CLI, fakeOpenRouter, FIXTURES, Sandbox } from "./sandbox.ts"

const providerLines = async (sandbox: Sandbox) => (await sandbox.stt("provider", "ls")).stdout.trimEnd().split("\n")

test.concurrent("provider set is remembered and listed, and phonon defaults", async () => {
  using sandbox = new Sandbox()
  expect(await providerLines(sandbox)).toEqual([
    "* phonon-2.5-en   Phonon-2.5 English (local)",
    "  qwen3-asr-1.7b  Qwen3-ASR 1.7B (local CPU)",
    "  openrouter      OpenRouter (MAI-Transcribe 2)",
  ])

  const set = await sandbox.stt("provider", "set", "openrouter")

  expect(set.stdout).toBe("Using OpenRouter (MAI-Transcribe 2)\n")
  expect((await providerLines(sandbox))[2]).toStartWith("* openrouter")
  expect(JSON.parse(sandbox.read("config/stt/config.json"))).toEqual({ provider: "openrouter" })
})

test.concurrent("provider set rejects an unknown provider", async () => {
  using sandbox = new Sandbox()

  const set = await sandbox.stt("provider", "set", "cartesia")

  expect(set.code).not.toBe(0)
  expect(sandbox.read("config/stt/config.json")).toBe("")
})

test.concurrent("choosing phonon downloads the model, then providers only update the shared configuration", async () => {
  using sandbox = new Sandbox()
  expect((await sandbox.stt("provider", "set", "phonon-2.5-en")).code).toBe(0)
  expect(sandbox.read("uv")).toBe(`run --quiet --script ${sandbox.path("cache", "stt", "phonon.py")} --prepare\n`)
  expect((await sandbox.stt("provider", "set", "openrouter")).code).toBe(0)
  expect(sandbox.read("systemctl")).toBe("")
})

test.concurrent("choosing Qwen prepares its model and saves the provider", async () => {
  using sandbox = new Sandbox()
  expect((await sandbox.stt("provider", "set", "qwen3-asr-1.7b")).code).toBe(0)
  expect(sandbox.read("uv")).toBe(`run --quiet --script ${sandbox.path("cache", "stt", "qwen.py")} --prepare\n`)
  expect(JSON.parse(sandbox.read("config/stt/config.json"))).toEqual({ provider: "qwen3-asr-1.7b" })
})

test.concurrent("transcribe prints a file's transcript without pasting or saving it", async () => {
  using server = fakeOpenRouter(200, { text: " hello file " })
  using sandbox = new Sandbox({ STT_OPENROUTER_URL: server.url })
  await sandbox.stt("provider", "set", "openrouter")
  await sandbox.startServer()
  const file = sandbox.path("hello.wav")
  Bun.spawnSync(["ffmpeg", "-loglevel", "error", "-f", "s16le", "-ar", "16000", "-ac", "1", "-i", join(FIXTURES, "hello.pcm"), file])

  const transcribed = await sandbox.stt("transcribe", file)

  expect(transcribed.stdout).toBe("hello file\n")
  expect(server.requests).toHaveLength(1)
  expect(Buffer.from(server.requests[0]?.body.input_audio.data ?? "", "base64").length - 44).toBe(readFileSync(join(FIXTURES, "hello.pcm")).length)
  expect(sandbox.read("typed")).toBe("")
  expect(savedDictations(sandbox)).toHaveLength(0)
})

test.concurrent("a history from before the transcription time was recorded is migrated", async () => {
  using sandbox = new Sandbox()
  const path = sandbox.path("data", "stt", "history.sqlite")
  mkdirSync(join(path, ".."), { recursive: true })
  const database = new Database(path)
  database.run(readFileSync(join(import.meta.dir, "..", "migrations", "0001_create_transcriptions.sql"), "utf8"))
  database.run(
    "INSERT INTO transcriptions (provider, model, audio, transcript) VALUES ('onnxbark', 'ConwayResearch/Underdog-Bark-3.0-Clean', x'', 'old words')"
  )
  database.run("PRAGMA user_version = 1")
  database.close()

  const listed = await sandbox.stt("history", "ls")

  expect(listed.stdout).toMatch(/^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d {2}onnxbark \(ConwayResearch\/Underdog-Bark-3.0-Clean\) {2}old words\n$/u)
  const migrated = new Database(path, { readonly: true })
  expect(migrated.query("PRAGMA user_version").get()).toEqual({ user_version: 2 })
  expect(migrated.query("SELECT transcription_ms FROM transcriptions").get()).toEqual({ transcription_ms: 0 })
  migrated.close()
})

test.concurrent("install binds End in Hyprland and uninstall removes it", async () => {
  using sandbox = new Sandbox()
  const config = sandbox.path("home", ".config", "hypr", "hyprland.lua")
  writeFileSync(config, `require("hypr.bindings")`)

  expect((await sandbox.stt("install")).code).toBe(0)

  expect(readFileSync(config, "utf8")).toBe(`require("hypr.bindings")\nrequire("hypr.stt")\n`)
  const binding = readFileSync(sandbox.path("home", ".config", "hypr", "stt.lua"), "utf8")
  expect(binding).toBe(`o.bind("END", "Toggle speech to text", "stt toggle")\n`)

  expect((await sandbox.stt("uninstall")).code).toBe(0)

  expect(readFileSync(config, "utf8")).toBe(`require("hypr.bindings")\n`)
  expect(existsSync(sandbox.path("home", ".config", "hypr", "stt.lua"))).toBe(false)
})

test.concurrent("manages the API server as a systemd user service", async () => {
  using sandbox = new Sandbox()
  const unit = sandbox.path("config", "systemd", "user", "stt-server.service")

  expect((await sandbox.stt("service", "install")).code).toBe(0)
  expect(readFileSync(unit, "utf8")).toContain(`${CLI} serve`)
  expect(readFileSync(unit, "utf8")).toContain("WantedBy=default.target")
  expect(sandbox.read("systemctl")).toContain("--user enable --now stt-server.service")
  expect((await sandbox.stt("service", "reinstall")).code).toBe(0)
  expect(sandbox.read("systemctl")).toContain("--user restart stt-server.service")
  expect((await sandbox.stt("service", "status")).code).toBe(0)
  expect(sandbox.read("systemctl")).toContain("--user status --no-pager --lines=20 stt-server.service")
  expect((await sandbox.stt("service", "uninstall")).code).toBe(0)
  expect(existsSync(unit)).toBe(false)
  expect(sandbox.read("systemctl")).toContain("--user disable --now stt-server.service")
})

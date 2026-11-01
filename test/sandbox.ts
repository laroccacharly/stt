import { chmodSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"

export const CLI = join(import.meta.dir, "..", "src", "cli", "index.ts")
export const FIXTURES = join(import.meta.dir, "fixtures")
export const KEY = "test-key"

// Endless 100 ms chunks of a quiet square wave, like a real microphone.
const MICROPHONE = String.raw`while :; do printf '\x00\x10\x00\xf0%.0s' $(seq 800); sleep 0.1; done`

const FAKES: Readonly<Record<string, string>> = {
  "pw-record": MICROPHONE,
  // The paste keystroke; pasted text is what wl-copy received.
  wtype: "exit 0",
  "wl-copy": `cat >> "$STT_TYPED"; printf '|' >> "$STT_TYPED"`,
  "notify-send": `printf '%s: %s\\n' "$4" "$5" >> "$STT_NOTIFIED"`,
  systemctl: `echo "$*" >> "$STT_SYSTEMCTL"; [ "$2" = is-active ] && { echo inactive; exit 3; }; exit 0`,
  hyprctl: "exit 0",
  uv: `echo "$*" >> "$STT_UV"`,
}

export interface Run {
  readonly code: number
  readonly stdout: string
  readonly stderr: string
}

// An isolated home: private XDG directories, and fake tools first on PATH.
export class Sandbox {
  readonly dir = mkdtempSync(join(tmpdir(), "stt-test-"))
  readonly bin = join(this.dir, "bin")
  readonly env: Record<string, string>

  constructor(extraEnv: Readonly<Record<string, string>> = {}) {
    mkdirSync(this.bin)
    for (const [name, body] of Object.entries(FAKES)) {
      this.script(name, body)
    }
    for (const directory of ["runtime/stt", "config", "data", "cache", "home/.config/hypr"]) {
      mkdirSync(join(this.dir, directory), { recursive: true })
    }
    const { WAYLAND_DISPLAY: _wayland, ...inherited } = process.env
    this.env = {
      ...Object.fromEntries(Object.entries(inherited).filter((entry): entry is [string, string] => entry[1] !== undefined)),
      PATH: `${this.bin}:${process.env["PATH"] ?? ""}`,
      HOME: join(this.dir, "home"),
      XDG_RUNTIME_DIR: join(this.dir, "runtime"),
      XDG_CONFIG_HOME: join(this.dir, "config"),
      XDG_DATA_HOME: join(this.dir, "data"),
      XDG_CACHE_HOME: join(this.dir, "cache"),
      STT_TYPED: join(this.dir, "typed"),
      STT_NOTIFIED: join(this.dir, "notified"),
      STT_SYSTEMCTL: join(this.dir, "systemctl"),
      STT_UV: join(this.dir, "uv"),
      OPENROUTER_API_KEY: KEY,
      ...extraEnv,
    }
  }

  script(name: string, body: string): void {
    const path = join(this.bin, name)
    writeFileSync(path, `#!/bin/sh\n${body}\n`)
    chmodSync(path, 0o755)
  }

  path(...parts: ReadonlyArray<string>): string {
    return join(this.dir, ...parts)
  }

  read(name: string): string {
    const path = this.path(name)
    if (!existsSync(path)) {
      return ""
    }
    return readFileSync(path, "utf8")
  }

  async stt(...args: ReadonlyArray<string>): Promise<Run> {
    const child = Bun.spawn(["bun", CLI, ...args], { env: this.env, stdin: "ignore", stdout: "pipe", stderr: "pipe" })
    const [stdout, stderr, code] = await Promise.all([new Response(child.stdout).text(), new Response(child.stderr).text(), child.exited])
    return { code, stdout, stderr }
  }

  get pidFile(): string {
    return this.path("runtime", "stt", "recording.pid")
  }

  // Starts a dictation, lets it record for `ms`, stops it and waits for the recorder to finish.
  async dictate(ms = 400): Promise<void> {
    const started = await this.stt("toggle")
    if (started.code !== 0) {
      throw new Error(`toggle failed: ${started.stderr}`)
    }
    await Bun.sleep(ms)
    await this.stt("toggle")
    await this.recorderExit()
  }

  async recorderExit(timeoutMs = 8000): Promise<void> {
    const deadline = Date.now() + timeoutMs
    while (existsSync(this.pidFile)) {
      if (Date.now() > deadline) {
        throw new Error(`the recorder did not finish:\n${this.read("runtime/stt/stt.log")}`)
      }
      await Bun.sleep(25)
    }
  }

  [Symbol.dispose](): void {
    rmSync(this.dir, { recursive: true, force: true })
  }
}

// What stt posts to OpenRouter.
export interface TranscriptionBody {
  readonly model: string
  readonly input_audio: { readonly data: string; readonly format: string }
}

export interface Request {
  readonly path: string
  readonly headers: Headers
  readonly body: TranscriptionBody
}

// A fake OpenRouter that answers every request with `status` and `reply`, recording what it was sent.
export const fakeOpenRouter = (status: number, reply: object) => {
  const requests: Request[] = []
  const server = Bun.serve({
    port: 0,
    fetch: async (request) => {
      requests.push({ path: new URL(request.url).pathname, headers: request.headers, body: (await request.json()) as TranscriptionBody })
      return Response.json(reply, { status })
    },
  })
  return { url: server.url.origin, requests, [Symbol.dispose]: () => void server.stop(true) }
}

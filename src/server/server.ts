import { mkdtemp, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

const MAX_BYTES = 25 * 1024 * 1024

export const startServer = (transcribe: (path: string) => Promise<string>, port = 4318) => Bun.serve({
  hostname: "127.0.0.1",
  port,
  maxRequestBodySize: MAX_BYTES,
  async fetch(request) {
    const url = new URL(request.url)
    if (url.pathname !== "/v1/transcribe" || request.method !== "POST") return Response.json({ message: "Not found" }, { status: 404 })
    if (!request.headers.get("content-type")?.toLowerCase().startsWith("audio/")) return Response.json({ message: "Expected audio" }, { status: 415 })
    const audio = await request.arrayBuffer()
    if (audio.byteLength === 0) return Response.json({ message: "Recording is empty" }, { status: 400 })
    if (audio.byteLength > MAX_BYTES) return Response.json({ message: "Recording is too large" }, { status: 413 })
    const directory = await mkdtemp(join(tmpdir(), "stt-request-"))
    try {
      const path = join(directory, "audio")
      await writeFile(path, new Uint8Array(audio))
      return Response.json({ text: await transcribe(path) })
    } catch (error) {
      if (error instanceof Error) return Response.json({ message: error.message }, { status: 502 })
      return Response.json({ message: "Transcription failed" }, { status: 502 })
    } finally {
      await rm(directory, { recursive: true, force: true })
    }
  },
})

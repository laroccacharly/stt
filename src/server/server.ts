import { mkdtemp, rm, stat } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

// Long videos go through here, so the body is streamed to disk instead of held in memory.
const MAX_BYTES = 512 * 1024 * 1024

export interface TranscriptionApi {
  // Loads the local model or checks the key the next transcription needs.
  readonly prepare: () => Promise<void>
  // The transcript of the audio or video file at `path`, with the provider that made it.
  readonly transcribe: (path: string) => Promise<{ readonly model: string; readonly provider: string; readonly text: string }>
}

const notFound = () => Response.json({ message: "Not found" }, { status: 404 })

const failed = (error: Error) => Response.json({ message: error.message }, { status: 502 })

// The pipeline throws Errors; anything else gets a generic message.
const attempted = async (run: () => Promise<Response>): Promise<Response> => {
  try {
    return await run()
  } catch (error) {
    if (error instanceof Error) {
      return failed(error)
    }
    return failed(new Error("Transcription failed"))
  }
}

export const startServer = (api: TranscriptionApi, port = 4318) =>
  Bun.serve({
    hostname: "127.0.0.1",
    port,
    maxRequestBodySize: MAX_BYTES,
    async fetch(request) {
      const { pathname } = new URL(request.url)
      if (request.method !== "POST") {
        return notFound()
      }
      if (pathname === "/v1/prepare") {
        return attempted(async () => {
          await api.prepare()
          return Response.json({})
        })
      }
      if (pathname !== "/v1/transcribe") {
        return notFound()
      }
      if (!request.headers.get("content-type")?.toLowerCase().startsWith("audio/")) {
        return Response.json({ message: "Expected audio" }, { status: 415 })
      }
      const directory = await mkdtemp(join(tmpdir(), "stt-request-"))
      return attempted(async () => {
        const path = join(directory, "audio")
        await Bun.write(path, request)
        const { size } = await stat(path)
        if (size === 0) {
          return Response.json({ message: "Recording is empty" }, { status: 400 })
        }
        if (size > MAX_BYTES) {
          return Response.json({ message: "Recording is too large" }, { status: 413 })
        }
        return Response.json(await api.transcribe(path))
      }).finally(() => rm(directory, { recursive: true, force: true }))
    },
  })

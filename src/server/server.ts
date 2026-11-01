import { mkdtemp, rm, stat } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Cause, Effect } from "effect"
import { HttpRouter, HttpServerRequest, HttpServerResponse } from "effect/http"

// Long videos go through here, so the body is streamed to disk instead of held in memory.
const MAX_BYTES = 512 * 1024 * 1024

// Every pipeline error carries a message for the caller.
export interface ApiFailure {
  readonly message: string
}

export interface Transcript {
  readonly model: string
  readonly provider: string
  readonly text: string
}

export interface TranscriptionApi {
  // Loads the local model or checks the key the next transcription needs.
  readonly prepare: Effect.Effect<void, ApiFailure>
  // The transcript of the audio or video file at `path`, with the provider that made it.
  readonly transcribe: (path: string) => Effect.Effect<Transcript, ApiFailure>
}

const json = (body: object, status = 200) => HttpServerResponse.fromWeb(Response.json(body, { status }))

const failed = (failure: ApiFailure) => json({ message: failure.message }, 502)

// Expected failures keep their message; a defect gets a generic one.
const answered = (effect: Effect.Effect<HttpServerResponse.HttpServerResponse, ApiFailure>) =>
  effect.pipe(
    Effect.catch((failure) => Effect.succeed(failed(failure))),
    Effect.catchCause((cause) => Effect.logError("transcription failed", Cause.pretty(cause)).pipe(Effect.as(failed({ message: "Transcription failed" }))))
  )

const isMedia = (contentType: string) =>
  ["audio/", "video/", "application/octet-stream"].some((prefix) => contentType.startsWith(prefix))

// A private directory for one upload, removed with everything in it when the request ends.
const requestDirectory = Effect.acquireRelease(
  Effect.tryPromise({ try: () => mkdtemp(join(tmpdir(), "stt-request-")), catch: (cause) => ({ message: `could not store the upload: ${String(cause)}` }) }),
  (directory) => Effect.promise(() => rm(directory, { recursive: true, force: true }))
)

const transcribeRoute = (api: TranscriptionApi) =>
  HttpRouter.route("POST", "/v1/transcribe", (request) => {
    const contentType = request.headers["content-type"]?.toLowerCase() ?? ""
    if (!isMedia(contentType)) {
      return Effect.succeed(json({ message: "Expected audio or video" }, 415))
    }
    return answered(Effect.scoped(Effect.gen(function* () {
      const path = join(yield* requestDirectory, "audio")
      const web = yield* HttpServerRequest.toWeb(request).pipe(Effect.mapError((error) => ({ message: error.message })))
      yield* Effect.tryPromise({ try: () => Bun.write(path, web), catch: (cause) => ({ message: `could not store the upload: ${String(cause)}` }) })
      // Bun enforces maxRequestBodySize, so the body is already within the limit here.
      const { size } = yield* Effect.promise(() => stat(path))
      if (size === 0) {
        return json({ message: "Recording is empty" }, 400)
      }
      return json(yield* api.transcribe(path))
    })))
  })

const routes = (api: TranscriptionApi) => [
  HttpRouter.route("GET", "/health", Effect.succeed(json({ ok: true }))),
  HttpRouter.route("POST", "/v1/prepare", answered(api.prepare.pipe(Effect.as(json({}))))),
  transcribeRoute(api),
  HttpRouter.route("*", "*", Effect.succeed(json({ message: "Not found" }, 404))),
]

// The transcription API on 127.0.0.1; `stop` closes the listener and the router's runtime.
export const startServer = (api: TranscriptionApi, port = 4318) => {
  const http = HttpRouter.toWebHandler(HttpRouter.addAll(routes(api)), { disableLogger: true })
  const server = Bun.serve({
    hostname: "127.0.0.1",
    port,
    maxRequestBodySize: MAX_BYTES,
    // A long file or a cold model load keeps a request silent for minutes; Bun would close it after 10 s.
    idleTimeout: 0,
    fetch: (request) => http.handler(request),
  })
  return {
    url: server.url,
    stop: async () => {
      await server.stop(true)
      await http.dispose()
    },
  }
}

import { mkdtemp, rm, stat } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"
import { Cause, Effect, Option, Schema } from "effect"
import { HttpRouter, HttpServerRequest, HttpServerResponse } from "effect/http"
import type { DictationState } from "../session/dictation.ts"

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

// The dictation End toggles, which the server runs so a key press does not wait for a process to start.
export interface DictationApi {
  // `display` is the Wayland socket of the session that pressed End.
  readonly toggle: (display: Option.Option<string>) => Effect.Effect<DictationState>
  // Drops the dictation without pasting or saving it.
  readonly cancel: Effect.Effect<DictationState>
  readonly state: Effect.Effect<DictationState>
}

const ToggleRequest = Schema.Struct({ display: Schema.optional(Schema.String) })

const json = (body: object, status = 200) => HttpServerResponse.fromWeb(Response.json(body, { status }))

const failed = (failure: ApiFailure) => json({ message: failure.message }, 502)

// Expected failures keep their message; a defect gets a generic one.
const answered = (effect: Effect.Effect<HttpServerResponse.HttpServerResponse, ApiFailure>) =>
  effect.pipe(
    Effect.catch((failure) => Effect.succeed(failed(failure))),
    Effect.catchCause((cause) => Effect.logError("transcription failed", Cause.pretty(cause)).pipe(Effect.as(failed({ message: "Transcription failed" }))))
  )

const isMedia = (contentType: string) => ["audio/", "video/", "application/octet-stream"].some((prefix) => contentType.startsWith(prefix))

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
    return answered(
      Effect.scoped(
        Effect.gen(function* () {
          const path = join(yield* requestDirectory, "audio")
          const web = yield* HttpServerRequest.toWeb(request).pipe(Effect.mapError((error) => ({ message: error.message })))
          yield* Effect.tryPromise({ try: () => Bun.write(path, web), catch: (cause) => ({ message: `could not store the upload: ${String(cause)}` }) })
          // Bun enforces maxRequestBodySize, so the body is already within the limit here.
          const { size } = yield* Effect.promise(() => stat(path))
          if (size === 0) {
            return json({ message: "Recording is empty" }, 400)
          }
          return json(yield* api.transcribe(path))
        })
      )
    )
  })

// A browser sends Origin with every cross-origin POST, and a simple one skips the CORS preflight; stt toggle never sends it.
// Refusing it keeps a web page from turning the microphone on.
const fromCli =
  (handler: (request: HttpServerRequest.HttpServerRequest) => Effect.Effect<HttpServerResponse.HttpServerResponse>) =>
  (request: HttpServerRequest.HttpServerRequest) => {
    if (request.headers["origin"] !== undefined) {
      return Effect.succeed(json({ message: "Cross-origin requests are not allowed" }, 403))
    }
    return handler(request)
  }

const dictationRoutes = (dictation: DictationApi) => [
  HttpRouter.route("GET", "/v1/dictation", dictation.state.pipe(Effect.map((state) => json({ state })))),
  HttpRouter.route(
    "POST",
    "/v1/dictation/cancel",
    fromCli(() => dictation.cancel.pipe(Effect.map((state) => json({ state }))))
  ),
  HttpRouter.route(
    "POST",
    "/v1/dictation/toggle",
    fromCli((request) =>
      request.json.pipe(
        Effect.orElseSucceed(() => ({})),
        Effect.flatMap(Schema.decodeUnknownEffect(ToggleRequest)),
        Effect.flatMap(({ display }) => dictation.toggle(Option.fromUndefinedOr(display))),
        Effect.map((state) => json({ state })),
        Effect.orElseSucceed(() => json({ message: 'Expected {"display"?: string}' }, 400))
      )
    )
  ),
]

const routes = (api: TranscriptionApi, dictation: Option.Option<DictationApi>) => [
  ...Option.match(dictation, { onNone: () => [], onSome: dictationRoutes }),
  HttpRouter.route("GET", "/health", Effect.succeed(json({ ok: true }))),
  HttpRouter.route("POST", "/v1/prepare", answered(api.prepare.pipe(Effect.as(json({}))))),
  transcribeRoute(api),
  HttpRouter.route("*", "*", Effect.succeed(json({ message: "Not found" }, 404))),
]

// The transcription API on 127.0.0.1; `stop` closes the listener and the router's runtime.
export const startServer = (api: TranscriptionApi, port = 4318, dictation: Option.Option<DictationApi> = Option.none()) => {
  const http = HttpRouter.toWebHandler(HttpRouter.addAll(routes(api, dictation)), { disableLogger: true })
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

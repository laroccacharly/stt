import { Context, Effect, Layer, Option, Schema } from "effect"

export class SttClientError extends Schema.TaggedError<SttClientError>()("SttClientError", {
  message: Schema.String,
}) {}

const TranscriptSchema = Schema.Struct({
  model: Schema.optional(Schema.String),
  provider: Schema.optional(Schema.String),
  text: Schema.String,
})

/** A transcript, with the provider and model the server used for it when it reports them. */
export type Transcript = typeof TranscriptSchema.Type

const ServerErrorSchema = Schema.Struct({ message: Schema.String })

export interface AudioInput {
  readonly data: ArrayBuffer
  readonly contentType: string
}

export class SttClient extends Context.Service<SttClient, {
  /** Asks the server to start the local model or check the key the next transcription needs. */
  readonly prepare: Effect.Effect<void, SttClientError>
  readonly transcribe: (audio: AudioInput) => Effect.Effect<string, SttClientError>
  readonly transcribeDetailed: (audio: AudioInput) => Effect.Effect<Transcript, SttClientError>
}>()("stt/client/SttClient") {
  static layer(baseUrl: string) {
    const call = (path: string, init: RequestInit): Effect.Effect<Response, SttClientError> =>
      Effect.tryPromise({
        try: (signal) => fetch(new URL(path, baseUrl), { ...init, signal }),
        catch: (cause) => new SttClientError({ message: `Could not reach stt: ${String(cause)}` }),
      })

    const bodyOf = (response: Response) =>
      Effect.tryPromise({
        try: () => response.json(),
        catch: () => new SttClientError({ message: `Invalid stt response (HTTP ${response.status})` }),
      })

    // The server explains a failure in `{ message }`; a body it cannot parse leaves only the status.
    const failure = (response: Response): Effect.Effect<never, SttClientError> =>
      Effect.gen(function* failureEffect() {
        const body = yield* Effect.option(bodyOf(response))
        if (Option.isSome(body)) {
          const error = Schema.decodeUnknownOption(ServerErrorSchema)(body.value)
          if (error._tag === "Some") {
            return yield* new SttClientError({ message: error.value.message })
          }
        }
        return yield* new SttClientError({ message: `stt request failed (HTTP ${response.status})` })
      })

    const transcribeDetailed: (audio: AudioInput) => Effect.Effect<Transcript, SttClientError> = Effect.fn("SttClient.transcribeDetailed")(function* (audio: AudioInput) {
      const response = yield* call("/v1/transcribe", {
        body: audio.data,
        headers: { "content-type": audio.contentType },
        method: "POST",
      })
      if (!response.ok) {
        return yield* failure(response)
      }
      return yield* Schema.decodeUnknownEffect(TranscriptSchema)(yield* bodyOf(response)).pipe(
        Effect.mapError(() => new SttClientError({ message: "Invalid stt transcription response" }))
      )
    })

    const prepare: Effect.Effect<void, SttClientError> = call("/v1/prepare", { method: "POST" }).pipe(
      Effect.flatMap((response) => {
        if (response.ok) {
          return Effect.void
        }
        return failure(response)
      })
    )

    return Layer.succeed(SttClient)(SttClient.of({
      prepare,
      transcribe: (audio) => transcribeDetailed(audio).pipe(Effect.map((result) => result.text)),
      transcribeDetailed,
    }))
  }
}

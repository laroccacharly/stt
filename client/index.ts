import { Context, Effect, Layer, Schema } from "effect"

export class SttClientError extends Schema.TaggedError<SttClientError>()("SttClientError", {
  message: Schema.String,
}) {}

const Transcript = Schema.Struct({ text: Schema.String })
const ServerError = Schema.Struct({ message: Schema.String })

export interface AudioInput {
  readonly data: ArrayBuffer
  readonly contentType: string
}

export class SttClient extends Context.Service<SttClient, {
  readonly transcribe: (audio: AudioInput) => Effect.Effect<string, SttClientError>
}>()("stt/client/SttClient") {
  static layer(baseUrl: string) {
    return Layer.succeed(SttClient)(SttClient.of({
      transcribe: Effect.fn("SttClient.transcribe")(function* (audio: AudioInput) {
        const response = yield* Effect.tryPromise({
          try: (signal) => fetch(new URL("/v1/transcribe", baseUrl), {
            method: "POST",
            headers: { "content-type": audio.contentType },
            body: audio.data,
            signal,
          }),
          catch: (cause) => new SttClientError({ message: `Could not reach stt: ${String(cause)}` }),
        })
        const body = yield* Effect.tryPromise({
          try: () => response.json(),
          catch: () => new SttClientError({ message: `Invalid stt response (HTTP ${response.status})` }),
        })
        if (!response.ok) {
          const error = Schema.decodeUnknownOption(ServerError)(body)
          if (error._tag === "Some") return yield* new SttClientError({ message: error.value.message })
          return yield* new SttClientError({ message: `stt request failed (HTTP ${response.status})` })
        }
        const result = yield* Schema.decodeUnknownEffect(Transcript)(body).pipe(
          Effect.mapError(() => new SttClientError({ message: "Invalid stt transcription response" }))
        )
        return result.text
      }),
    }))
  }
}

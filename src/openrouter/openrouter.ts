import { Context, Effect, Layer, Option, Redacted, Schema } from "effect"
import { type CredentialsError, Keyring, secret } from "effect-lib/credentials"
import { FetchHttpClient, HttpClient, HttpClientRequest, type HttpClientResponse } from "effect/http"
import { wav } from "../audio/pcm.ts"
import { PROVIDERS } from "../provider/provider.ts"

const ORIGIN = "https://openrouter.ai"

// Shared with every app that declares OPENROUTER_API_KEY, so one login is enough.
export const ApiKey = secret("OPENROUTER_API_KEY", { label: "OpenRouter API key" })

export class OpenRouterError extends Schema.TaggedError<OpenRouterError>()("OpenRouterError", {
  message: Schema.String,
}) {}

const Transcript = Schema.Struct({ text: Schema.String })
const ErrorBody = Schema.fromJsonString(Schema.Struct({ error: Schema.Struct({ message: Schema.String }) }))

// The transcript of a 2xx response; otherwise fails with OpenRouter's error message when it sends one.
const decodeTranscript = Effect.fnUntraced(function* (response: HttpClientResponse.HttpClientResponse) {
  if (response.status < 200 || response.status >= 300) {
    const text = yield* response.text
    const message = Schema.decodeOption(ErrorBody)(text).pipe(Option.match({ onNone: () => text.slice(0, 500), onSome: (body) => body.error.message }))
    return yield* new OpenRouterError({ message: `OpenRouter answered ${response.status}: ${message}` })
  }
  const transcript = yield* response.json.pipe(Effect.flatMap(Schema.decodeUnknownEffect(Transcript)))
  return transcript.text.trim()
})

// MAI-Transcribe 2 through OpenRouter; STT_OPENROUTER_URL points it at a test server.
export class OpenRouter extends Context.Service<
  OpenRouter,
  {
    // Fails before recording when there is no key, rather than after.
    readonly checkKey: Effect.Effect<void, CredentialsError>
    // PCM at SAMPLE_RATE to text.
    readonly transcribe: (pcm: Uint8Array) => Effect.Effect<string, OpenRouterError | CredentialsError>
  }
>()("stt/openrouter/OpenRouter") {
  static readonly layer: Layer.Layer<OpenRouter, never, Keyring> = Layer.effect(
    OpenRouter,
    Effect.gen(function* () {
      const keyring = yield* Keyring
      const origin = process.env["STT_OPENROUTER_URL"] ?? ORIGIN
      const http = (yield* HttpClient.HttpClient).pipe(HttpClient.mapRequest(HttpClientRequest.prependUrl(origin)))
      const model = PROVIDERS.openrouter.model
      // Read on every request, so a rotated key is picked up without a restart.
      const apiKey = ApiKey.resolve.pipe(Effect.provideService(Keyring, keyring))

      const transcribe: (pcm: Uint8Array) => Effect.Effect<string, OpenRouterError | CredentialsError> = Effect.fn("OpenRouter.transcribe")(function* (
        pcm: Uint8Array
      ) {
        const key = yield* apiKey
        return yield* HttpClientRequest.post("/api/v1/audio/transcriptions").pipe(
          HttpClientRequest.bearerToken(Redacted.value(key)),
          HttpClientRequest.setHeaders({ "X-OpenRouter-Title": "stt" }),
          HttpClientRequest.bodyJsonUnsafe({
            model,
            input_audio: { data: Buffer.from(wav(pcm)).toString("base64"), format: "wav" },
            // Drop fillers and false starts: this is dictation, not a transcript.
            provider: { options: { azure: { enhancedMode: { modelOptions: { transcribeStyle: "clean" } } } } },
          }),
          http.execute,
          Effect.flatMap(decodeTranscript),
          Effect.catchTags({
            HttpClientError: (error) => Effect.fail(new OpenRouterError({ message: `could not transcribe the audio with ${model}: ${error.message}` })),
            SchemaError: (error) => Effect.fail(new OpenRouterError({ message: `unexpected answer from OpenRouter: ${error.message}` })),
          })
        )
      })

      return OpenRouter.of({ checkKey: Effect.asVoid(apiKey), transcribe })
    })
  ).pipe(Layer.provide(FetchHttpClient.layer))
}

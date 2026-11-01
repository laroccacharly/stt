import { Context, Duration, Effect, Layer, Option, Schema } from "effect"

export const SttClientErrorReason = Schema.Literals(["unreachable", "server", "invalid-response", "timeout"])
export type SttClientErrorReason = typeof SttClientErrorReason.Type

export class SttClientError extends Schema.TaggedError<SttClientError>()("SttClientError", {
  message: Schema.String,
  reason: SttClientErrorReason,
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

export interface SttClientOptions {
  /** How long each call may take; calls wait as long as the server works when it is not set. */
  readonly timeout?: Duration.Input
}

export class SttClient extends Context.Service<
  SttClient,
  {
    /** Asks the server to start the local model or check the key the next transcription needs. */
    readonly prepare: Effect.Effect<void, SttClientError>
    readonly transcribe: (audio: AudioInput) => Effect.Effect<string, SttClientError>
    readonly transcribeDetailed: (audio: AudioInput) => Effect.Effect<Transcript, SttClientError>
  }
>()("stt/client/SttClient") {
  static layer(baseUrl: string, options: SttClientOptions = {}) {
    // A long file or a cold model load can take many minutes, so only a caller that asks for a limit gets one.
    const limited = <A>(effect: Effect.Effect<A, SttClientError>): Effect.Effect<A, SttClientError> => {
      if (options.timeout === undefined) {
        return effect
      }
      const timeout = Duration.fromInputUnsafe(options.timeout)
      return effect.pipe(
        Effect.timeoutOrElse({
          duration: timeout,
          orElse: () =>
            Effect.fail(
              new SttClientError({
                message: `stt-server at ${baseUrl} did not answer within ${Duration.format(timeout)}`,
                reason: "timeout",
              })
            ),
        })
      )
    }

    const call = (path: string, init: RequestInit): Effect.Effect<Response, SttClientError> =>
      Effect.tryPromise({
        try: (signal) => fetch(new URL(path, baseUrl), { ...init, signal }),
        catch: () =>
          new SttClientError({
            message: `stt-server is not reachable at ${baseUrl}; start it with \`stt service install\``,
            reason: "unreachable",
          }),
      })

    // The body of a response that should carry JSON, or an invalid-response failure when it does not.
    const bodyOf = (response: Response): Effect.Effect<Schema.Json, SttClientError> =>
      Effect.tryPromise({
        try: () => response.json(),
        catch: () =>
          new SttClientError({
            message: `Invalid stt response (HTTP ${response.status})`,
            reason: "invalid-response",
          }),
      })

    // The server explains a failure in `{ message }`; a body it cannot parse leaves only the status.
    const failure = (response: Response): Effect.Effect<never, SttClientError> =>
      Effect.gen(function* failureEffect() {
        const body = yield* Effect.option(bodyOf(response))
        if (Option.isSome(body)) {
          const error = Schema.decodeUnknownOption(ServerErrorSchema)(body.value)
          if (Option.isSome(error)) {
            return yield* new SttClientError({ message: error.value.message, reason: "server" })
          }
        }
        return yield* new SttClientError({ message: `stt request failed (HTTP ${response.status})`, reason: "server" })
      })

    const transcribeDetailed: (audio: AudioInput) => Effect.Effect<Transcript, SttClientError> = Effect.fn("SttClient.transcribeDetailed")(function* (
      audio: AudioInput
    ) {
      const response = yield* call("/v1/transcribe", {
        body: audio.data,
        headers: { "content-type": audio.contentType },
        method: "POST",
      })
      if (!response.ok) {
        return yield* failure(response)
      }
      return yield* Schema.decodeUnknownEffect(TranscriptSchema)(yield* bodyOf(response)).pipe(
        Effect.mapError(() => new SttClientError({ message: "Invalid stt transcription response", reason: "invalid-response" }))
      )
    }, limited)

    const prepare: Effect.Effect<void, SttClientError> = Effect.gen(function* prepareEffect() {
      const response = yield* call("/v1/prepare", { method: "POST" })
      if (!response.ok) {
        return yield* failure(response)
      }
      // Nothing in a successful reply is needed; release its body so the connection is not held open.
      yield* Effect.promise(() => response.text()).pipe(Effect.ignore)
    }).pipe(limited)

    return Layer.succeed(SttClient)(
      SttClient.of({
        prepare,
        transcribe: (audio) => transcribeDetailed(audio).pipe(Effect.map((result) => result.text)),
        transcribeDetailed,
      })
    )
  }
}

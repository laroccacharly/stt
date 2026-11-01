import { expect, test } from "bun:test"
import { Effect } from "effect"
import { SttClient } from "../client/index.ts"

test("sends audio and returns the transcript", async () => {
  const server = Bun.serve({ port: 0, fetch: async (request) => {
    expect(new URL(request.url).pathname).toBe("/v1/transcribe")
    expect(request.headers.get("content-type")).toBe("audio/webm")
    expect([...new Uint8Array(await request.arrayBuffer())]).toEqual([1, 2])
    return Response.json({ text: "hello" })
  } })
  try {
    const client = SttClient.layer(server.url.href)
    const result = await Effect.runPromise(Effect.gen(function* () {
      return yield* (yield* SttClient).transcribe({ data: new Uint8Array([1, 2]).buffer, contentType: "audio/webm" })
    }).pipe(Effect.provide(client)))
    expect(result).toBe("hello")

    const detailed = await Effect.runPromise(Effect.gen(function* () {
      return yield* (yield* SttClient).transcribeDetailed({ data: new Uint8Array([1, 2]).buffer, contentType: "audio/webm" })
    }).pipe(Effect.provide(client)))
    expect(detailed).toEqual({ text: "hello" })
  } finally { await server.stop(true) }
})

test("prepares the server before recording", async () => {
  const requests: string[] = []
  const server = Bun.serve({ port: 0, fetch: (request) => {
    requests.push(`${request.method} ${new URL(request.url).pathname}`)
    return Response.json({})
  } })
  try {
    await Effect.runPromise(Effect.gen(function* () {
      yield* (yield* SttClient).prepare
    }).pipe(Effect.provide(SttClient.layer(server.url.href))))
    expect(requests).toEqual(["POST /v1/prepare"])
  } finally { await server.stop(true) }
})

test("reports the server's explanation of a failure", async () => {
  const server = Bun.serve({ port: 0, fetch: () => Response.json({ message: "Model unavailable" }, { status: 503 }) })
  try {
    const error = await Effect.runPromise(Effect.gen(function* () {
      return yield* (yield* SttClient).prepare
    }).pipe(Effect.provide(SttClient.layer(server.url.href)), Effect.flip))
    expect(error.message).toBe("Model unavailable")
    expect(error.reason).toBe("server")
  } finally { await server.stop(true) }
})

test("reports an unreachable server with an actionable message", async () => {
  const reservation = Bun.serve({ port: 0, fetch: () => new Response() })
  const url = reservation.url.href
  await reservation.stop(true)

  const error = await Effect.runPromise(Effect.gen(function* () {
    return yield* (yield* SttClient).prepare
  }).pipe(Effect.provide(SttClient.layer(url)), Effect.flip))

  expect(error.reason).toBe("unreachable")
  expect(error.message).toBe(`stt-server is not reachable at ${url}; start it with \`stt service install\``)
})

test("reports a server that does not answer in time", async () => {
  const server = Bun.serve({ port: 0, fetch: () => new Promise<Response>(() => {}) })
  try {
    const error = await Effect.runPromise(Effect.gen(function* () {
      return yield* (yield* SttClient).prepare
    }).pipe(Effect.provide(SttClient.layer(server.url.href, { timeout: "100 millis" })), Effect.flip))

    expect(error.reason).toBe("timeout")
    expect(error.message).toContain("did not answer within")
  } finally { await server.stop(true) }
})

test("reports a reply body it cannot decode", async () => {
  const server = Bun.serve({ port: 0, fetch: () => new Response("not json") })
  try {
    const error = await Effect.runPromise(Effect.gen(function* () {
      return yield* (yield* SttClient).transcribeDetailed({ data: new Uint8Array([1, 2]).buffer, contentType: "audio/webm" })
    }).pipe(Effect.provide(SttClient.layer(server.url.href)), Effect.flip))

    expect(error.reason).toBe("invalid-response")
  } finally { await server.stop(true) }
})

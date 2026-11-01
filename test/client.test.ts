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
    const result = await Effect.runPromise(Effect.gen(function* () {
      return yield* (yield* SttClient).transcribe({ data: new Uint8Array([1, 2]).buffer, contentType: "audio/webm" })
    }).pipe(Effect.provide(SttClient.layer(server.url.href))))
    expect(result).toBe("hello")
  } finally { await server.stop(true) }
})

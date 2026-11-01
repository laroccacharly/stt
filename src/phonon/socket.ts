import { createConnection } from "node:net"
import { Effect, Schema } from "effect"

// The server is not listening yet: it is loading, or not running.
export class NotListening extends Schema.TaggedError<NotListening>()("NotListening", {
  message: Schema.String,
}) {}

export class SocketError extends Schema.TaggedError<SocketError>()("SocketError", {
  message: Schema.String,
}) {}

// Sends PCM over the Unix socket, half-closes it, and reads the reply until the server closes (see python/serve.py).
export const exchange = (path: string, pcm: Uint8Array): Effect.Effect<string, NotListening | SocketError> =>
  Effect.callback<string, NotListening | SocketError>((resume) => {
    const socket = createConnection({ path })
    let connected = false
    let reply = ""
    socket.setEncoding("utf8")
    socket.on("connect", () => {
      connected = true
      socket.end(pcm)
    })
    socket.on("data", (data: string) => {
      reply += data
    })
    socket.on("end", () => resume(Effect.succeed(reply)))
    socket.on("error", (error) => {
      if (connected) {
        return resume(Effect.fail(new SocketError({ message: error.message })))
      }
      resume(Effect.fail(new NotListening({ message: error.message })))
    })
    return Effect.sync(() => socket.destroy())
  })

// Whether a server accepts connections on the socket, without sending it anything.
export const isListening = (path: string): Effect.Effect<boolean> =>
  Effect.callback<boolean>((resume) => {
    const socket = createConnection({ path })
    socket.on("connect", () => {
      socket.destroy()
      resume(Effect.succeed(true))
    })
    socket.on("error", () => resume(Effect.succeed(false)))
    return Effect.sync(() => socket.destroy())
  })

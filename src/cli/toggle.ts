// What End runs: asks stt-server to start or stop the dictation. It imports nothing heavy, so the key press costs one request.
import { spawn } from "node:child_process"
import { NOTIFICATION_ID } from "../desktop/notification-id.ts"

export const serverUrl = (): string => process.env["STT_SERVER_URL"] ?? "http://127.0.0.1:4318"

// Notifies and prints a failure: End has no terminal. Resolves once notify-send is done, so the notification is not lost.
const failed = (message: string): Promise<number> => {
  console.error(`stt: ${message}`)
  return new Promise((resolve) => {
    const notify = spawn("notify-send", ["--app-name=stt", `--replace-id=${NOTIFICATION_ID}`, "--expire-time=5000", "STT error", message], { stdio: "ignore" })
    notify.on("error", () => resolve(1))
    notify.on("exit", () => resolve(1))
  })
}

// The server explains a failure in `{ message }`.
const reply = async (response: Response): Promise<string> => {
  const body = (await response.json().catch(() => ({}))) as { readonly message?: string }
  return body.message ?? `stt-server answered HTTP ${response.status}`
}

const requestBody = (): string => {
  const display = process.env["WAYLAND_DISPLAY"] ?? ""
  if (display === "") {
    return "{}"
  }
  return JSON.stringify({ display })
}

// The exit code: 0 once the server has started or stopped the dictation.
export const toggle = async (): Promise<number> => {
  // The server pastes into, and draws the overlay on, the session that pressed End.
  const response = await fetch(new URL("/v1/dictation/toggle", serverUrl()), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: requestBody(),
  }).catch(() => null)
  if (response === null) {
    return failed(`stt-server is not reachable at ${serverUrl()}; start it with \`stt service install\``)
  }
  if (!response.ok) {
    return failed(await reply(response))
  }
  return 0
}

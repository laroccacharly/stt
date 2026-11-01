import { homedir } from "node:os"
import { join } from "node:path"
import { Console, Effect, FileSystem, Schema } from "effect"
import { ChildProcess, ChildProcessSpawner } from "effect/process"

export class HyprlandError extends Schema.TaggedError<HyprlandError>()("HyprlandError", {
  message: Schema.String,
}) {}

const REQUIRE_LINE = `require("hypr.stt")`

const hyprDirectory = () => join(homedir(), ".config", "hypr")
const bindingPath = () => join(hyprDirectory(), "stt.lua")
const configPath = () => join(hyprDirectory(), "hyprland.lua")

const failed = (cause: { readonly message: string }) => new HyprlandError({ message: cause.message })

// Hyprland's PATH has bun's global bin directory, where `bun link` put `stt`.
const BINDING = `o.bind("END", "Toggle speech to text", "stt toggle")`

const withRequireLine = (config: string) => {
  if (config.split("\n").some((line) => line.trim() === REQUIRE_LINE)) {
    return config
  }
  if (config === "" || config.endsWith("\n")) {
    return `${config}${REQUIRE_LINE}\n`
  }
  return `${config}\n${REQUIRE_LINE}\n`
}

const withoutRequireLine = (config: string) =>
  config
    .split("\n")
    .filter((line) => line.trim() !== REQUIRE_LINE)
    .join("\n")

const reload = Effect.gen(function* () {
  const spawner = yield* ChildProcessSpawner.ChildProcessSpawner
  const code = yield* spawner.exitCode(ChildProcess.make("hyprctl", ["reload"], { stdout: "ignore" })).pipe(Effect.mapError(failed))
  if (code !== 0) {
    return yield* new HyprlandError({ message: "hyprctl reload failed" })
  }
})

const rewriteConfig = Effect.fnUntraced(function* (change: (config: string) => string) {
  const fs = yield* FileSystem.FileSystem
  const config = yield* fs.readFileString(configPath()).pipe(Effect.mapError(failed))
  const changed = change(config)
  if (changed !== config) {
    yield* fs.writeFileString(configPath(), changed).pipe(Effect.mapError(failed))
  }
})

// Binds End to `stt toggle` in Hyprland.
export const install = Effect.gen(function* () {
  const fs = yield* FileSystem.FileSystem
  yield* fs.writeFileString(bindingPath(), `${BINDING}\n`).pipe(Effect.mapError(failed))
  yield* rewriteConfig(withRequireLine)
  yield* reload
  yield* Console.log(`Installed the End binding in ${bindingPath()}`)
})

export const uninstall = Effect.gen(function* () {
  const fs = yield* FileSystem.FileSystem
  yield* rewriteConfig(withoutRequireLine)
  yield* fs.remove(bindingPath(), { force: true }).pipe(Effect.mapError(failed))
  yield* reload
  yield* Console.log("Removed the End binding")
})

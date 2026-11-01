import { homedir } from "node:os"
import { join } from "node:path"

const xdgDirectory = (variable: string, fallback: string) => process.env[variable] || join(homedir(), fallback)

// `$XDG_RUNTIME_DIR/stt`: the recorder's PID file and log, and the phonon socket.
export const runtimeDirectory = (): string => join(process.env["XDG_RUNTIME_DIR"] || `/run/user/${process.getuid?.() ?? 0}`, "stt")

export const configPath = (): string => join(xdgDirectory("XDG_CONFIG_HOME", ".config"), "stt", "config.json")

export const historyPath = (): string => join(xdgDirectory("XDG_DATA_HOME", ".local/share"), "stt", "history.sqlite")

// uv keys a script's environment on its path, so the phonon scripts are copied to a stable place.
export const cacheDirectory = (): string => join(xdgDirectory("XDG_CACHE_HOME", ".cache"), "stt")

// The CLI entry point, which the recorder, the phonon service and the Hyprland binding run.
export const cliScript: string = join(import.meta.dir, "cli", "index.ts")

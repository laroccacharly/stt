#!/usr/bin/env bun
// `stt` and `stt toggle`, which End runs, skip loading the CLI: that alone takes about 200 ms.
import { toggle } from "./toggle.ts"

const args = process.argv.slice(2)
if (args.length === 0 || (args.length === 1 && args[0] === "toggle")) {
  process.exitCode = await toggle()
} else {
  await import("./main.ts")
}

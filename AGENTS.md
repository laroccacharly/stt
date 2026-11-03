# Agent notes

stt is a Bun + Effect CLI (`src/cli/index.ts`, linked as `stt` with `bun link`).
`stt toggle` (the End key) is a tiny entry point (`src/cli/toggle.ts`, no
Effect) that POSTs `/v1/dictation/toggle` to `stt-server`; every other command
loads `src/cli/main.ts`. The server runs the dictation itself
(`src/session/dictation.ts`): it records with the overlay, transcribes with the
current provider, pastes with `wl-copy` + `wtype` into the Wayland display the
request names, and saves to the SQLite history (`migrations/*.sql`, FLAC
through ffmpeg).

- Providers: `phonon-2.5-en`, a Python server (`python/`) that `stt serve`
  starts through `uv` and reaches over a Unix socket; and `openrouter`, through
  `effect-lib/stt`. Desktop dictation, `stt transcribe` and API clients all go
  through the server (`client/`), which owns the model process.
- Plumbing comes from `effect-lib` (`github:laroccacharly/effect-lib`):
  `credentials` (OPENROUTER_API_KEY, `stt login`), `systemd`, `json-store`
  (`~/.config/stt/config.json`) and `stt`.
- The waveform overlay is the `overlay` package
  (`github:laroccacharly/overlay`, napi-rs), installed from a tag that ships
  the prebuilt native module.

## After every change

1. Bump the version in `package.json`: patch for fixes, minor for features.
2. `bun run test` (fakes for every external tool, under 5 s) and, for model or
   API changes, `bun run test:live`.
3. `bun run typecheck` and `bun run lint` must print no errors or warnings.
4. A change to the server (`src/server/`, `src/phonon/`, `python/`) needs
   `stt service reinstall` to restart `stt-server`; so does a change to
   dictation (`src/session/`, `src/audio/`, `src/desktop/`, `src/history/`),
   which runs in the server. The CLI runs from this checkout.

# Learning more about Effect

This repository uses the Effect Typescript library.

Before writing any Effect code, first read `node_modules/effect/AGENTS.md`
**completely**, and follow the links in the file when required.

If you need to learn more about particular Effect apis and concepts that the
guide doesn't cover, search through the source code in `node_modules/effect/src`.

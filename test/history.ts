import { Database } from "bun:sqlite"
import { existsSync } from "node:fs"
import type { Sandbox } from "./sandbox.ts"

export interface Saved {
  readonly provider: string
  readonly model: string
  readonly transcript: string
  readonly transcriptionMs: number
  // Samples in the decoded FLAC.
  readonly samples: number
}

const decodedSamples = (flac: Uint8Array) => {
  const decoded = Bun.spawnSync(["ffmpeg", "-loglevel", "error", "-i", "pipe:0", "-f", "s16le", "pipe:1"], { stdin: flac })
  return decoded.stdout.length / 2
}

// The dictations saved in the sandbox's history, oldest first.
export const savedDictations = (sandbox: Sandbox): ReadonlyArray<Saved> => {
  const path = sandbox.path("data", "stt", "history.sqlite")
  if (!existsSync(path)) {
    return []
  }
  const database = new Database(path, { readonly: true })
  const rows = database.query("SELECT provider, model, transcript, transcription_ms, audio FROM transcriptions ORDER BY id").all() as ReadonlyArray<{
    provider: string
    model: string
    transcript: string
    transcription_ms: number
    audio: Uint8Array
  }>
  database.close()
  return rows.map((row) => ({ provider: row.provider, model: row.model, transcript: row.transcript, transcriptionMs: row.transcription_ms, samples: decodedSamples(row.audio) }))
}

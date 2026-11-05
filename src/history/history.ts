import { Database } from "bun:sqlite"
import { mkdirSync, readdirSync, readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { Context, Effect, Layer, Schema } from "effect"
import { Ffmpeg, type FfmpegError } from "../audio/ffmpeg.ts"
import { historyPath } from "../paths.ts"

export class HistoryError extends Schema.TaggedError<HistoryError>()("HistoryError", {
  message: Schema.String,
}) {}

const MIGRATIONS_DIRECTORY = join(import.meta.dir, "..", "..", "migrations")

// Schema migrations, oldest first. A database's user_version is how many it has applied, so only add new files.
const migrations = () =>
  readdirSync(MIGRATIONS_DIRECTORY)
    .filter((name) => name.endsWith(".sql"))
    .toSorted()
    .map((name) => readFileSync(join(MIGRATIONS_DIRECTORY, name), "utf8"))

const UserVersion = Schema.Struct({ user_version: Schema.Int })

// Applies each pending migration in its own transaction with the user_version bump, so a failure leaves the last good version.
const migrate = (database: Database) => {
  const applied = Schema.decodeUnknownSync(UserVersion)(database.query("PRAGMA user_version").get()).user_version
  migrations().forEach((sql, index) => {
    const version = index + 1
    if (version <= applied) return
    database.transaction(() => {
      database.run(sql)
      database.run(`PRAGMA user_version = ${version}`)
    })()
  })
}

// A saved dictation, without its audio.
export const Entry = Schema.Struct({
  // Local time, `YYYY-MM-DD HH:MM:SS`.
  time: Schema.String,
  provider: Schema.String,
  model: Schema.String,
  transcript: Schema.String,
})
export type Entry = typeof Entry.Type

export interface Dictation {
  readonly provider: string
  readonly model: string
  // PCM at SAMPLE_RATE, saved as FLAC.
  readonly pcm: Uint8Array
  readonly transcript: string
  // How long transcribing took after recording stopped.
  readonly transcriptionMs: number
}

// Every dictation in `$XDG_DATA_HOME/stt/history.sqlite`.
export class History extends Context.Service<
  History,
  {
    readonly save: (dictation: Dictation) => Effect.Effect<void, HistoryError | FfmpegError>
    // The most recent dictations, newest first.
    readonly recent: (limit: number) => Effect.Effect<ReadonlyArray<Entry>, HistoryError>
  }
>()("stt/history/History") {
  static readonly layer = Layer.effect(
    History,
    Effect.gen(function* () {
      const ffmpeg = yield* Ffmpeg
      const path = historyPath()
      const failed = (action: string, reason: string) => new HistoryError({ message: `could not ${action} ${path}: ${reason}` })
      // Opened on first use, so commands that never touch the history never create it.
      const database = yield* Effect.cached(
        Effect.try({
          try: () => {
            mkdirSync(dirname(path), { recursive: true })
            const opened = new Database(path, { create: true, strict: true })
            migrate(opened)
            return opened
          },
          catch: (cause) => failed("open", String(cause)),
        })
      )

      const save: (dictation: Dictation) => Effect.Effect<void, HistoryError | FfmpegError> = Effect.fn("History.save")(function* (dictation: Dictation) {
        const audio = yield* ffmpeg.encodeFlac(dictation.pcm)
        const opened = yield* database
        yield* Effect.try({
          try: () =>
            opened
              .query("INSERT INTO transcriptions (provider, model, audio, transcript, transcription_ms) VALUES ($provider, $model, $audio, $transcript, $ms)")
              .run({
                provider: dictation.provider,
                model: dictation.model,
                audio,
                transcript: dictation.transcript,
                ms: Math.round(dictation.transcriptionMs),
              }),
          catch: (cause) => failed("save to", String(cause)),
        })
      })

      const recent: (limit: number) => Effect.Effect<ReadonlyArray<Entry>, HistoryError> = Effect.fn("History.recent")(function* (limit: number) {
        const opened = yield* database
        const rows = yield* Effect.try({
          try: () =>
            opened
              .query(
                "SELECT strftime('%Y-%m-%d %H:%M:%S', created_at, 'localtime') AS time, provider, model, transcript FROM transcriptions ORDER BY id DESC LIMIT $limit"
              )
              .all({ limit }),
          catch: (cause) => failed("read", String(cause)),
        })
        return yield* Schema.decodeUnknownEffect(Schema.Array(Entry))(rows).pipe(Effect.mapError((error) => failed("read", error.message)))
      })

      return History.of({ save, recent })
    })
  )
}

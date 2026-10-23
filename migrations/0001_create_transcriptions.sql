-- IF NOT EXISTS adopts histories created before migrations were tracked.
CREATE TABLE IF NOT EXISTS transcriptions (
    id INTEGER PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    audio BLOB NOT NULL,
    transcript TEXT NOT NULL
);

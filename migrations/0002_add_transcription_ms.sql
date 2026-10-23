-- How long a batch transcription took after recording stopped; 0 for realtime.
ALTER TABLE transcriptions ADD COLUMN transcription_ms INTEGER NOT NULL DEFAULT 0;

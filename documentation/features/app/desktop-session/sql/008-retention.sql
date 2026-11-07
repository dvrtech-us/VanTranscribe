-- Durable confirmation survives process termination. No meeting FK: deleted meetings leave a minimal operation receipt.
CREATE TABLE IF NOT EXISTS app_retention_intents (
 token TEXT PRIMARY KEY, meeting_id TEXT NOT NULL, kind TEXT NOT NULL CHECK(kind IN ('meeting','audio','runs')),
 state TEXT NOT NULL CHECK(state IN ('preview','confirmed','cleanup','done','failed')),
 payload_json TEXT NOT NULL, created_at TEXT NOT NULL, confirmed_at TEXT, error TEXT
);
CREATE INDEX IF NOT EXISTS app_retention_pending ON app_retention_intents(state);
CREATE TABLE IF NOT EXISTS app_retention_gate (
 token TEXT PRIMARY KEY REFERENCES app_retention_intents(token)
);
CREATE TRIGGER IF NOT EXISTS app_retention_gate_confirmed BEFORE INSERT ON app_retention_gate
 WHEN NOT EXISTS(SELECT 1 FROM app_retention_intents WHERE token=NEW.token AND state='confirmed')
 BEGIN SELECT RAISE(ABORT,'Deletion requires a confirmed durable intent'); END;
CREATE TABLE IF NOT EXISTS app_audio_retention (
 meeting_id TEXT PRIMARY KEY REFERENCES app_meetings(id),
 audio_available INTEGER NOT NULL CHECK(audio_available IN (0,1)),
 purged_at TEXT NOT NULL, original_chunks INTEGER NOT NULL, original_bytes INTEGER NOT NULL
);

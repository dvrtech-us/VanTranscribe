-- Speaker embeddings never enter this schema. Only meeting-local labels,
-- time spans, explicit user constraints, and assignment provenance persist.
CREATE TABLE IF NOT EXISTS app_speaker_identities (
 id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
 track TEXT NOT NULL CHECK(track IN ('mic','system')), label TEXT NOT NULL,
 user_name TEXT, alias_of TEXT REFERENCES app_speaker_identities(id),
 alias_constraint_id TEXT REFERENCES app_speaker_constraints(id),
 is_me INTEGER NOT NULL DEFAULT 0, created_at TEXT NOT NULL,
 shared_room INTEGER NOT NULL DEFAULT 0 CHECK(shared_room IN (0,1)),
 person_id TEXT REFERENCES app_people(id) ON DELETE SET NULL
);
CREATE TABLE IF NOT EXISTS app_run_speaker_keys (
 run_id TEXT NOT NULL REFERENCES app_processing_runs(id), local_id TEXT NOT NULL,
 identity_id TEXT NOT NULL REFERENCES app_speaker_identities(id),
 PRIMARY KEY(run_id,local_id)
);
CREATE TABLE IF NOT EXISTS app_speaker_turns (
 run_id TEXT NOT NULL REFERENCES app_processing_runs(id), id TEXT NOT NULL,
 track TEXT NOT NULL CHECK(track IN ('mic','system')), start_ms INTEGER NOT NULL CHECK(start_ms>=0),
 end_ms INTEGER NOT NULL CHECK(end_ms>=start_ms), speaker_ids_json TEXT NOT NULL,
 label TEXT NOT NULL, provenance TEXT NOT NULL, score REAL,
 overlap INTEGER NOT NULL, uncertain INTEGER NOT NULL, provisional INTEGER NOT NULL,
 PRIMARY KEY(run_id,id)
);
CREATE INDEX IF NOT EXISTS app_speaker_turns_time ON app_speaker_turns(run_id,track,start_ms,end_ms);
CREATE TRIGGER IF NOT EXISTS app_speaker_turn_insert_guard BEFORE INSERT ON app_speaker_turns
 WHEN (SELECT status FROM app_processing_runs WHERE id=NEW.run_id)!='running'
 BEGIN SELECT RAISE(ABORT,'Completed speaker turns are immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_speaker_turn_update_guard BEFORE UPDATE ON app_speaker_turns
 WHEN (SELECT status FROM app_processing_runs WHERE id=OLD.run_id)!='running'
 BEGIN SELECT RAISE(ABORT,'Completed speaker turns are immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_speaker_turn_delete_guard BEFORE DELETE ON app_speaker_turns
 WHEN (SELECT status FROM app_processing_runs WHERE id=OLD.run_id)!='running'
 BEGIN SELECT RAISE(ABORT,'Completed speaker turns are immutable'); END;
CREATE TABLE IF NOT EXISTS app_speaker_run_state (
 run_id TEXT PRIMARY KEY REFERENCES app_processing_runs(id),
 embedding_version TEXT NOT NULL, calibrated INTEGER NOT NULL,
 warnings_json TEXT NOT NULL, segmentation_context_json TEXT
);
-- Existing tables receive this nullable column only after store::initialize
-- checks PRAGMA table_info; legacy rows retain NULL (unknown settings).
CREATE TABLE IF NOT EXISTS app_speaker_constraints (
 id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
 kind TEXT NOT NULL CHECK(kind IN ('rename','must_link','cannot_link','revoke')),
 left_anchor_json TEXT NOT NULL, right_anchor_json TEXT,
 identity_id TEXT REFERENCES app_speaker_identities(id), value TEXT,
 version INTEGER NOT NULL, created_at TEXT NOT NULL, is_active INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS app_speaker_constraints_meeting ON app_speaker_constraints(meeting_id,version);
CREATE TABLE IF NOT EXISTS app_speaker_turn_overrides (
 run_id TEXT NOT NULL, turn_id TEXT NOT NULL,
 identity_id TEXT NOT NULL REFERENCES app_speaker_identities(id),
 constraint_id TEXT REFERENCES app_speaker_constraints(id),
 PRIMARY KEY(run_id,turn_id),
 FOREIGN KEY(run_id,turn_id) REFERENCES app_speaker_turns(run_id,id)
);
CREATE TABLE IF NOT EXISTS app_speaker_reviews (
 id TEXT PRIMARY KEY, run_id TEXT NOT NULL REFERENCES app_processing_runs(id),
 kind TEXT NOT NULL, previous_ids_json TEXT NOT NULL, current_ids_json TEXT NOT NULL,
 reason TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'needs_review',
 constraint_id TEXT REFERENCES app_speaker_constraints(id)
);

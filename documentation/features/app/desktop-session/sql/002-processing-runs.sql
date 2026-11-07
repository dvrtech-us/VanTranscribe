-- Legacy app_segments are retained as a migration archive. New writes use run-scoped rows.
CREATE TABLE IF NOT EXISTS app_processing_runs (
 id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
 kind TEXT NOT NULL CHECK(kind IN ('live','replay','refine','legacy')),
 status TEXT NOT NULL CHECK(status IN ('running','complete','abandoned','superseded')),
 engines_json TEXT NOT NULL, constraints_version INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS app_runs_meeting ON app_processing_runs(meeting_id,created_at);
CREATE TABLE IF NOT EXISTS app_meeting_revision (
 meeting_id TEXT PRIMARY KEY REFERENCES app_meetings(id),
 active_run_id TEXT REFERENCES app_processing_runs(id), correction_version INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS app_run_segments (
 run_id TEXT NOT NULL REFERENCES app_processing_runs(id), id TEXT NOT NULL,
 track TEXT NOT NULL CHECK(track IN ('mic','system')),
 start_ms INTEGER NOT NULL CHECK(start_ms >= 0), end_ms INTEGER NOT NULL CHECK(end_ms >= start_ms),
 text TEXT NOT NULL,speaker TEXT NOT NULL,words_json TEXT NOT NULL DEFAULT '[]', PRIMARY KEY(run_id,id)
);
CREATE INDEX IF NOT EXISTS app_run_segments_time ON app_run_segments(run_id,start_ms,id);
CREATE TRIGGER IF NOT EXISTS app_segment_insert_guard BEFORE INSERT ON app_run_segments
 WHEN (SELECT status FROM app_processing_runs WHERE id=NEW.run_id) != 'running'
 BEGIN SELECT RAISE(ABORT,'Completed transcript rows are immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_segment_update_guard BEFORE UPDATE ON app_run_segments
 BEGIN SELECT RAISE(ABORT,'Transcript rows are immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_segment_delete_guard BEFORE DELETE ON app_run_segments
 BEGIN SELECT RAISE(ABORT,'Transcript rows are immutable'); END;
CREATE TABLE IF NOT EXISTS app_segment_corrections (
 id TEXT PRIMARY KEY, meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
 track TEXT NOT NULL,start_ms INTEGER NOT NULL,end_ms INTEGER NOT NULL,
 original_text_hash TEXT NOT NULL,replacement_text TEXT NOT NULL,
 status TEXT NOT NULL CHECK(status IN ('applied','needs_review','dismissed')),created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS app_corrections_meeting ON app_segment_corrections(meeting_id);
CREATE TABLE IF NOT EXISTS app_correction_matches (
 correction_id TEXT NOT NULL REFERENCES app_segment_corrections(id),
 run_id TEXT NOT NULL, segment_id TEXT NOT NULL,
 PRIMARY KEY(correction_id,run_id), UNIQUE(run_id,segment_id),
 FOREIGN KEY(run_id,segment_id) REFERENCES app_run_segments(run_id,id)
);
CREATE TABLE IF NOT EXISTS app_summaries (
 id TEXT PRIMARY KEY,meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
 run_id TEXT NOT NULL REFERENCES app_processing_runs(id),correction_version INTEGER NOT NULL,
 provider TEXT NOT NULL,model TEXT NOT NULL,template TEXT NOT NULL,
 content_json TEXT NOT NULL,content_md TEXT NOT NULL,is_stale INTEGER NOT NULL DEFAULT 0,
 created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS app_summaries_meeting ON app_summaries(meeting_id,created_at);
CREATE TABLE IF NOT EXISTS app_summary_sources (
 summary_id TEXT NOT NULL REFERENCES app_summaries(id),run_id TEXT NOT NULL,segment_id TEXT NOT NULL,
 PRIMARY KEY(summary_id,segment_id),FOREIGN KEY(run_id,segment_id) REFERENCES app_run_segments(run_id,id)
);
CREATE VIRTUAL TABLE IF NOT EXISTS app_search_fts USING fts5(meeting_id UNINDEXED,source UNINDEXED,ref_id UNINDEXED,text);

-- Provider/summary failures do not overwrite recording or transcript health warnings.
CREATE TABLE IF NOT EXISTS app_summary_failures (
 meeting_id TEXT PRIMARY KEY REFERENCES app_meetings(id) ON DELETE CASCADE,
 error TEXT NOT NULL
);

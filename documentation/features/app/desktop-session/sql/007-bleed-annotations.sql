-- User decisions are immutable events. Match snapshots belong to each transcript run.
CREATE TABLE IF NOT EXISTS app_bleed_decisions (
    seq INTEGER PRIMARY KEY AUTOINCREMENT,
    id TEXT NOT NULL UNIQUE,
    lineage_id TEXT NOT NULL,
    meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
    source_run_id TEXT NOT NULL,
    source_segment_id TEXT NOT NULL,
    track TEXT NOT NULL CHECK(track='mic'),
    start_ms INTEGER NOT NULL,
    end_ms INTEGER NOT NULL CHECK(end_ms>=start_ms),
    original_text_hash TEXT NOT NULL,
    decision TEXT NOT NULL CHECK(decision IN ('bleed','genuine')),
    reason TEXT NOT NULL,
    retired INTEGER NOT NULL DEFAULT 0 CHECK(retired IN (0,1)),
    created_at TEXT NOT NULL,
    FOREIGN KEY(source_run_id,source_segment_id) REFERENCES app_run_segments(run_id,id)
);
CREATE INDEX IF NOT EXISTS app_bleed_lineage ON app_bleed_decisions(meeting_id,lineage_id,seq);
CREATE TRIGGER IF NOT EXISTS app_bleed_audit_immutable_update BEFORE UPDATE ON app_bleed_decisions BEGIN SELECT RAISE(ABORT,'Bleed decision audit is immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_bleed_audit_immutable_delete BEFORE DELETE ON app_bleed_decisions BEGIN SELECT RAISE(ABORT,'Bleed decision audit is immutable'); END;
CREATE TABLE IF NOT EXISTS app_bleed_run_matches (
    run_id TEXT NOT NULL REFERENCES app_processing_runs(id),
    decision_id TEXT NOT NULL REFERENCES app_bleed_decisions(id),
    segment_id TEXT,
    status TEXT NOT NULL CHECK(status IN ('applied','needs_review','superseded','dismissed')),
    reason TEXT NOT NULL,
    PRIMARY KEY(run_id,decision_id),
    FOREIGN KEY(run_id,segment_id) REFERENCES app_run_segments(run_id,id)
);
CREATE INDEX IF NOT EXISTS app_bleed_matches_segment ON app_bleed_run_matches(run_id,segment_id);
-- Current decisions can change; completed historical snapshots cannot.
CREATE TRIGGER IF NOT EXISTS app_bleed_snapshot_insert BEFORE INSERT ON app_bleed_run_matches WHEN NOT EXISTS (
 SELECT 1 FROM app_processing_runs r LEFT JOIN app_meeting_revision v ON v.meeting_id=r.meeting_id
 WHERE r.id=NEW.run_id AND (r.status='running' OR v.active_run_id=r.id)
) BEGIN SELECT RAISE(ABORT,'Historical bleed snapshot is immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_bleed_snapshot_update BEFORE UPDATE ON app_bleed_run_matches WHEN NOT EXISTS (
 SELECT 1 FROM app_processing_runs r LEFT JOIN app_meeting_revision v ON v.meeting_id=r.meeting_id
 WHERE r.id=OLD.run_id AND (r.status='running' OR v.active_run_id=r.id)
) BEGIN SELECT RAISE(ABORT,'Historical bleed snapshot is immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_bleed_snapshot_delete BEFORE DELETE ON app_bleed_run_matches WHEN NOT EXISTS (
 SELECT 1 FROM app_processing_runs r LEFT JOIN app_meeting_revision v ON v.meeting_id=r.meeting_id
 WHERE r.id=OLD.run_id AND (r.status='running' OR v.active_run_id=r.id)
) BEGIN SELECT RAISE(ABORT,'Historical bleed snapshot is immutable'); END;

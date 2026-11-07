-- Inference coverage is separate from immutable raw recording gap rows.
CREATE TABLE IF NOT EXISTS app_run_inference_health (
    run_id TEXT PRIMARY KEY REFERENCES app_processing_runs(id) ON DELETE CASCADE,
    meeting_id TEXT NOT NULL REFERENCES app_meetings(id) ON DELETE CASCADE,
    info_json TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS app_run_inference_health_meeting ON app_run_inference_health(meeting_id);
CREATE TRIGGER IF NOT EXISTS app_run_inference_health_sealed_update
BEFORE UPDATE ON app_run_inference_health
WHEN (SELECT status FROM app_processing_runs WHERE id=OLD.run_id) <> 'running'
BEGIN SELECT RAISE(ABORT,'completed inference health is immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_run_inference_health_sealed_insert
BEFORE INSERT ON app_run_inference_health
WHEN (SELECT status FROM app_processing_runs WHERE id=NEW.run_id) <> 'running'
BEGIN SELECT RAISE(ABORT,'completed inference health is immutable'); END;

CREATE TRIGGER IF NOT EXISTS app_run_inference_health_owner_insert
BEFORE INSERT ON app_run_inference_health
WHEN NOT EXISTS(SELECT 1 FROM app_processing_runs WHERE id=NEW.run_id AND meeting_id=NEW.meeting_id)
BEGIN SELECT RAISE(ABORT,'inference health run belongs to another meeting'); END;
CREATE TRIGGER IF NOT EXISTS app_run_inference_health_owner_update
BEFORE UPDATE ON app_run_inference_health
WHEN NEW.run_id<>OLD.run_id OR NEW.meeting_id<>OLD.meeting_id
BEGIN SELECT RAISE(ABORT,'inference health ownership is immutable'); END;

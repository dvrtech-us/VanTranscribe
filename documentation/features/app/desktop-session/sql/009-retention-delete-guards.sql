-- Only DELETE guards change. Normal inserts/updates remain immutable.
-- This gate is inserted and removed inside the same deletion transaction.
DROP TRIGGER IF EXISTS app_segment_delete_guard;
CREATE TRIGGER app_segment_delete_guard BEFORE DELETE ON app_run_segments
 WHEN NOT EXISTS(SELECT 1 FROM app_retention_gate g JOIN app_retention_intents i ON i.token=g.token
 JOIN app_processing_runs r ON r.meeting_id=i.meeting_id WHERE r.id=OLD.run_id AND i.state='confirmed' AND (i.kind='meeting' OR (i.kind IN ('runs','audio') AND EXISTS(SELECT 1 FROM json_each(i.payload_json,'$.run_ids') WHERE value=OLD.run_id))))
 BEGIN SELECT RAISE(ABORT,'Transcript rows are immutable'); END;
DROP TRIGGER IF EXISTS app_speaker_turn_delete_guard;
CREATE TRIGGER app_speaker_turn_delete_guard BEFORE DELETE ON app_speaker_turns
 WHEN (SELECT status FROM app_processing_runs WHERE id=OLD.run_id)!='running' AND NOT EXISTS(
 SELECT 1 FROM app_retention_gate g JOIN app_retention_intents i ON i.token=g.token
 JOIN app_processing_runs r ON r.meeting_id=i.meeting_id WHERE r.id=OLD.run_id AND i.state='confirmed' AND (i.kind='meeting' OR (i.kind IN ('runs','audio') AND EXISTS(SELECT 1 FROM json_each(i.payload_json,'$.run_ids') WHERE value=OLD.run_id))))
 BEGIN SELECT RAISE(ABORT,'Completed speaker turns are immutable'); END;
DROP TRIGGER IF EXISTS app_bleed_audit_immutable_delete;
CREATE TRIGGER app_bleed_audit_immutable_delete BEFORE DELETE ON app_bleed_decisions
 WHEN NOT EXISTS(SELECT 1 FROM app_retention_gate g JOIN app_retention_intents i ON i.token=g.token
 WHERE i.meeting_id=OLD.meeting_id AND i.state='confirmed' AND i.kind='meeting')
 BEGIN SELECT RAISE(ABORT,'Bleed decision audit is immutable'); END;
DROP TRIGGER IF EXISTS app_bleed_snapshot_delete;
CREATE TRIGGER app_bleed_snapshot_delete BEFORE DELETE ON app_bleed_run_matches WHEN NOT EXISTS (
 SELECT 1 FROM app_processing_runs r LEFT JOIN app_meeting_revision v ON v.meeting_id=r.meeting_id
 WHERE r.id=OLD.run_id AND (r.status='running' OR v.active_run_id=r.id)
) AND NOT EXISTS(SELECT 1 FROM app_retention_gate g JOIN app_retention_intents i ON i.token=g.token
 JOIN app_processing_runs r ON r.meeting_id=i.meeting_id WHERE r.id=OLD.run_id AND i.state='confirmed' AND (i.kind='meeting' OR (i.kind IN ('runs','audio') AND EXISTS(SELECT 1 FROM json_each(i.payload_json,'$.run_ids') WHERE value=OLD.run_id))))
 BEGIN SELECT RAISE(ABORT,'Historical bleed snapshot is immutable'); END;

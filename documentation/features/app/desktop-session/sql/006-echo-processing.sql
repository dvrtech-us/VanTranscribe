-- Route observations describe capture-time evidence, never the current device
-- at replay time. No process/device names, IDs or audio samples are stored here.
CREATE TABLE IF NOT EXISTS app_output_routes (
    meeting_id TEXT NOT NULL REFERENCES app_meetings(id) ON DELETE CASCADE,
    epoch INTEGER NOT NULL CHECK(epoch >= 0),
    start_ns INTEGER NOT NULL CHECK(start_ns >= 0),
    route_json TEXT NOT NULL,
    PRIMARY KEY(meeting_id,start_ns,epoch)
);
CREATE TRIGGER IF NOT EXISTS app_output_routes_immutable
BEFORE UPDATE ON app_output_routes
BEGIN SELECT RAISE(ABORT,'capture route observations are immutable'); END;

-- Written only after a clean stop and successful route persistence. Interrupted
-- or incomplete histories are never used to enable offline echo cancellation.
CREATE TABLE IF NOT EXISTS app_route_evidence (
    meeting_id TEXT PRIMARY KEY REFERENCES app_meetings(id) ON DELETE CASCADE,
    complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0,1))
);

CREATE TABLE IF NOT EXISTS app_run_echo_processing (
    run_id TEXT PRIMARY KEY REFERENCES app_processing_runs(id) ON DELETE CASCADE,
    info_json TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS app_run_echo_processing_sealed_update
BEFORE UPDATE ON app_run_echo_processing
WHEN (SELECT status FROM app_processing_runs WHERE id=OLD.run_id) <> 'running'
BEGIN SELECT RAISE(ABORT,'completed echo processing is immutable'); END;
CREATE TRIGGER IF NOT EXISTS app_run_echo_processing_sealed_insert
BEFORE INSERT ON app_run_echo_processing
WHEN (SELECT status FROM app_processing_runs WHERE id=NEW.run_id) <> 'running'
BEGIN SELECT RAISE(ABORT,'completed echo processing is immutable'); END;

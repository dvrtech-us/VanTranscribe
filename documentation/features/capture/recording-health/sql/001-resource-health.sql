CREATE TABLE IF NOT EXISTS app_resource_health (
    id INTEGER PRIMARY KEY CHECK(id=1),
    snapshot_json TEXT NOT NULL,
    checked_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS app_source_timing (
    meeting_id TEXT PRIMARY KEY REFERENCES app_meetings(id) ON DELETE CASCADE,
    timing_json TEXT NOT NULL,
    origin_ns INTEGER CHECK(origin_ns IS NULL OR origin_ns>=0)
);
CREATE TABLE IF NOT EXISTS app_note_timing (
    note_id TEXT PRIMARY KEY REFERENCES app_notes(id) ON DELETE CASCADE,
    provenance TEXT NOT NULL,
    synchronized INTEGER NOT NULL CHECK(synchronized IN (0,1))
);
CREATE TABLE IF NOT EXISTS app_meeting_health (
    id INTEGER PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES app_meetings(id) ON DELETE CASCADE,
    source_ms INTEGER NOT NULL CHECK(source_ms>=0),
    free_bytes INTEGER CHECK(free_bytes IS NULL OR free_bytes>=0),
    disk_state TEXT NOT NULL,
    inference_suspended INTEGER NOT NULL CHECK(inference_suspended IN (0,1)),
    created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS app_meeting_health_timeline ON app_meeting_health(meeting_id,source_ms,id);

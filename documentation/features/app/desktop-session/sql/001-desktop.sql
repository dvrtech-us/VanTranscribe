CREATE TABLE IF NOT EXISTS app_meetings (
    id TEXT PRIMARY KEY REFERENCES meetings(id),
    title TEXT NOT NULL,
    status TEXT NOT NULL CHECK(status IN ('recording','paused','processing','ready','recovered','failed')),
    started_at TEXT NOT NULL,
    duration_ms INTEGER NOT NULL DEFAULT 0 CHECK(duration_ms >= 0),
    mode TEXT NOT NULL CHECK(mode IN ('remote','in_person')),
    error TEXT
);
CREATE TABLE IF NOT EXISTS app_segments (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
    track TEXT NOT NULL CHECK(track IN ('mic','system')),
    start_ms INTEGER NOT NULL CHECK(start_ms >= 0),
    end_ms INTEGER NOT NULL CHECK(end_ms >= start_ms),
    text TEXT NOT NULL,
    speaker TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS app_segments_meeting_time ON app_segments(meeting_id,start_ms);
CREATE TABLE IF NOT EXISTS app_notes (
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
    t_ms INTEGER NOT NULL CHECK(t_ms >= 0),
    text TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS app_notes_meeting_time ON app_notes(meeting_id,t_ms);
-- Replay publishes atomically. Until success, the existing transcript remains
-- visible; interrupted staging rows can be discarded without losing it.
CREATE TABLE IF NOT EXISTS app_transcription_staging (
    job_id TEXT NOT NULL,
    id TEXT PRIMARY KEY,
    meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
    track TEXT NOT NULL CHECK(track IN ('mic','system')),
    start_ms INTEGER NOT NULL CHECK(start_ms >= 0),
    end_ms INTEGER NOT NULL CHECK(end_ms >= start_ms),
    text TEXT NOT NULL,
    speaker TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS app_transcription_staging_job ON app_transcription_staging(job_id);
CREATE TABLE IF NOT EXISTS app_audio_clock (
    meeting_id TEXT NOT NULL REFERENCES app_meetings(id),
    track TEXT NOT NULL,
    seq INTEGER NOT NULL,
    source_epoch INTEGER NOT NULL,
    source_first_frame INTEGER NOT NULL,
    host_time_ns INTEGER NOT NULL,
    PRIMARY KEY(meeting_id,track,seq)
);

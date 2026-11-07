-- M0 recording manifest. Frame indexes are per channel, relative to an epoch.
-- Nanosecond times are relative to the session's monotonic clock origin.
CREATE TABLE meetings (
    id TEXT PRIMARY KEY,
    status TEXT NOT NULL CHECK (status IN ('recording', 'processing', 'recovered'))
);

CREATE TABLE audio_chunks (
    meeting_id TEXT NOT NULL REFERENCES meetings(id),
    track TEXT NOT NULL CHECK (track IN ('mic', 'system')),
    seq INTEGER NOT NULL CHECK (seq >= 0),
    epoch INTEGER NOT NULL CHECK (epoch >= 0),
    first_frame INTEGER NOT NULL CHECK (first_frame >= 0),
    frame_count INTEGER NOT NULL CHECK (frame_count >= 0),
    sample_rate INTEGER NOT NULL CHECK (sample_rate BETWEEN 8000 AND 192000),
    channels INTEGER NOT NULL CHECK (channels BETWEEN 1 AND 8),
    start_ns INTEGER NOT NULL CHECK (start_ns >= 0),
    end_ns INTEGER NOT NULL CHECK (end_ns > start_ns),
    kind TEXT NOT NULL CHECK (kind IN ('audio', 'gap')),
    state TEXT NOT NULL CHECK (state IN ('pending', 'ready', 'gap')),
    path TEXT,
    sha256 TEXT,
    gap_reason TEXT,
    clipped_samples INTEGER NOT NULL DEFAULT 0 CHECK (clipped_samples >= 0),
    PRIMARY KEY (meeting_id, track, seq),
    CHECK ((kind = 'audio' AND state IN ('pending', 'ready') AND path IS NOT NULL AND sha256 IS NOT NULL AND frame_count > 0)
        OR (kind = 'gap' AND state = 'gap' AND gap_reason IS NOT NULL AND path IS NULL AND sha256 IS NULL))
);
PRAGMA user_version = 1;

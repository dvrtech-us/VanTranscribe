-- Applied idempotently by vt-speakers::initialize to the existing app database.
CREATE TABLE IF NOT EXISTS app_voice_memory_settings (
 singleton INTEGER PRIMARY KEY CHECK(singleton=1),
 enabled INTEGER NOT NULL DEFAULT 0 CHECK(enabled IN (0,1)),
 retention_months INTEGER NOT NULL DEFAULT 12 CHECK(retention_months BETWEEN 1 AND 24),
 explanation_acknowledged_at TEXT,
 store_id TEXT NOT NULL,
 key_check BLOB
);
CREATE TABLE IF NOT EXISTS app_people (
 id TEXT PRIMARY KEY,
 display_name TEXT NOT NULL,
 is_self INTEGER NOT NULL CHECK(is_self IN (0,1)),
 created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS app_people_one_self ON app_people(is_self) WHERE is_self=1;
CREATE TABLE IF NOT EXISTS app_biometric_consents (
 id TEXT PRIMARY KEY,
 person_id TEXT NOT NULL REFERENCES app_people(id) ON DELETE CASCADE,
 attested_at TEXT NOT NULL,
 method TEXT NOT NULL,
 attested_by TEXT NOT NULL,
 is_self INTEGER NOT NULL CHECK(is_self IN (0,1)),
 revoked_at TEXT
);
CREATE INDEX IF NOT EXISTS app_biometric_consents_person ON app_biometric_consents(person_id);
CREATE TABLE IF NOT EXISTS app_voiceprints (
 id TEXT PRIMARY KEY,
 person_id TEXT NOT NULL REFERENCES app_people(id) ON DELETE CASCADE,
 channel_type TEXT NOT NULL CHECK(channel_type IN ('remote_wideband','remote_narrowband','in_person')),
 channel_provenance TEXT NOT NULL CHECK(channel_provenance IN ('user_declared','meeting_mode')),
 embedding_version TEXT NOT NULL,
 exemplars BLOB NOT NULL,
 n_exemplars INTEGER NOT NULL CHECK(n_exemplars BETWEEN 1 AND 10),
 last_matched_at TEXT NOT NULL,
 expires_at TEXT NOT NULL,
 updated_at TEXT NOT NULL,
 UNIQUE(person_id,channel_type,embedding_version)
);
CREATE INDEX IF NOT EXISTS app_voiceprints_expiry ON app_voiceprints(expires_at);

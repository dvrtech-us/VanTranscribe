-- Explicit processing preferences only; provider credentials live in Keychain.
CREATE TABLE IF NOT EXISTS app_settings (
 key TEXT PRIMARY KEY,
 value_json TEXT NOT NULL
);

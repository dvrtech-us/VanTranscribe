-- Executed once when the legacy staging table lacks acoustic word timing.
ALTER TABLE app_transcription_staging ADD COLUMN words_json TEXT NOT NULL DEFAULT '[]';

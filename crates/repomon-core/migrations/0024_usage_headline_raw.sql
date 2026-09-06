-- Preserve unfiltered headline input so the UI can show the original text.
ALTER TABLE usage_sessions ADD COLUMN headline_raw TEXT;

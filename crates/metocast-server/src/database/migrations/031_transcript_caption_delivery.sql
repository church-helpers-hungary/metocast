-- What happened to each live-transcript session's YouTube captions: how many posts YouTube
-- accepted, how many failed, the latest failure, and the stream they went to. Never the signed
-- ingestion URL.
ALTER TABLE transcript_sessions
    ADD COLUMN IF NOT EXISTS captions_delivered INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS captions_failed    INTEGER NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS caption_last_error TEXT,
    ADD COLUMN IF NOT EXISTS caption_stream_id  TEXT;

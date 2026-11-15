# Stored transcripts

Every live English transcript is saved as it runs. Each finished sentence goes into the core's
PostgreSQL database with its start and end time, in `transcript_sessions` and
`transcript_segments`. Nothing is written to a file until you download one. The **transcript
library** lets you list these sessions, read them, download them, delete them, and have AI clean
them up.

Only the English text is kept. The Hungarian source is never transcribed, so it is not stored.

## Use it

**Apple app:** **Settings → Server Tools → Transcripts**.

- The list shows each session's event, date, length and number of lines, newest first. It also
  marks the session that is live now and the ones that have been cleaned up.
- Open one to read it.
  - **Download** saves it as `.txt` (one line per sentence), `.srt` or `.vtt`. Subtitle times count
    from the start of the session. Files are named `transcript-<date>.en.vtt`: drop one on the
    Media page and it is taken as English, then placed sentence by sentence on the program's
    speech ([Media processing](media.md#transcripts)).
  - **Clean Up with AI** (see below).
  - **Delete** removes the session, with its original and cleaned-up text. This cannot be undone.
- Once a clean-up has succeeded, the **Original / Cleaned Up** switch picks which text is shown
  and downloaded.

**Sanctum:** **Stored transcripts**, linked from the dashboard's English transcript card
(`/transcripts`). It has the same list, reader, downloads, clean-up and delete.

The session that is live now cannot be deleted or cleaned up until it stops.

## Live caption delivery

When **Live YouTube captions** is on, each finished sentence is also posted to the YouTube
broadcast as an HTTP-POST caption. Every session keeps a record of what happened to those posts:

- **Delivered** and **failed** counts. A caption counts as failed when YouTube refused it after
  three tries, when it was over a minute old, or when no caption ingestion URL was found.
- The **last error**, such as `HTTP 403 Forbidden: …`.
- The **stream** the captions were posted to (`captionStreamId`). It is empty when the ingestion
  URL was pasted in the OpenAI settings.

The captions go to the stream bound to the event's YouTube broadcast (`liveBroadcasts` →
`boundStreamId` → that stream's `closedCaptionsIngestionUrl`). A broadcast is bound when it goes
live, and the transcript usually starts before that, so until then the channel's first stream is
used and the lookup is repeated at most every 30 s, when a caption is due. A session with no event
always uses the channel's first stream. The signed ingestion URL is never stored or logged.

The Apple app and Sanctum show the record under the transcript's summary, for example "YouTube
captions: 779 delivered, 0 failed · stream …", and the last error in red when any failed. It is
on `GET /api/transcripts` and `GET /api/transcripts/{id}` (and the `transcripts_list` and
`transcripts_get` MCP tools) as `captionsDelivered`, `captionsFailed`, `captionLastError` and
`captionStreamId`.

## AI clean-up

A live translation comes out sentence by sentence, so it has broken words, repeated fragments and
mistranslations. **Clean Up with AI** sends the finished English lines to OpenAI's `gpt-5-mini`.
It uses the API key of the **OpenAI Translation** connector, so it needs no new setup.

- The lines go in chunks of 40. Each chunk is sent with the cleaned lines just before it as
  context.
- The model rewrites each line into clear English. It drops lines that are only filler and never
  adds, merges or summarises anything.
- The cleaned text is kept **next to** the original, line by line, so the subtitle times still
  fit. The original is never changed.
- Progress arrives as `transcript.cleanup.state`.
- A successful pass replaces an earlier cleaned version. A failed pass leaves it as it was, and
  the error is shown on the transcript. Cleaning up again is always allowed.
- If the server stops during a pass, that pass is marked failed when the server starts again.
- **Cost:** a few cents for an hour-long service.

## Entry points

| Does                     | REST                                                                        | MCP tool                                                                                          |
| ------------------------ | --------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------- |
| List stored transcripts  | `GET /api/transcripts` (`?eventId=`)                                        | `transcripts_list`                                                                                |
| Read one with every line | `GET /api/transcripts/{id}`                                                 | `transcripts_get`                                                                                 |
| Download as a file       | `GET /api/transcripts/{id}/download?format=txt\|srt\|vtt&clean=true\|false` | `transcripts_download`                                                                            |
| Delete one               | `DELETE /api/transcripts/{id}`                                              | `transcripts_delete` (destructive)                                                                |
| Start the AI clean-up    | `POST /api/transcripts/{id}/cleanup`                                        | `transcripts_cleanup` (destructive: it sends the text to OpenAI and replaces the cleaned version) |

WebSocket push: `transcript.cleanup.state` `{ sessionId, state, progress, error }`. `state` is
`running`, `succeeded` or `failed`, and `progress` goes from 0 to 1.

Refusals:

- `409` with `transcript_live` for the live session.
- `409` with `transcript_cleanup_running` while a pass runs.
- `409` with `transcript_empty` for a session with no lines.
- `409` with `transcript_not_cleaned` for `clean=true` before a clean-up has succeeded.
- `400` with `transcription_not_configured` when OpenAI has no key.

Sessions are also deleted along with their event.

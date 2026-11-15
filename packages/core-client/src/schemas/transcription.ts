import { z } from 'zod';

/** Below this the credit line turns red; mirrors `metocast_core::transcription::LOW_CREDIT_USD`. */
export const LOW_CREDIT_USD = 5;

export const TranscriptionStatusSchema = z.enum([
  'notConfigured',
  'unavailable',
  'ready',
  'connecting',
  'running',
  'error',
]);

export const TranscriptionStateSchema = z.object({
  status: TranscriptionStatusSchema,
  /** `mixerOffline` while unavailable; `noCredit`, `invalidKey` or a message on error. */
  reason: z.string().nullable(),
  sessionId: z.string().uuid().nullable(),
  eventId: z.string().uuid().nullable(),
  startedAt: z.string().nullable(),
  youtube: z
    .object({ ok: z.boolean(), lastSeq: z.number().int(), lastError: z.string().nullable() })
    .nullable(),
});

export const TranscriptSegmentSchema = z.object({
  seq: z.number().int(),
  startedAt: z.string(),
  endedAt: z.string(),
  text: z.string(),
});

export const CreditStatusSchema = z.object({
  balanceUsd: z.number(),
  toppedUpUsd: z.number(),
  spentUsd: z.number(),
  asOf: z.string(),
  low: z.boolean(),
});

export const OpenAiVerifyResultSchema = z.object({
  result: z.enum(['ok', 'invalid', 'noAccess']),
  message: z.string(),
});

export const CaptionsJobStateSchema = z.object({
  jobId: z.string().uuid(),
  recordingId: z.string().uuid(),
  state: z.enum(['running', 'succeeded', 'failed']),
  progress: z.number(),
  error: z.string().nullable(),
});

export const CleanupStatusSchema = z.enum(['none', 'running', 'succeeded', 'failed']);

/** One stored live-transcript session, as `GET /api/transcripts` lists it. */
export const StoredTranscriptSchema = z.object({
  id: z.string().uuid(),
  eventId: z.string().uuid().nullable(),
  eventTitle: z.string().nullable(),
  startedAt: z.string(),
  /** Null while it runs, or when the server stopped without closing it. */
  endedAt: z.string().nullable(),
  status: z.string(),
  /** The session the live transcript is writing to now. */
  live: z.boolean(),
  durationSeconds: z.number(),
  segmentCount: z.number().int(),
  cleanupStatus: CleanupStatusSchema,
  cleanupError: z.string().nullable(),
  /** Live captions YouTube accepted, and those that failed or were dropped. */
  captionsDelivered: z.number().int(),
  captionsFailed: z.number().int(),
  /** Why the latest failed caption was not delivered. */
  captionLastError: z.string().nullable(),
  /** The YouTube stream the captions were last posted to; null for a pasted ingestion URL. */
  captionStreamId: z.string().nullable(),
});

export const StoredSegmentSchema = TranscriptSegmentSchema.extend({
  /** The AI clean-up's version; empty when it dropped the line as filler. */
  cleanText: z.string().nullable(),
});

export const TranscriptDetailSchema = z.object({
  transcript: StoredTranscriptSchema,
  segments: z.array(StoredSegmentSchema),
});

export const CleanupProgressSchema = z.object({
  sessionId: z.string().uuid(),
  state: CleanupStatusSchema,
  progress: z.number(),
  error: z.string().nullable(),
});

/** Every `transcription.*` broadcast, for a page that listens to nothing else. */
export const TranscriptMessageSchema = z.discriminatedUnion('type', [
  z.object({ type: z.literal('transcription.state'), state: TranscriptionStateSchema }),
  z.object({ type: z.literal('transcription.partial'), text: z.string() }),
  z.object({ type: z.literal('transcription.segment'), segment: TranscriptSegmentSchema }),
  z.object({
    type: z.literal('transcription.snapshot'),
    state: TranscriptionStateSchema,
    segments: z.array(TranscriptSegmentSchema),
    partial: z.string(),
    credit: CreditStatusSchema.nullable(),
  }),
  z.object({ type: z.literal('transcription.credit'), credit: CreditStatusSchema }),
]);

export type TranscriptionStatus = z.infer<typeof TranscriptionStatusSchema>;
export type TranscriptionState = z.infer<typeof TranscriptionStateSchema>;
export type TranscriptSegment = z.infer<typeof TranscriptSegmentSchema>;
export type CreditStatus = z.infer<typeof CreditStatusSchema>;
export type OpenAiVerifyResult = z.infer<typeof OpenAiVerifyResultSchema>;
export type CaptionsJobState = z.infer<typeof CaptionsJobStateSchema>;
export type TranscriptMessage = z.infer<typeof TranscriptMessageSchema>;
export type CleanupStatus = z.infer<typeof CleanupStatusSchema>;
export type StoredTranscript = z.infer<typeof StoredTranscriptSchema>;
export type StoredSegment = z.infer<typeof StoredSegmentSchema>;
export type TranscriptDetail = z.infer<typeof TranscriptDetailSchema>;
export type CleanupProgress = z.infer<typeof CleanupProgressSchema>;

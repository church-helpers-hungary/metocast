import { z } from 'zod';
import {
  EventSchema,
  EventSummarySchema,
  EventActivitySchema,
  OrderItemSchema,
  OrderPositionSchema,
} from './event.js';
import { RecordingSchema, RecordingWithEventSchema } from './recording.js';
import { UntrackedRecordingSchema } from './untracked-recording.js';
import { QueueSummarySchema } from './queue.js';
import { AudioProgressSchema, ProgramProgressSchema, RenderProgressSchema } from './media.js';
import {
  CaptionsJobStateSchema,
  CleanupProgressSchema,
  TranscriptMessageSchema,
} from './transcription.js';
import { AutomationMessageSchema, RecorderMessageSchema } from './automations.js';
import {
  AtemStateSchema,
  AtemStreamTargetSchema,
  CameraDestinationSchema,
  CameraStreamTargetSchema,
  ConnectorStatusPayloadSchema,
  DiscoveredCameraSchema,
  MiddlecontrolStateSchema,
  RodecasterMuteEventSchema,
  RodecasterProfileSchema,
  RodecasterAudioDiscoverySchema,
  RodecasterAudioRecorderStateSchema,
  RodecasterDeskRecorderSchema,
} from './connectors.js';

// ── OBS Device types ──────────────────────────────────────────────────────────

export const ObsDeviceItemSchema = z.object({
  itemName: z.string(),
  itemValue: z.string(),
});

export const ObsAvailableDevicesSchema = z.object({
  displays: z.array(ObsDeviceItemSchema),
  audioInputs: z.array(ObsDeviceItemSchema),
  audioOutputs: z.array(ObsDeviceItemSchema),
  videoInputs: z.array(ObsDeviceItemSchema),
  captureCards: z.array(ObsDeviceItemSchema),
  scannedAt: z.string(),
});

export const DeviceListenerSchema = z.object({
  id: z.string().uuid(),
  connectorType: z.string(),
  category: z.string(),
  deviceItemValue: z.string(),
  deviceItemName: z.string(),
  friendlyName: z.string(),
  createdAt: z.string(),
  updatedAt: z.string(),
});

export const DeviceListenerStatusSchema = z.object({
  listenerId: z.string().uuid(),
  available: z.boolean(),
  lastChecked: z.string(),
});

export type ObsDeviceItem = z.infer<typeof ObsDeviceItemSchema>;
export type ObsAvailableDevices = z.infer<typeof ObsAvailableDevicesSchema>;
export type DeviceListener = z.infer<typeof DeviceListenerSchema>;
export type DeviceListenerStatus = z.infer<typeof DeviceListenerStatusSchema>;

export const KeynoteStatusSchema = z.object({
  appRunning: z.boolean(),
  slideshowActive: z.boolean(),
  currentSlide: z.number().nullable(),
  totalSlides: z.number().nullable(),
  documentName: z.string().nullable(),
});

/** A run of text in a search result; `hit` marks the words the search matched. */
export const HighlightPartSchema = z.object({
  text: z.string(),
  hit: z.boolean(),
});

export const PptFileSchema = z.object({
  id: z.string(),
  name: z.string(),
  path: z.string(),
  folderId: z.string(),
  /** The file name without its extension. */
  title: z.string(),
  /** The hymn number, from the last "(47)" in the title. */
  number: z.number().int().nullable(),
  /** Why the deck matched; null when nothing was searched for. */
  matchedIn: z.enum(['number', 'title', 'text']).nullable(),
  titleParts: z.array(HighlightPartSchema),
  /** Slide text around the match; empty unless `matchedIn` is `text`. */
  snippetParts: z.array(HighlightPartSchema),
});

export const PptFolderSchema = z.object({
  id: z.string().uuid(),
  path: z.string(),
  name: z.string(),
  sortOrder: z.number().int(),
});

export const CronJobSchema = z.object({
  id: z.string().uuid(),
  name: z.string(),
  cronExpression: z.string(),
  enabled: z.boolean(),
  pullYoutube: z.boolean(),
  autoUpload: z.boolean(),
  createdAt: z.string(),
  updatedAt: z.string(),
});

export const BroadlinkDeviceSchema = z.object({
  id: z.string().uuid(),
  name: z.string(),
  deviceType: z.string(),
  model: z.string().nullable(),
  host: z.string(),
  mac: z.string(),
  isDefault: z.boolean(),
});

export const WsClientInfoSchema = z.object({
  id: z.string().uuid(),
  label: z.string(),
  userAgent: z.string().nullable(),
  hostname: z.string().nullable(),
  connectedAt: z.string(),
  lastPongAt: z.string().nullable(),
  latencyMs: z.number().int().nullable(),
});

export const ParagraphContentSchema = z
  .object({
    // New format: explicit lines split at <a:br>, plus font size from PPTX.
    lines: z.array(z.string()).optional(),
    fontSizePt: z.number().optional(),
    // Legacy format (old binary): single text string per paragraph.
    text: z.string().optional(),
    align: z.string(),
  })
  .transform((p) => ({
    lines: p.lines ?? (p.text !== undefined ? [p.text] : []),
    align: p.align,
    fontSizePt: p.fontSizePt ?? 0,
  }));

export const SlideContentSchema = z.object({
  index: z.number().int().positive(),
  paragraphs: z.array(ParagraphContentSchema),
});

export const PresenterRenderModeSchema = z.enum(['text', 'svg']);
export const PresenterThemeSchema = z.enum(['classic', 'editorial']);

export const SvgSlideContentSchema = z.object({
  index: z.number().int().positive(),
  svg: z.string(),
  widthPx: z.number().int().positive(),
  heightPx: z.number().int().positive(),
});

export const PresenterStateSchema = z.object({
  loaded: z.boolean(),
  filePath: z.string().nullable(),
  currentSlide: z.number().int().nonnegative(),
  totalSlides: z.number().int().nonnegative(),
  renderMode: PresenterRenderModeSchema.default('text'),
  slides: z.array(SlideContentSchema),
  svgSlides: z.array(SvgSlideContentSchema).default([]),
  muted: z.boolean(),
  // Optional: absent from old binaries; defaults to standard 16:9.
  slideWidthEmu: z.number().int().nonnegative().default(12192000),
  slideHeightEmu: z.number().int().nonnegative().default(6858000),
});

export const BroadlinkCommandSchema = z.object({
  id: z.string().uuid(),
  deviceId: z.string().uuid().nullable(),
  name: z.string(),
  slug: z.string(),
  code: z.string(),
  codeType: z.string(),
  category: z.string(),
});

export type KeynoteStatus = z.infer<typeof KeynoteStatusSchema>;
export type PptFile = z.infer<typeof PptFileSchema>;
export type HighlightPart = z.infer<typeof HighlightPartSchema>;
export type PptFolder = z.infer<typeof PptFolderSchema>;
export type ParagraphContent = z.infer<typeof ParagraphContentSchema>;
export type SlideContent = z.infer<typeof SlideContentSchema>;
export type PresenterRenderMode = z.infer<typeof PresenterRenderModeSchema>;
export type PresenterTheme = z.infer<typeof PresenterThemeSchema>;
export type SvgSlideContent = z.infer<typeof SvgSlideContentSchema>;
export type PresenterState = z.infer<typeof PresenterStateSchema>;
export type WsClientInfo = z.infer<typeof WsClientInfoSchema>;
export type CronJob = z.infer<typeof CronJobSchema>;
export type BroadlinkDevice = z.infer<typeof BroadlinkDeviceSchema>;
export type BroadlinkCommand = z.infer<typeof BroadlinkCommandSchema>;

export const MiddlecontrolCommandSchema = z.discriminatedUnion('type', [
  z.object({
    type: z.literal('middlecontrol.camera.select'),
    camera_id: z.number().int().min(1).max(99),
  }),
  z.object({
    type: z.literal('middlecontrol.record.start'),
    camera_id: z.number().int().min(1).max(99).optional(),
  }),
  z.object({
    type: z.literal('middlecontrol.record.stop'),
    camera_id: z.number().int().min(1).max(99).optional(),
  }),
  z.object({ type: z.literal('middlecontrol.record.start_all') }),
  z.object({ type: z.literal('middlecontrol.record.stop_all') }),
  z.object({
    type: z.literal('middlecontrol.preset.recall'),
    preset: z.number().int().min(1).max(12),
    camera_id: z.number().int().min(1).max(99).optional(),
  }),
]);

export type MiddlecontrolCommand = z.infer<typeof MiddlecontrolCommandSchema>;

export const WsMessageSchema = z.discriminatedUnion('type', [
  // ── Core ───────────────────────────────────────────────────────────────────
  z.object({ type: z.literal('connected'), serverId: z.string() }),
  z.object({ type: z.literal('ok') }),
  z.object({ type: z.literal('error'), message: z.string() }),
  z.object({
    type: z.literal('notification'),
    level: z.enum(['info', 'warn', 'error']),
    message: z.string(),
  }),
  // ── Connector push (server → client) ───────────────────────────────────────
  z.object({
    type: z.literal('event.changed'),
    data: z.object({ operation: z.string(), record: EventSchema }),
  }),
  z.object({
    type: z.literal('recording.changed'),
    data: z.object({ operation: z.string(), record: RecordingSchema }),
  }),
  z.object({
    type: z.literal('connector.status'),
    connector: z.enum([
      'obs',
      'vmix',
      'atem',
      'youtube',
      'facebook',
      'discord',
      'broadlink',
      'blackmagic-camera',
      'middlecontrol',
      'rodecaster',
    ]),
    status: ConnectorStatusPayloadSchema,
  }),
  z.discriminatedUnion('connector', [
    z.object({
      type: z.literal('connector.state'),
      connector: z.literal('atem'),
      state: AtemStateSchema,
    }),
    z.object({
      type: z.literal('connector.state'),
      connector: z.enum([
        'obs',
        'vmix',
        'broadlink',
        'youtube',
        'facebook',
        'discord',
        'blackmagic-camera',
        'middlecontrol',
      ]),
      isStreaming: z.boolean().optional(),
      /** The camera's own livestream word, so a client can tell `Connecting` from `Idle`.
       *  Absent for every connector but the Blackmagic camera. */
      streamStatus: z.string().optional(),
      isRecording: z.boolean().optional(),
      /** False with no card or SSD in the Blackmagic camera; absent for every other connector. */
      hasMedia: z.boolean().optional(),
      /** Where the Blackmagic camera streams to; null without livestreaming. */
      destination: CameraDestinationSchema.nullable().optional(),
      state: MiddlecontrolStateSchema.optional(),
    }),
  ]),
  z.object({
    type: z.literal('cron.youtube_pull'),
    hasLive: z.boolean(),
  }),
  z.object({
    type: z.literal('recording.detected'),
    fileName: z.string(),
    eventTitle: z.string().nullable(),
  }),
  z.object({
    type: z.literal('recording.untracked.removed'),
    id: z.string().uuid(),
  }),
  z.object({
    type: z.literal('obs.state'),
    isStreaming: z.boolean(),
    isRecording: z.boolean(),
  }),
  z.object({
    type: z.literal('broadlink.device.discovered'),
    device: z.object({
      name: z.string(),
      host: z.string(),
      mac: z.string(),
      deviceType: z.string(),
      model: z.string().nullable(),
    }),
  }),
  z.object({
    type: z.literal('broadlink.learn.result'),
    code: z.string().nullable(),
    error: z.string().nullable(),
  }),
  z.object({
    type: z.literal('keynote.status'),
    status: KeynoteStatusSchema,
  }),
  z.object({
    type: z.literal('ppt.search_results'),
    files: z.array(PptFileSchema),
    filter: z.string().optional(),
  }),
  z.object({
    type: z.literal('ppt.folders_changed'),
  }),
  z.object({
    type: z.literal('ppt.index.state'),
    /** True while the core parses new or changed decks into the search index. */
    indexing: z.boolean(),
    /** How many decks this run parses; 0 when idle. */
    pending: z.number().int(),
  }),
  z.object({
    type: z.literal('upload.progress'),
    recordingId: z.string().uuid(),
    platform: z.string(),
    progressBytes: z.number().int(),
    totalBytes: z.number().int(),
  }),
  z.object({
    type: z.literal('upload.completed'),
    recordingId: z.string().uuid(),
    platform: z.string(),
    videoId: z.string(),
    videoUrl: z.string(),
  }),
  z.object({
    type: z.literal('upload.failed'),
    recordingId: z.string().uuid(),
    platform: z.string(),
    error: z.string(),
  }),
  z.object({
    type: z.literal('upload.paused'),
    recordingId: z.string().uuid(),
    reason: z.string(),
  }),
  z.object({
    type: z.literal('media.render.progress'),
    progress: RenderProgressSchema,
  }),
  z.object({
    type: z.literal('media.audio.progress'),
    progress: AudioProgressSchema,
  }),
  z.object({
    type: z.literal('media.program.progress'),
    progress: ProgramProgressSchema,
  }),
  // ── Automations ────────────────────────────────────────────────────────────
  ...AutomationMessageSchema.options,
  ...RecorderMessageSchema.options,
  // ── Live transcript and final-video captions ───────────────────────────────
  ...TranscriptMessageSchema.options,
  CaptionsJobStateSchema.extend({ type: z.literal('captions.job.state') }),
  CleanupProgressSchema.extend({ type: z.literal('transcript.cleanup.state') }),
  // ── Events (WS command responses) ──────────────────────────────────────────
  z.object({ type: z.literal('events.list'), events: z.array(EventSummarySchema) }),
  z.object({
    type: z.literal('events.presenter_list'),
    events: z.array(EventSummarySchema),
    selectedEventId: z.string().uuid().nullable(),
  }),
  z.object({ type: z.literal('events.get'), event: EventSchema }),
  z.object({ type: z.literal('events.create'), event: EventSchema }),
  z.object({ type: z.literal('events.update'), event: EventSchema }),
  // ── Recordings (WS command responses) ──────────────────────────────────────
  z.object({ type: z.literal('recordings.list'), recordings: z.array(RecordingSchema) }),
  z.object({
    type: z.literal('recordings.list_all'),
    recordings: z.array(RecordingWithEventSchema),
  }),
  z.object({ type: z.literal('recordings.create'), recording: RecordingSchema }),
  z.object({
    type: z.literal('recordings.untracked.list'),
    recordings: z.array(UntrackedRecordingSchema),
  }),
  z.object({ type: z.literal('recordings.untracked.assign'), recording: RecordingSchema }),
  // ── Activities (WS command responses) ──────────────────────────────────────
  z.object({ type: z.literal('activities.list'), activities: z.array(EventActivitySchema) }),
  z.object({ type: z.literal('activities.create'), activity: EventActivitySchema }),
  // ── Cron jobs (WS command responses) ───────────────────────────────────────
  z.object({ type: z.literal('cron_jobs.list'), jobs: z.array(CronJobSchema) }),
  z.object({ type: z.literal('cron_jobs.create'), job: CronJobSchema }),
  z.object({ type: z.literal('cron_jobs.update'), job: CronJobSchema }),
  // ── PPT (WS command responses) ─────────────────────────────────────────────
  z.object({ type: z.literal('ppt.folders.list'), folders: z.array(PptFolderSchema) }),
  z.object({ type: z.literal('ppt.folders.add'), folder: PptFolderSchema.nullable() }),
  // ── Presentation settings (push + command response) ────────────────────────
  z.object({
    type: z.literal('presentation.settings'),
    useWebPresenter: z.boolean(),
    presenterTheme: PresenterThemeSchema.default('classic'),
  }),
  // ── Presenter (push + command responses) ───────────────────────────────────
  z.object({
    type: z.literal('presenter.state'),
    state: PresenterStateSchema,
    /** True only for a client connected with `/ws?presenter=frames`: `svgSlides[].svg` is empty. */
    svgOmitted: z.boolean().default(false),
  }),
  // ── Order of Service (push + `order.status` reply) ────────────────────────
  z.object({
    type: z.literal('order.state'),
    position: OrderPositionSchema.nullable(),
    /** The running event's steps; empty when no step is on screen. */
    items: z.array(OrderItemSchema).default([]),
  }),
  z.object({
    type: z.literal('event.order.changed'),
    eventId: z.string().uuid(),
    items: z.array(OrderItemSchema),
  }),
  z.object({
    type: z.literal('presenter.slide_changed'),
    currentSlide: z.number().int().nonnegative(),
    totalSlides: z.number().int().nonnegative(),
  }),
  // ── Connected clients ───────────────────────────────────────────────────────
  z.object({ type: z.literal('clients.updated'), clients: z.array(WsClientInfoSchema) }),
  z.object({ type: z.literal('clients.list'), clients: z.array(WsClientInfoSchema) }),
  z.object({ type: z.literal('ping'), pingId: z.number().int() }),
  // ── Connectors (WS command responses) ──────────────────────────────────────
  z.object({
    type: z.literal('connectors.status'),
    obs: ConnectorStatusPayloadSchema,
    vmix: ConnectorStatusPayloadSchema,
    broadlink: ConnectorStatusPayloadSchema,
    youtube: ConnectorStatusPayloadSchema,
    facebook: ConnectorStatusPayloadSchema,
    'blackmagic-camera': ConnectorStatusPayloadSchema,
    middlecontrol: ConnectorStatusPayloadSchema,
    rodecaster: ConnectorStatusPayloadSchema,
    atem: ConnectorStatusPayloadSchema,
  }),
  z.object({
    type: z.literal('connectors.state'),
    obs: z.object({ isStreaming: z.boolean(), isRecording: z.boolean() }).nullable(),
    'blackmagic-camera': z
      .object({
        isStreaming: z.boolean(),
        streamStatus: z.string().default('Idle'),
        isRecording: z.boolean(),
        hasMedia: z.boolean().default(true),
        destination: CameraDestinationSchema.nullable().default(null),
      })
      .nullable(),
    middlecontrol: MiddlecontrolStateSchema.nullable(),
    atem: AtemStateSchema.nullable(),
  }),
  z.object({ type: z.literal('connectors.youtube.stream_key'), rtmpUrl: z.string() }),
  z.object({ type: z.literal('connectors.facebook.stream_key'), rtmpUrl: z.string() }),
  z.object({
    type: z.literal('connectors.youtube.content'),
    content: z.object({
      liveBroadcasts: z.array(
        z.object({
          id: z.string(),
          title: z.string(),
          thumbnailUrl: z.string(),
          publishedAt: z.string().nullable(),
          viewCount: z.number().nonnegative().nullable(),
          likeCount: z.number().nonnegative().nullable(),
          duration: z.string().nullable(),
          liveStatus: z.string(),
          scheduledStartTime: z.string().nullable(),
          watchUrl: z.string(),
          privacyStatus: z.string(),
        }),
      ),
      videos: z.array(
        z.object({
          id: z.string(),
          title: z.string(),
          thumbnailUrl: z.string(),
          publishedAt: z.string().nullable(),
          viewCount: z.number().nonnegative().nullable(),
          likeCount: z.number().nonnegative().nullable(),
          duration: z.string().nullable(),
          liveStatus: z.string(),
          scheduledStartTime: z.string().nullable(),
          watchUrl: z.string(),
          privacyStatus: z.string(),
        }),
      ),
    }),
  }),
  // ── Auth (WS command responses) ────────────────────────────────────────────
  z.object({ type: z.literal('auth.youtube.url'), url: z.string() }),
  z.object({ type: z.literal('auth.facebook.url'), url: z.string() }),
  // ── Broadlink (WS command responses) ───────────────────────────────────────
  z.object({ type: z.literal('broadlink.status'), status: ConnectorStatusPayloadSchema }),
  z.object({ type: z.literal('broadlink.devices.list'), devices: z.array(BroadlinkDeviceSchema) }),
  z.object({ type: z.literal('broadlink.devices.add'), device: BroadlinkDeviceSchema }),
  z.object({
    type: z.literal('broadlink.commands.list'),
    commands: z.array(BroadlinkCommandSchema),
  }),
  z.object({ type: z.literal('broadlink.commands.add'), command: BroadlinkCommandSchema }),
  z.object({ type: z.literal('broadlink.commands.update'), command: BroadlinkCommandSchema }),
  // ── Blackmagic camera (WS command responses) ───────────────────────────────
  z.object({
    type: z.literal('blackmagic-camera.discovered'),
    cameras: z.array(DiscoveredCameraSchema),
  }),
  CameraStreamTargetSchema.extend({
    type: z.literal('blackmagic-camera.stream.platform'),
  }),
  AtemStreamTargetSchema.extend({ type: z.literal('atem.stream.destination') }),
  // ── RØDECaster ──────────────────────────────────────────────────────────────
  z.object({ type: z.literal('rodecaster.profile'), profile: RodecasterProfileSchema }),
  RodecasterMuteEventSchema.extend({ type: z.literal('rodecaster.mute') }),
  z.object({
    type: z.literal('rodecaster.audio.discovery'),
    discovery: RodecasterAudioDiscoverySchema,
  }),
  z.object({
    type: z.literal('rodecaster.audio.record.state'),
    state: RodecasterAudioRecorderStateSchema,
  }),
  /** The desk's own SD/USB recorder, as opposed to Metocast's audio recording above. */
  z.object({
    type: z.literal('rodecaster.record.state'),
    state: RodecasterDeskRecorderSchema,
  }),
  // ── OBS Devices ────────────────────────────────────────────────────────────
  z.object({
    type: z.literal('obs.devices.available'),
    devices: ObsAvailableDevicesSchema,
    listenerStatuses: z.array(DeviceListenerStatusSchema),
  }),
  z.object({
    type: z.literal('obs.listeners.list'),
    listeners: z.array(DeviceListenerSchema),
    statuses: z.array(DeviceListenerStatusSchema),
  }),
  z.object({ type: z.literal('obs.listeners.create'), listener: DeviceListenerSchema }),
  z.object({ type: z.literal('obs.listeners.update'), listener: DeviceListenerSchema }),
  z.object({ type: z.literal('obs.listeners.delete'), id: z.string().uuid() }),
  // ── Job queue ──────────────────────────────────────────────────────────────
  z.object({ type: z.literal('queue.stats'), queues: z.array(QueueSummarySchema) }),
]);

export type WsMessage = z.infer<typeof WsMessageSchema>;

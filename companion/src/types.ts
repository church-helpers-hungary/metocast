import { z } from 'zod'

export interface ModuleConfig {
	host: string
	port: number
	authToken: string
	pollInterval: number
}

export interface RfIrCommand {
	id: string
	name: string
	slug: string
	deviceId: string | null
	code: string
	codeType: string
	category: 'projector' | 'screen' | 'hvac' | 'lighting' | 'audio' | 'other'
}

export interface BroadlinkDevice {
	id: string
	name: string
	type: string
	model: string
	host: string
	mac: string
}

// PPT Selector Types
export interface PptFolder {
	id: string
	path: string
	name: string
}

export interface HighlightPart {
	text: string
	hit: boolean
}

export interface PptFile {
	id: string
	name: string
	path: string
	folderId: string
	/** The file name without its extension; absent from servers before the search index. */
	title?: string
	/** The hymn number, from the last "(47)" in the title. */
	number?: number | null
	/** Why the deck matched; null when nothing was searched for. */
	matchedIn?: 'number' | 'title' | 'text' | null
	titleParts?: HighlightPart[]
	snippetParts?: HighlightPart[]
}

export interface PptFilesResponse {
	files: PptFile[]
	total: number
	filter: string | null
}

export interface PptFoldersResponse {
	folders: PptFolder[]
}

export interface PptOpenResponse {
	success: boolean
	fileName: string
	presenterStarted: boolean
}

// Keynote Status (from backend WS)
export interface KeynoteStatus {
	appRunning: boolean
	slideshowActive: boolean
	currentSlide: number | null
	totalSlides: number | null
	documentName: string | null
}

// Presentation Control Types
export interface PresentationStatus {
	app: string | null
	appRunning: boolean
	slideshowActive: boolean
	currentSlide: number | null
	totalSlides: number | null
	currentSlideTitle: string | null
	blanked: boolean
}

export const PresentationSettingsMessageSchema = z.object({
	type: z.literal('presentation.settings'),
	useWebPresenter: z.boolean(),
	presenterTheme: z.enum(['classic', 'editorial']).default('classic'),
	deliveryLog: z.boolean().default(false),
})

/** `order.state`: the Order of Service step on screen, with its event's steps. */
export const OrderStateMessageSchema = z.object({
	type: z.literal('order.state'),
	position: z.object({ eventId: z.string(), itemId: z.string() }).nullable(),
	items: z.array(z.object({ id: z.string().nullish(), kind: z.string(), title: z.string().default('') })).default([]),
})

/** The step on screen, as the module shows it. */
export interface OrderStep {
	title: string
	/** The step after it, so the operator sees what comes next; empty on the last step. */
	next: string
	number: number
	count: number
}

export const EventSummarySchema = z.object({
	id: z.string().uuid(),
	title: z.string(),
	dateTime: z.string(),
	speaker: z.string(),
	recordingCount: z.number().int().nonnegative(),
	isCompleted: z.boolean(),
	/** Its broadcasts; `scheduleStatus` is `scheduled`, `starting`, `live` or `ended` once one exists. */
	connections: z.array(z.object({ platform: z.string(), scheduleStatus: z.string() })).default([]),
	createdAt: z.string(),
	updatedAt: z.string(),
})

export const PresenterEventListResponseSchema = z.object({
	type: z.literal('events.presenter_list'),
	events: z.array(EventSummarySchema),
	selectedEventId: z.string().uuid().nullable(),
})

/** A rule as Companion needs it: to name it and to run it. The rest of the rule is ignored. */
export const AutomationSummarySchema = z.object({
	id: z.string().uuid(),
	name: z.string(),
	enabled: z.boolean(),
})

/** Sent on connect, after any change, and in answer to `automations.list`. */
export const AutomationsChangedMessageSchema = z.object({
	type: z.literal('automations.changed'),
	armed: z.boolean(),
	automations: z.array(AutomationSummarySchema),
})

export const AutomationRunMessageSchema = z.object({
	type: z.literal('automation.run'),
	run: z.object({
		name: z.string(),
		manual: z.boolean(),
		results: z.array(z.object({ outcome: z.enum(['done', 'skipped', 'failed']) })),
	}),
})

/** The RØDECaster's own SD/USB recorder, not Metocast's FLAC recording of its USB audio. */
export const DeskRecorderMessageSchema = z.object({
	type: z.literal('rodecaster.record.state'),
	state: z.object({
		status: z.enum(['stopped', 'paused', 'recording']),
		mediaPresent: z.boolean(),
	}),
})

/** The Record button's recorders; only whether the selected ones record matters here. */
export const RecordersStateMessageSchema = z.object({
	type: z.literal('recorders.state'),
	recorders: z.array(z.object({ selected: z.boolean(), recording: z.boolean() })),
})

export const RecorderStoppedMessageSchema = z.object({
	type: z.literal('recorder.stopped'),
	target: z.object({ device: z.string() }),
	reason: z.enum(['metocast', 'disconnected', 'device']),
	error: z.string().nullable(),
})

export type AutomationSummary = z.infer<typeof AutomationSummarySchema>
export type EventSummary = z.infer<typeof EventSummarySchema>
export type PresenterEventListResponse = z.infer<typeof PresenterEventListResponseSchema>
export type BibleReferenceType = 'textus' | 'leckio'

export const CATEGORY_COLORS: Record<string, number> = {
	projector: 0xdd614a,
	screen: 0xf48668,
	hvac: 0xf4a698,
	lighting: 0xc5c392,
	audio: 0x73a580,
	other: 0x808080,
}

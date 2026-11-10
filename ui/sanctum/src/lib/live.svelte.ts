import { goto } from '$app/navigation';
import { resolve } from '$app/paths';
import { get } from 'svelte/store';
import { _ } from 'svelte-i18n';
import {
  sendWsCommand,
  connectObs,
  connectCamera,
  youtubeAuthUrl,
  facebookAuthUrl,
  openExternal,
} from '@metocast/core-client';
import type {
  AtemState,
  MiddlecontrolState,
  RodecasterAudioDiscovery,
  RodecasterAudioRecorderState,
  RodecasterProfile,
  WsMessage,
} from '@metocast/core-client';
import type {
  PresenterState,
  PresenterTheme,
  KeynoteStatus,
  WsClientInfo,
  PptFile,
} from '@metocast/core-client/schemas/ws-messages';
import type { EventSummary, OrderPosition } from '@metocast/core-client/schemas/event';
import type { CleanupProgress, CreditStatus, TranscriptionState } from '@metocast/core-client';
import { siObsstudio, siYoutube, siFacebook, siBlackmagicdesign, siDiscord } from './brand-marks';
import { rode } from './rode';
import { notify, resolveByKey } from './notifications.svelte';

/** Translates outside a component: notification text is built here, not in markup.
 *  Wording is resolved when the notification is raised, so a card already on screen
 *  keeps the language it was written in until its connector reports again. */
const t = (key: string, values?: Record<string, string | number>): string =>
  get(_)(key, values ? { values } : undefined);

/** A status the core states as a stable code (`login_required`) becomes real prose;
 *  a driver's own error text has spaces or capitals, so it is never looked up and
 *  reaches the operator exactly as the device worded it. */
function statusText(message: string | undefined): string | undefined {
  if (!message || !/^[a-z][a-z0-9_]*$/.test(message)) return message;
  const key = `conn.status.${message}`;
  const translated = t(key);
  return translated === key ? message : translated;
}

/** Display name, kind label and brand mark per connector — matches the reference spec. */
const CONNECTOR_META: Record<
  string,
  { name: string; kind: string; brand?: string; brandBox?: string }
> = {
  obs: { name: 'OBS Studio', kind: 'notif.kind.encoder', brand: siObsstudio.path },
  youtube: { name: 'YouTube', kind: 'notif.kind.streaming', brand: siYoutube.path },
  facebook: { name: 'Facebook Live', kind: 'notif.kind.streaming', brand: siFacebook.path },
  'blackmagic-camera': {
    name: 'Blackmagic Camera',
    kind: 'notif.kind.camera',
    brand: siBlackmagicdesign.path,
  },
  atem: { name: 'Blackmagic ATEM', kind: 'notif.kind.switcher', brand: siBlackmagicdesign.path },
  rodecaster: {
    name: 'RØDECaster Pro II',
    kind: 'notif.kind.mixer',
    brand: rode.path,
    brandBox: rode.viewBox,
  },
  middlecontrol: { name: 'Middle Control', kind: 'notif.kind.cameraControl' },
  discord: { name: 'Discord', kind: 'notif.kind.webhooks', brand: siDiscord.path },
};

/** How a failing connector is put right: the settings row to open, plus the one-click
 *  recovery it supports — a reconnect (the core dials out again) or an OAuth re-login. */
const RECOVERY: Record<
  string,
  {
    open: '' | `?open=${string}`;
    reconnect?: () => Promise<unknown>;
    login?: () => Promise<string>;
  }
> = {
  obs: { open: '', reconnect: connectObs },
  'blackmagic-camera': { open: '?open=blackmagic-camera', reconnect: connectCamera },
  middlecontrol: { open: '?open=middlecontrol' },
  atem: { open: '?open=atem' },
  rodecaster: { open: '?open=rodecaster' },
  youtube: { open: '?open=youtube', login: youtubeAuthUrl },
  facebook: { open: '?open=facebook', login: facebookAuthUrl },
};

const REMEDIATION = (name: string) => [
  t('notif.remediation.open'),
  t('notif.remediation.check', { name }),
  t('notif.remediation.enable'),
];

// Sanctum's realtime state, in runes. The transport (core-client) validates every
// message; this module is the only place that decides which piece of UI state each
// one updates — the same role classic's ws-bindings plays for its stores.

type ConnState = 'disconnected' | 'connecting' | 'connected' | 'error';

let obsStreaming = $state(false);
let obsRecording = $state(false);
let connectorStatus = $state<Record<string, ConnState>>({});
let cameraStreaming = $state(false);
let cameraRecording = $state(false);
/** The camera's own word — `Connecting` and `Flushing` are the transitions the
 *  camera screen shows as busy, which a bool cannot express. */
let cameraStreamStatus = $state('Idle');
let presenter = $state<PresenterState | null>(null);
let keynote = $state<KeynoteStatus | null>(null);
let clients = $state<WsClientInfo[]>([]);
let pptResults = $state<PptFile[]>([]);
let useWebPresenter = $state(true);
let presenterTheme = $state<PresenterTheme>('classic');
let deliveryLog = $state(false);
let rodecasterProfile = $state<RodecasterProfile | null>(null);
let rodecasterAudioDiscovery = $state<RodecasterAudioDiscovery | null>(null);
let rodecasterAudioRecorder = $state<RodecasterAudioRecorderState | null>(null);
let middlecontrolState = $state<MiddlecontrolState | null>(null);
let atemState = $state<AtemState | null>(null);
let transcription = $state<TranscriptionState | null>(null);
/** The open line, else the last committed one. */
let transcriptLine = $state('');
let transcriptCredit = $state<CreditStatus | null>(null);
/** The event the core says is happening now — nobody in the UI decides this. */
let currentEvent = $state<EventSummary | null>(null);
/** The Order of Service step on screen, the same on every client. */
let orderPosition = $state<OrderPosition | null>(null);
/** Bumped when any event's Order of Service changes, so open views reload it. */
let orderRevision = $state(0);
// Bumped on every `event.changed`, so a list re-reads its events (a broadcast went live, …).
let eventsRevision = $state(0);
/** Decks the core is parsing into the slide search index; 0 when idle. */
let slideIndexPending = $state(0);
/** Bumped when the core finishes indexing, so open deck lists search again. */
let slideIndexRevision = $state(0);
let transcriptCleanup = $state<CleanupProgress | null>(null);

export const live = {
  get streaming() {
    return obsStreaming;
  },
  get recording() {
    return obsRecording;
  },
  get connectorStatus() {
    return connectorStatus;
  },
  get cameraStreaming() {
    return cameraStreaming;
  },
  get cameraRecording() {
    return cameraRecording;
  },
  get cameraStreamStatus() {
    return cameraStreamStatus;
  },
  get presenter() {
    return presenter;
  },
  get keynote() {
    return keynote;
  },
  get clients() {
    return clients;
  },
  get pptResults() {
    return pptResults;
  },
  get useWebPresenter() {
    return useWebPresenter;
  },
  get presenterTheme() {
    return presenterTheme;
  },
  get deliveryLog() {
    return deliveryLog;
  },
  get rodecasterProfile() {
    return rodecasterProfile;
  },
  get rodecasterAudioDiscovery() {
    return rodecasterAudioDiscovery;
  },
  get rodecasterAudioRecorder() {
    return rodecasterAudioRecorder;
  },
  get middlecontrolState() {
    return middlecontrolState;
  },
  get atemState() {
    return atemState;
  },
  get currentEvent() {
    return currentEvent;
  },
  get orderPosition() {
    return orderPosition;
  },
  get orderRevision() {
    return orderRevision;
  },
  get eventsRevision() {
    return eventsRevision;
  },
  get slideIndexPending() {
    return slideIndexPending;
  },
  get slideIndexRevision() {
    return slideIndexRevision;
  },
  get transcription() {
    return transcription;
  },
  get transcriptLine() {
    return transcriptLine;
  },
  /** The latest `transcript.cleanup.state`, for the transcript library. */
  get transcriptCleanup() {
    return transcriptCleanup;
  },
  get transcriptCredit() {
    return transcriptCredit;
  },
};

/** Names a connector for a notification, without inventing prose the core didn't send. */
function sourceName(id: string): string {
  return id.charAt(0).toUpperCase() + id.slice(1);
}

export function handleWs(msg: WsMessage): void {
  switch (msg.type) {
    case 'obs.state':
      obsStreaming = msg.isStreaming;
      obsRecording = msg.isRecording;
      break;
    case 'connector.state':
      if (msg.connector === 'obs') {
        if (msg.isStreaming !== undefined) obsStreaming = msg.isStreaming;
        if (msg.isRecording !== undefined) obsRecording = msg.isRecording;
      } else if (msg.connector === 'blackmagic-camera') {
        if (msg.isStreaming !== undefined) cameraStreaming = msg.isStreaming;
        if (msg.isRecording !== undefined) cameraRecording = msg.isRecording;
        if (msg.streamStatus !== undefined) cameraStreamStatus = msg.streamStatus;
      } else if (msg.connector === 'middlecontrol' && msg.state) {
        middlecontrolState = msg.state;
      } else if (msg.connector === 'atem') {
        atemState = msg.state;
      }
      break;
    case 'connectors.state':
      obsStreaming = msg.obs?.isStreaming ?? false;
      obsRecording = msg.obs?.isRecording ?? false;
      cameraStreaming = msg['blackmagic-camera']?.isStreaming ?? false;
      cameraRecording = msg['blackmagic-camera']?.isRecording ?? false;
      cameraStreamStatus = msg['blackmagic-camera']?.streamStatus ?? 'Idle';
      middlecontrolState = msg.middlecontrol;
      atemState = msg.atem;
      break;
    case 'connectors.status':
      connectorStatus = {
        obs: msg.obs.type,
        vmix: msg.vmix.type,
        broadlink: msg.broadlink.type,
        youtube: msg.youtube.type,
        facebook: msg.facebook.type,
        'blackmagic-camera': msg['blackmagic-camera'].type,
        middlecontrol: msg.middlecontrol.type,
        rodecaster: msg.rodecaster.type,
        atem: msg.atem.type,
      };
      break;
    case 'connector.status': {
      const prev = connectorStatus[msg.connector];
      const next = msg.status.type;
      connectorStatus = { ...connectorStatus, [msg.connector]: next };
      if (msg.connector === 'middlecontrol' && next !== 'connected') middlecontrolState = null;
      if (msg.connector === 'atem' && next !== 'connected') atemState = null;
      const key = `connector:${msg.connector}`;
      const meta = CONNECTOR_META[msg.connector] ?? {
        name: sourceName(msg.connector),
        kind: 'notif.kind.connector',
      };
      const kind = t(meta.kind);
      const recovery = RECOVERY[msg.connector];
      // Navigation is instant, so it stays a plain action — only work that takes
      // time (a reconnect, a re-login) returns its promise and animates.
      const editFor = recovery
        ? {
            label: t('notif.action.edit'),
            run: () => void goto(resolve(`/settings/connectors${recovery.open}`)),
          }
        : null;
      // The backend restart only reports a status edge the card can already be sitting
      // on, so the click paints the reconnecting chip itself rather than waiting for one.
      const showReconnecting = (body?: string) =>
        notify({
          tier: 'warn',
          issue: true,
          kind,
          source: meta.name,
          title: t('notif.conn.disconnected', { name: meta.name }),
          body,
          state: t('notif.conn.reconnecting'),
          brand: meta.brand,
          brandBox: meta.brandBox,
          actions: editFor ? [editFor] : undefined,
          remediation: REMEDIATION(meta.name),
          key,
        });
      const reconnect = (start: () => Promise<unknown>) => () => {
        showReconnecting();
        return start();
      };
      const login = recovery?.login;
      const retry = recovery?.reconnect
        ? { label: t('notif.action.reconnect'), primary: true, run: reconnect(recovery.reconnect) }
        : login
          ? {
              label: t('notif.action.relogin'),
              primary: true,
              run: () => login().then(openExternal),
            }
          : null;
      const actions = retry && editFor ? [retry, editFor] : undefined;
      if (next === 'error') {
        notify({
          tier: 'error',
          issue: true,
          kind,
          source: meta.name,
          title: t('notif.conn.disconnected', { name: meta.name }),
          body: statusText(msg.status.message),
          state: 'error',
          brand: meta.brand,
          brandBox: meta.brandBox,
          actions,
          remediation: REMEDIATION(meta.name),
          key,
        });
      } else if (next === 'connecting' && prev === 'error') {
        // Already retrying, so a Reconnect button here would just repeat the chip.
        showReconnecting(statusText(msg.status.message));
      } else if (next === 'connected' && (prev === 'error' || prev === 'connecting')) {
        resolveByKey(key, {
          kind,
          source: meta.name,
          title: t('notif.conn.reconnected', { name: meta.name }),
        });
      }
      break;
    }
    case 'rodecaster.profile':
      rodecasterProfile = msg.profile;
      break;
    case 'rodecaster.mute':
      if (rodecasterProfile) {
        rodecasterProfile = {
          ...rodecasterProfile,
          channels: rodecasterProfile.channels.map((c) =>
            c.channel === msg.channel
              ? msg.remote
                ? { ...c, wirelessMute: msg.muted }
                : { ...c, mute: msg.muted }
              : c,
          ),
        };
      }
      if (msg.notify) {
        notify({
          tier: msg.remote && msg.muted ? 'warn' : 'ok',
          kind: t('notif.kind.mixer'),
          source: 'RØDECaster Pro II',
          brand: rode.path,
          brandBox: rode.viewBox,
          title: t(
            msg.remote
              ? msg.muted
                ? 'notif.mute.remoteMuted'
                : 'notif.mute.remoteUnmuted'
              : msg.muted
                ? 'notif.mute.muted'
                : 'notif.mute.unmuted',
            { label: msg.label },
          ),
          key: `rodecaster:${msg.remote ? 'remote-' : ''}mute:${msg.channel}`,
        });
      }
      break;
    case 'events.presenter_list':
      currentEvent = msg.events.find((e) => e.id === msg.selectedEventId) ?? null;
      break;
    case 'event.changed':
      eventsRevision += 1;
      sendWsCommand('events.presenter_list');
      break;
    case 'order.state':
      orderPosition = msg.position;
      break;
    case 'event.order.changed':
      orderRevision += 1;
      break;
    case 'ppt.index.state':
      if (slideIndexPending > 0 && !msg.indexing) slideIndexRevision += 1;
      slideIndexPending = msg.pending;
      break;
    case 'rodecaster.audio.discovery':
      rodecasterAudioDiscovery = msg.discovery;
      break;
    case 'rodecaster.audio.record.state':
      rodecasterAudioRecorder = msg.state;
      break;
    case 'transcription.snapshot':
      transcription = msg.state;
      transcriptLine = msg.partial || (msg.segments.at(-1)?.text ?? '');
      transcriptCredit = msg.credit;
      break;
    case 'transcription.state':
      transcription = msg.state;
      break;
    case 'transcription.partial':
      if (msg.text) transcriptLine = msg.text;
      break;
    case 'transcription.segment':
      transcriptLine = msg.segment.text;
      break;
    case 'transcription.credit':
      transcriptCredit = msg.credit;
      break;
    case 'transcript.cleanup.state':
      transcriptCleanup = msg;
      break;
    case 'notification':
      notify({
        tier: msg.level === 'error' ? 'error' : msg.level === 'warn' ? 'warn' : 'ok',
        kind: t('notif.kind.system'),
        source: t('notif.core.source'),
        title: msg.message,
      });
      break;
    case 'error':
      // This channel carries whatever a command failed with, camera error paths and
      // status codes alike — often a raw string like ".../livestreams/0/stop". Keep it
      // out of the serif title (a "0" there reads as "o") and mono in the body instead.
      // Keyed on the text so a repeated refusal updates its card instead of stacking.
      notify({
        tier: 'error',
        kind: t('notif.kind.system'),
        source: t('notif.core.source'),
        title: t('notif.core.commandFailed'),
        body: statusText(msg.message) ?? msg.message,
        mono: true,
        key: `error:${msg.message}`,
      });
      break;
    case 'presentation.settings':
      useWebPresenter = msg.useWebPresenter;
      presenterTheme = msg.presenterTheme;
      deliveryLog = msg.deliveryLog;
      break;
    case 'presenter.state':
      presenter = msg.state;
      break;
    case 'presenter.slide_changed':
      if (presenter)
        presenter = { ...presenter, currentSlide: msg.currentSlide, totalSlides: msg.totalSlides };
      break;
    case 'keynote.status':
      keynote = msg.status;
      break;
    case 'clients.updated':
    case 'clients.list':
      clients = msg.clients;
      break;
    case 'ppt.search_results':
      pptResults = msg.files;
      break;
    case 'ping':
      sendWsCommand('pong', { ping_id: msg.pingId });
      break;
  }
}

/** Called once the socket opens: identify this window and pull current realtime state. */
export function registerLive(): void {
  sendWsCommand('presenter.register', { label: 'Sanctum' });
  sendWsCommand('clients.list');
  sendWsCommand('connectors.status');
  sendWsCommand('events.presenter_list');
}

<script lang="ts">
  // The transcript library: every stored live transcript, read, downloaded, cleaned up with AI or
  // deleted. Clean-up progress arrives over the socket as `transcript.cleanup.state`.
  import { onMount } from 'svelte';
  import { _ } from 'svelte-i18n';
  import {
    PageHeader,
    List,
    Row,
    Segmented,
    Button,
    Spinner,
    ErrorState,
    EmptyState,
    Dialog,
  } from '@metocast/design-system';
  import {
    listTranscripts,
    getTranscript,
    deleteTranscript,
    downloadTranscript,
    cleanupTranscript,
    type StoredTranscript,
    type TranscriptDetail,
    type TranscriptFormat,
  } from '@metocast/core-client';
  import { live } from '$lib/live.svelte';
  import { pushToast } from '$lib/notifications.svelte';

  let transcripts = $state<StoredTranscript[]>([]);
  let open = $state<TranscriptDetail | null>(null);
  let clean = $state(false);
  let loading = $state(true);
  let failure = $state('');
  let confirmOpen = $state(false);

  const cleanup = $derived(
    open && live.transcriptCleanup?.sessionId === open.transcript.id
      ? live.transcriptCleanup
      : null,
  );
  const lines = $derived(
    (open?.segments ?? [])
      .map((s) => ({ seq: s.seq, text: clean ? (s.cleanText ?? s.text) : s.text }))
      .filter((l) => l.text.trim() !== ''),
  );

  function when(t: StoredTranscript): string {
    const minutes = Math.round(t.durationSeconds / 60);
    return `${new Date(t.startedAt).toLocaleString()} · ${minutes} min · ${$_('transcripts.lines', { values: { count: t.segmentCount } })}`;
  }

  function fail(e: unknown) {
    pushToast({
      kind: $_('transcripts.title'),
      source: 'Metocast',
      title: $_('transcripts.failed'),
      body: String(e),
      tone: 'error',
    });
  }

  async function load() {
    loading = true;
    failure = '';
    try {
      transcripts = await listTranscripts();
    } catch (e) {
      failure = String(e);
    } finally {
      loading = false;
    }
  }

  async function show(id: string) {
    try {
      open = await getTranscript(id);
      clean = open.transcript.cleanupStatus === 'succeeded';
    } catch (e) {
      fail(e);
    }
  }

  async function download(format: TranscriptFormat) {
    if (!open) return;
    try {
      const text = await downloadTranscript(open.transcript.id, format, clean);
      const url = URL.createObjectURL(new Blob([text], { type: 'text/plain' }));
      const date = open.transcript.startedAt.slice(0, 16).replace(/[T:]/g, '-');
      Object.assign(document.createElement('a'), {
        href: url,
        // English, as the library is: Metocast's Media page reads `.en` from the name.
        download: `transcript-${date}${clean ? '-clean' : ''}.en.${format}`,
      }).click();
      URL.revokeObjectURL(url);
    } catch (e) {
      fail(e);
    }
  }

  async function startCleanup() {
    if (!open) return;
    try {
      await cleanupTranscript(open.transcript.id);
      open = { ...open, transcript: { ...open.transcript, cleanupStatus: 'running' } };
    } catch (e) {
      fail(e);
    }
  }

  async function remove() {
    confirmOpen = false;
    if (!open) return;
    try {
      await deleteTranscript(open.transcript.id);
      open = null;
      pushToast({
        kind: $_('transcripts.title'),
        source: 'Metocast',
        title: $_('transcripts.deleted'),
        tone: 'ok',
      });
      await load();
    } catch (e) {
      fail(e);
    }
  }

  // A finished clean-up re-reads the open transcript and the list.
  $effect(() => {
    const state = cleanup?.state;
    if (state === 'succeeded' || state === 'failed') {
      const id = cleanup?.sessionId;
      if (id) void show(id);
      void load();
    }
  });

  onMount(load);
</script>

{#if open}
  {@const t = open.transcript}
  <PageHeader
    title={t.eventTitle ?? $_('transcripts.noEvent')}
    back={{ label: $_('transcripts.title'), onclick: () => (open = null) }}
  />
  <p class="meta">{when(t)}</p>
  <div class="toolbar">
    {#if t.cleanupStatus === 'succeeded'}
      <Segmented
        compact
        label={$_('transcripts.clean')}
        value={clean ? 'clean' : 'raw'}
        options={[
          { value: 'raw', label: $_('transcripts.raw') },
          { value: 'clean', label: $_('transcripts.clean') },
        ]}
        onchange={(value) => (clean = value === 'clean')}
      />
    {/if}
    <div class="actions">
      {#each ['txt', 'srt', 'vtt'] as const as format (format)}
        <Button variant="secondary" compact onclick={() => download(format)}>
          {$_('transcripts.download', { values: { format: format.toUpperCase() } })}
        </Button>
      {/each}
      {#if !t.live}
        <Button
          variant="secondary"
          compact
          loading={t.cleanupStatus === 'running'}
          loadingLabel={$_('transcripts.cleaning', {
            values: { percent: Math.round((cleanup?.progress ?? 0) * 100) },
          })}
          onclick={startCleanup}
        >
          {t.cleanupStatus === 'succeeded'
            ? $_('transcripts.cleanupAgain')
            : $_('transcripts.cleanup')}
        </Button>
        <Button
          variant="danger"
          compact
          disabled={t.cleanupStatus === 'running'}
          onclick={() => (confirmOpen = true)}>{$_('transcripts.delete')}</Button
        >
      {/if}
    </div>
  </div>
  {#if t.cleanupStatus === 'running'}
    <p class="meta">
      {$_('transcripts.cleaning', {
        values: { percent: Math.round((cleanup?.progress ?? 0) * 100) },
      })}
    </p>
  {:else if t.cleanupStatus === 'failed' && t.cleanupError}
    <p class="meta error">
      {$_('transcripts.cleanupFailed', { values: { error: t.cleanupError } })}
    </p>
  {:else if t.cleanupStatus === 'none' && !t.live}
    <p class="meta">{$_('transcripts.cleanupHint')}</p>
  {/if}
  {#if t.captionsDelivered + t.captionsFailed > 0}
    <p class="meta">
      {$_('transcripts.captions', {
        values: { delivered: t.captionsDelivered, failed: t.captionsFailed },
      })}{#if t.captionStreamId}
        · {$_('transcripts.captionStream', { values: { stream: t.captionStreamId } })}{/if}
    </p>
    {#if t.captionsFailed > 0 && t.captionLastError}
      <p class="meta error">
        {$_('transcripts.captionError', { values: { error: t.captionLastError } })}
      </p>
    {/if}
  {/if}
  <div class="surface">
    {#each lines as line (line.seq)}
      <p>{line.text}</p>
    {/each}
  </div>
{:else}
  <PageHeader title={$_('transcripts.title')} back={{ label: $_('transcripts.back'), href: '/' }} />
  {#if loading && transcripts.length === 0}
    <div class="centre"><Spinner label={$_('transcripts.loading')} /></div>
  {:else if failure}
    <ErrorState
      title={$_('transcripts.unavailable')}
      body={failure}
      retryLabel={$_('transcripts.retry')}
      onretry={load}
    />
  {:else if transcripts.length === 0}
    <EmptyState title={$_('transcripts.empty')} hint={$_('transcripts.emptyHint')} />
  {:else}
    <List>
      {#each transcripts as t, i (t.id)}
        <Row
          title={t.eventTitle ?? $_('transcripts.noEvent')}
          meta={when(t)}
          detail={t.live
            ? $_('transcripts.live')
            : t.cleanupStatus === 'succeeded'
              ? $_('transcripts.clean')
              : ''}
          last={i === transcripts.length - 1}
          onclick={() => show(t.id)}
        />
      {/each}
    </List>
  {/if}
{/if}

<Dialog bind:open={confirmOpen} title={$_('transcripts.delete')} eyebrow={$_('transcripts.title')}>
  <p class="note">{$_('transcripts.deleteConfirm')}</p>
  {#snippet footer()}
    <Button onclick={() => (confirmOpen = false)}>{$_('transcripts.cancel')}</Button>
    <Button variant="danger" onclick={remove}>{$_('transcripts.delete')}</Button>
  {/snippet}
</Dialog>

<style>
  .centre {
    display: flex;
    justify-content: center;
    padding: 64px 24px;
  }
  .meta {
    margin: 0 24px 8px;
    color: var(--text-muted);
  }
  .meta.error {
    color: var(--status-error);
  }
  .toolbar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 10px;
    padding: 14px 24px;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }
  .surface {
    margin: 0 24px 96px;
    line-height: 1.6;
  }
  .note {
    margin: 0;
  }
</style>

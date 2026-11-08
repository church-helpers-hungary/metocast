<script lang="ts">
  import { onMount } from 'svelte';
  import { MediaQuery } from 'svelte/reactivity';
  import { _, locale } from 'svelte-i18n';
  import {
    PageHeader,
    Dot,
    Segmented,
    TransportDock,
    SlideSearch,
    SlideQueue,
    SectionLabel,
    List,
    Row,
    EmptyState,
    IconButton,
    Sheet,
    LabelledInput,
    TextArea,
    Button,
    ToggleRow,
  } from '@metocast/design-system';
  import type { TransportAction, SlideResult, QueueSlot } from '@metocast/design-system';
  import {
    sendWsCommand,
    searchFiles,
    openFile,
    listFolders,
    addFolder,
    removeFolder,
    keynoteCloseAll,
    pickDirectory,
    hostCapabilities,
    createSongPresentation,
    getSongSlideFolder,
    setSongSlideFolder,
    getPresentationFontsStatus,
    installPresentationFonts,
  } from '@metocast/core-client';
  import type { PresentationFontsStatus } from '@metocast/core-client';
  import type {
    ParagraphContent,
    PptFile,
    PptFolder,
  } from '@metocast/core-client/schemas/ws-messages';
  import { live } from '$lib/live.svelte';
  import { presenterUrls, appMode } from '$lib/core';
  import { relAge } from '$lib/format';
  import NotifBell from '$lib/NotifBell.svelte';
  import DeckLibrary from '$lib/DeckLibrary.svelte';
  import SwipeReveal from '$lib/SwipeReveal.svelte';
  import { notify } from '$lib/notifications.svelte';

  const mobile = new MediaQuery('(pointer: coarse)');
  const loc = $derived($locale ?? 'en');
  const mode = $derived(live.useWebPresenter ? 'web' : 'keynote');
  const presenterTheme = $derived(live.presenterTheme);

  const web = $derived(live.presenter);
  const kn = $derived(live.keynote);
  const loaded = $derived(
    mode === 'web' ? !!web?.loaded : !!(kn?.slideshowActive || kn?.documentName),
  );
  const deckName = $derived(
    mode === 'web' ? (web?.filePath?.split('/').pop() ?? '') : (kn?.documentName ?? ''),
  );
  const current = $derived(mode === 'web' ? (web?.currentSlide ?? 0) : (kn?.currentSlide ?? 0));
  const total = $derived(mode === 'web' ? (web?.totalSlides ?? 0) : (kn?.totalSlides ?? 0));
  const currentSlide = $derived(
    mode === 'web' && loaded ? web?.slides?.find((s) => s.index === web.currentSlide) : undefined,
  );

  function findCounterIndex(paragraphs: ParagraphContent[]): number {
    if (paragraphs.length < 2) return -1;
    const maxPt = paragraphs.reduce((max, paragraph) => Math.max(max, paragraph.fontSizePt), 0);
    if (maxPt === 0) return -1;
    const counter = (paragraph: ParagraphContent) =>
      paragraph.fontSizePt > 0 &&
      paragraph.fontSizePt < maxPt * 0.85 &&
      paragraph.align === 'center';
    if (paragraphs[0] && counter(paragraphs[0])) return 0;
    const last = paragraphs.length - 1;
    return paragraphs[last] && counter(paragraphs[last]) ? last : -1;
  }

  function paragraphText(paragraph: ParagraphContent | undefined): string {
    return paragraph?.lines.join(' ').trim() ?? '';
  }

  function bibleLabel(paragraph: ParagraphContent | undefined): string {
    return paragraphText(paragraph)
      .replace(/\s*\(\d+\/\d+\)\s*$/u, '')
      .replace(/\s*\|\s*/gu, ' · ');
  }

  function bibleVerseNumber(paragraph: ParagraphContent | undefined): string {
    const references = [...paragraphText(paragraph).matchAll(/\b\d+:(\d+)/gu)];
    return references.at(-1)?.[1] ?? '';
  }

  const previewCounterIndex = $derived(
    currentSlide ? findCounterIndex(currentSlide.paragraphs) : -1,
  );
  const previewCounter = $derived(
    currentSlide && previewCounterIndex >= 0
      ? currentSlide.paragraphs[previewCounterIndex]
      : undefined,
  );
  const previewBible = $derived(/^(Textus|Lekció)\b/u.test(paragraphText(previewCounter)));
  const previewParagraphs = $derived(
    currentSlide ? currentSlide.paragraphs.filter((_, index) => index !== previewCounterIndex) : [],
  );
  const previewBlank = $derived(previewParagraphs.every((paragraph) => !paragraphText(paragraph)));
  const previewSongTitle = $derived(!previewBible && current === 1);
  const previewSongLines = $derived(previewParagraphs.flatMap((paragraph) => paragraph.lines));
  const previewSongFontSize = $derived.by(() => {
    const longest = Math.max(1, ...previewSongLines.map((line) => line.length));
    const lineCount = Math.max(1, previewSongLines.length);
    const preferred = previewSongTitle ? 5.4 : 4.1;
    return Math.min(preferred, 77 / (longest * 0.55), 28 / lineCount);
  });
  const previewTextLength = $derived(
    previewParagraphs.reduce(
      (length, paragraph) =>
        length + paragraph.lines.reduce((lineLength, line) => lineLength + line.length, 0),
      0,
    ),
  );
  const previewDensity = $derived(
    previewTextLength > 300 ? 'dense' : previewTextLength > 180 ? 'medium' : 'short',
  );

  function previewDeckLabel(): string {
    const fileName = web?.filePath
      ?.split(/[/\\]/u)
      .at(-1)
      ?.replace(/\.pptx$/iu, '')
      .trim();
    return fileName ? `Ének · ${fileName}` : 'Ének';
  }

  function setMode(m: string) {
    sendWsCommand('presentation.set_use_web_presenter', { enabled: m === 'web' });
  }

  function setPresenterTheme(theme: string) {
    if (theme === 'classic' || theme === 'editorial') {
      sendWsCommand('presentation.set_presenter_theme', { theme });
    }
  }

  function tx(cmd: 'first' | 'prev' | 'next' | 'last' | 'start') {
    sendWsCommand(`presentation.${cmd}`);
  }
  function stop() {
    if (!sendWsCommand('keynote.close_all')) keynoteCloseAll();
  }

  const actions = $derived<TransportAction[]>([
    {
      icon: 'first',
      label: $_('presentations.first'),
      disabled: !loaded,
      onclick: () => tx('first'),
    },
    {
      icon: 'prev',
      label: $_('presentations.prev'),
      disabled: !loaded || current <= 1,
      onclick: () => tx('prev'),
    },
    {
      icon: 'next',
      label: $_('presentations.next'),
      variant: 'primary',
      disabled: !loaded || current >= total,
      onclick: () => tx('next'),
    },
    {
      icon: 'last',
      label: $_('presentations.last'),
      disabled: !loaded || current >= total,
      onclick: () => tx('last'),
    },
    ...(mode === 'keynote'
      ? [
          {
            icon: 'play' as const,
            label: $_('presentations.start'),
            disabled: !loaded,
            onclick: () => tx('start'),
          },
          {
            icon: 'stop' as const,
            label: $_('presentations.stop'),
            variant: 'stop' as const,
            disabled: !loaded,
            onclick: stop,
          },
        ]
      : [
          {
            icon: 'stop' as const,
            label: $_('presentations.unload'),
            variant: 'stop' as const,
            disabled: !loaded,
            onclick: stop,
          },
        ]),
  ]);

  const status = $derived(
    loaded
      ? $_(`presentations.mode.${mode}`) + ' · ' + $_('presentations.presenting')
      : $_('presentations.standby'),
  );

  // ── Deck search ───────────────────────────────────────────────────────────────
  let searchOpen = $state(false);
  let filter = $state('');
  let timer: ReturnType<typeof setTimeout>;

  function runSearch() {
    clearTimeout(timer);
    timer = setTimeout(() => {
      if (!sendWsCommand('ppt.search', { filter })) searchFiles(filter);
    }, 250);
  }
  function openSearch() {
    searchOpen = !searchOpen;
    if (searchOpen) runSearch();
  }
  $effect(() => {
    filter;
    if (searchOpen || filter.length > 0) runSearch();
  });

  const results = $derived<SlideResult[]>(
    searchOpen || filter.length > 0
      ? live.pptResults
          .slice(0, 8)
          .map((f) => ({ id: f.id, group: folderName(f.folderId), title: f.name }))
      : [],
  );

  // ── Preload queue ─────────────────────────────────────────────────────────────
  let slotFiles = $state<(PptFile | null)[]>([null, null, null, null, null]);
  const queueFull = $derived(slotFiles.every(Boolean));
  const slots = $derived<QueueSlot[]>(
    slotFiles.map((f, i) => ({
      index: i,
      title: f?.name,
      loaded: !!f && web?.filePath === f.path,
    })),
  );

  function fileById(id: string): PptFile | undefined {
    return live.pptResults.find((f) => f.id === id);
  }
  function queueResult(r: SlideResult) {
    const f = fileById(r.id);
    if (!f) return;
    const i = slotFiles.findIndex((s) => !s);
    if (i < 0) return;
    slotFiles[i] = f;
  }
  function openPath(path: string) {
    if (!sendWsCommand('presentation.open', { file_path: path })) openFile(path);
  }
  function openResult(r: SlideResult) {
    const f = fileById(r.id);
    if (f) openPath(f.path);
  }

  // ── Song creator ─────────────────────────────────────────────────────────────
  let songOpen = $state(false);
  let songTitle = $state('');
  let songLyrics = $state('');
  let songError = $state('');
  let creatingSong = $state(false);
  let outputFolder = $state('');
  let outputFolderDraft = $state('');
  let outputFolderError = $state('');
  let outputFolderSaving = $state(false);

  async function createSong(event: SubmitEvent) {
    event.preventDefault();
    songError = '';
    if (!songTitle.trim() || !songLyrics.trim()) {
      songError = $_('presentations.songRequired');
      return;
    }

    creatingSong = true;
    try {
      const result = await createSongPresentation(songTitle.trim(), songLyrics.trim());
      notify({
        tier: 'ok',
        kind: $_('presentations.songKind'),
        source: 'Metocast',
        title: $_('presentations.songCreated', { values: { n: result.slideCount } }),
        body: result.filePath,
        mono: true,
        actions: [
          {
            label: $_('presentations.open'),
            primary: true,
            run: () => openPath(result.filePath),
          },
        ],
      });
      songTitle = '';
      songLyrics = '';
      songOpen = false;
    } catch (error) {
      songError = error instanceof Error ? error.message : String(error);
      notify({
        tier: 'error',
        kind: $_('presentations.songKind'),
        source: 'Metocast',
        title: $_('presentations.songFailed'),
        body: songError,
      });
    } finally {
      creatingSong = false;
    }
  }

  // ── Settings sheet ────────────────────────────────────────────────────────────
  let settingsOpen = $state(false);
  let fontStatus = $state<PresentationFontsStatus | null>(null);
  const fontsInstalled = $derived(fontStatus?.installed ?? false);
  let installingFonts = $state(false);
  let fontBookOpened = $state(false);
  let fontError = $state('');

  let urls = $state<string[]>([]);
  let copied = $state<string | null>(null);
  const clients = $derived(
    live.clients.map((c) => ({
      id: c.id,
      name: c.label,
      address: c.hostname ?? c.userAgent ?? c.id.slice(0, 8),
      detail: relAge(c.connectedAt, loc) + (c.latencyMs != null ? ` · ${c.latencyMs}ms` : ''),
    })),
  );

  async function copyUrl(url: string) {
    try {
      await navigator.clipboard?.writeText(url);
      copied = url;
      setTimeout(() => (copied = null), 1300);
    } catch {
      /* clipboard blocked */
    }
  }

  async function installFonts() {
    fontError = '';
    installingFonts = true;
    try {
      fontStatus = await installPresentationFonts();
      fontBookOpened = true;
    } catch (error) {
      fontError = error instanceof Error ? error.message : String(error);
    } finally {
      installingFonts = false;
    }
  }

  let folders = $state<PptFolder[]>([]);
  function folderName(id: string): string {
    return folders.find((f) => f.id === id)?.name ?? '';
  }
  async function loadFolders() {
    try {
      folders = await listFolders();
    } catch {
      /* offline */
    }
  }
  // Only the desktop shell that *is* the core sees the same filesystem the core reads,
  // so it gets a native picker; every other window types the path and the core validates it.
  const canPick = $derived(hostCapabilities.dialogs && appMode() === 'server');
  let newPath = $state('');
  let folderError = $state('');
  let adding = $state(false);

  async function saveOutputFolder(path: string) {
    outputFolderError = '';
    outputFolderSaving = true;
    try {
      outputFolder = (await setSongSlideFolder(path)).path;
      outputFolderDraft = outputFolder;
    } catch (error) {
      outputFolderError = error instanceof Error ? error.message : String(error);
    } finally {
      outputFolderSaving = false;
    }
  }

  async function pickOutputFolder() {
    const path = await pickDirectory($_('presentations.outputFolderChoose'));
    if (path) await saveOutputFolder(path);
  }

  async function submitFolder(path: string) {
    folderError = '';
    adding = true;
    try {
      await addFolder(path, path.split(/[/\\]/).filter(Boolean).pop() || path);
      newPath = '';
      await loadFolders();
      if (searchOpen) runSearch();
    } catch (e) {
      folderError = e instanceof Error ? e.message : String(e);
    } finally {
      adding = false;
    }
  }

  async function addFolderFlow() {
    const path = await pickDirectory();
    if (path) await submitFolder(path);
  }
  async function dropFolder(id: string) {
    await removeFolder(id);
    await loadFolders();
  }

  onMount(async () => {
    urls = await presenterUrls();
    sendWsCommand('clients.list');
    const configuredOutput = await getSongSlideFolder().catch(() => null);
    outputFolder = configuredOutput?.path ?? '';
    outputFolderDraft = outputFolder;
    if (hostCapabilities.fonts) {
      fontStatus = await getPresentationFontsStatus().catch(() => null);
    }
    await loadFolders();
  });

  onMount(() => {
    const refreshFonts = async () => {
      if (!hostCapabilities.fonts) return;
      fontStatus = await getPresentationFontsStatus().catch(() => fontStatus);
      if (fontStatus?.installed) fontBookOpened = false;
    };
    window.addEventListener('focus', refreshFonts);
    return () => window.removeEventListener('focus', refreshFonts);
  });
</script>

<PageHeader eyebrow={$_(`presentations.mode.${mode}`)} title={$_('presentations.title')}>
  {#snippet trailing()}
    {#if loaded}<Dot color="var(--status-ok)" size={6} pulse />{/if}
    <IconButton
      icon="plus"
      label={$_('presentations.newSong')}
      variant="circle"
      onclick={() => (songOpen = true)}
    />
    <NotifBell />
    <IconButton
      icon="gear"
      label={$_('screens.settings.title')}
      variant="circle"
      onclick={() => (settingsOpen = true)}
    />
  {/snippet}
</PageHeader>

<div class="workspace">
  <div class="remote-col">
    <TransportDock
      label={$_('presentations.transport')}
      {status}
      current={loaded ? deckName : $_('presentations.noDeck')}
      position={loaded ? `${current} / ${total}` : '—'}
      {actions}
    />

    <SlideSearch
      {results}
      label={$_('presentations.search')}
      searchLabel={$_('presentations.search')}
      placeholder={mobile.current
        ? $_('presentations.tapToSearch')
        : $_('presentations.searchPlaceholder')}
      bind:filter
      emptyMessage={searchOpen || filter.length > 0
        ? $_('presentations.noMatch')
        : $_('presentations.tapToSearch')}
      openLabel={$_('presentations.open')}
      queueLabel={$_('presentations.queue')}
      queueDisabled={queueFull}
      numpad={mobile.current}
      ontrigger={openSearch}
      onopen={openResult}
      onqueue={queueResult}
    />

    <SectionLabel>{$_('library.title')}</SectionLabel>
    <DeckLibrary onpick={(file) => openPath(file.path)} openPath={web?.filePath ?? null} />
  </div>

  <aside class="secondary-col">
    <SlideQueue
      {slots}
      label={$_('presentations.queueLabel')}
      summary={$_('presentations.queueSummary', {
        values: { n: slotFiles.filter(Boolean).length, total: slotFiles.length },
      })}
      openLabel={$_('presentations.open')}
      clearLabel={$_('presentations.clear')}
      emptyLabel={$_('presentations.emptySlot')}
      onopen={(s) => {
        const f = slotFiles[s.index];
        if (f) openPath(f.path);
      }}
      onclear={(s) => (slotFiles[s.index] = null)}
    />

    <SectionLabel>{$_('presentations.preview')}</SectionLabel>
    <div class="preview">
      {#if currentSlide}
        <span class="pos">{current} / {total}</span>
        <div
          class:editorial={presenterTheme === 'editorial'}
          class:bible={presenterTheme === 'editorial' && previewBible}
          class:blank={presenterTheme === 'editorial' && previewBlank}
          class="slide"
          data-density={previewDensity}
        >
          {#if presenterTheme === 'editorial' && previewBlank}{:else if presenterTheme === 'editorial' && previewBible}
            <p class="preview-eyebrow">{bibleLabel(previewCounter)}</p>
            <p class="preview-verse-number">{bibleVerseNumber(previewCounter)}</p>
            <div class="preview-scripture">
              {#each previewParagraphs as paragraph, i (i)}
                <p>{paragraph.lines.join(' ')}</p>
              {/each}
            </div>
          {:else if presenterTheme === 'editorial'}
            <p class="preview-eyebrow">
              {previewSongTitle ? 'Ének' : previewDeckLabel()}
            </p>
            <div
              class="preview-lyrics"
              class:title={previewSongTitle}
              style:font-size={`${previewSongFontSize}cqi`}
            >
              {#each previewSongLines as line, i (i)}
                <span class="preview-line">{line || '\u00a0'}</span>
              {/each}
            </div>
            {#if !previewSongTitle}<p class="preview-section">{current}. dia</p>{/if}
          {:else}
            {#each currentSlide.paragraphs as p, i (i)}
              {#each p.lines as line, j (j)}<p>{line}</p>{/each}
            {/each}
          {/if}
          {#if presenterTheme === 'editorial' && !previewBlank}<p class="preview-mark">
              Metocast
            </p>{/if}
        </div>
      {:else}
        <EmptyState
          title={$_('presentations.previewWaiting')}
          hint={$_('presentations.previewWaitingHint')}
        />
      {/if}
    </div>
  </aside>
</div>

<Sheet bind:open={songOpen} title={$_('presentations.newSong')} eyebrow={$_('presentations.title')}>
  <form class="song-form" onsubmit={createSong}>
    <LabelledInput
      label={$_('presentations.songTitle')}
      placeholder={$_('presentations.songTitlePlaceholder')}
      bind:value={songTitle}
      required
    />
    <div class="song-lyrics">
      <SectionLabel hint={$_('presentations.songLyricsHint')}>
        {$_('presentations.songLyrics')}
      </SectionLabel>
      <TextArea
        label={$_('presentations.songLyrics')}
        placeholder={$_('presentations.songLyricsPlaceholder')}
        rows={13}
        bind:value={songLyrics}
        required
        invalid={songError.length > 0}
      />
    </div>
    <p class="song-structure">{$_('presentations.songStructureHint')}</p>
    <div class="song-output">
      <span>{$_('presentations.outputFolder')}</span>
      <code>{outputFolder || $_('presentations.outputFolderNone')}</code>
      <button
        type="button"
        onclick={() => {
          songOpen = false;
          settingsOpen = true;
        }}>{$_('presentations.outputFolderChange')}</button
      >
    </div>
    {#if songError}<p class="song-error">{songError}</p>{/if}
    <Button
      type="submit"
      variant="primary"
      block
      loading={creatingSong}
      loadingLabel={$_('presentations.creatingSong')}
      disabled={!songTitle.trim() || !songLyrics.trim() || !outputFolder}
    >
      {$_('presentations.createSong')}
    </Button>
  </form>
</Sheet>

<Sheet
  bind:open={settingsOpen}
  title={$_('screens.settings.title')}
  eyebrow={$_('presentations.title')}
>
  <SectionLabel>{$_('presentations.modeLabel')}</SectionLabel>
  <div class="mode-seg">
    <Segmented
      label={$_('presentations.modeLabel')}
      value={mode}
      options={[
        { value: 'web', label: $_('presentations.mode.web') },
        { value: 'keynote', label: $_('presentations.mode.keynote') },
      ]}
      onchange={setMode}
    />
  </div>

  {#if fontStatus?.supported}
    <SectionLabel hint={fontStatus.installDir ?? ''}>{$_('presentations.fontsTitle')}</SectionLabel>
    <List>
      <Row
        title={$_('presentations.fontsNames')}
        meta={fontsInstalled
          ? $_('presentations.fontsRestart')
          : fontBookOpened
            ? $_('presentations.fontsFontBook')
            : $_('presentations.fontsRequired')}
        chevron={false}
        last
      >
        {#snippet icon()}<span class="font-icon">Aa</span>{/snippet}
        {#snippet control()}
          {#if fontsInstalled}
            <span class="font-installed">{$_('presentations.fontsInstalled')}</span>
          {:else}
            <Button compact variant="secondary" loading={installingFonts} onclick={installFonts}>
              {$_('presentations.fontsInstall')}
            </Button>
          {/if}
        {/snippet}
      </Row>
    </List>
    {#if fontError}<p class="folder-error">{fontError}</p>{/if}
  {/if}

  <SectionLabel>{$_('presentations.themeLabel')}</SectionLabel>
  <div class="mode-seg">
    <Segmented
      label={$_('presentations.themeLabel')}
      value={presenterTheme}
      options={[
        { value: 'classic', label: $_('presentations.theme.classic') },
        { value: 'editorial', label: $_('presentations.theme.editorial') },
      ]}
      onchange={setPresenterTheme}
    />
  </div>

  <SectionLabel>{$_('presentations.deliveryLogLabel')}</SectionLabel>
  <List>
    <ToggleRow
      label={$_('presentations.deliveryLog')}
      sub={$_('presentations.deliveryLogSub')}
      checked={live.deliveryLog}
      onchange={(enabled) => sendWsCommand('presentation.set_delivery_log', { enabled })}
    />
  </List>

  {#if mode === 'web'}
    <SectionLabel>{$_('presentations.presenterSupport')}</SectionLabel>
    {#each urls as url (url)}
      <div class="url-bar">
        <span>{url}</span>
        <button type="button" onclick={() => copyUrl(url)}>
          {copied === url ? $_('presentations.copied') : $_('presentations.copy')}
        </button>
      </div>
    {/each}

    <SectionLabel hint={$_('presentations.clientsSummary', { values: { n: clients.length } })}>
      {$_('presentations.clients')}
    </SectionLabel>
    <List>
      {#if clients.length === 0}
        <div class="no-clients">{$_('presentations.noClients')}</div>
      {:else}
        {#each clients as c, i (c.id)}
          <Row
            title={c.name}
            meta={c.address}
            detail={c.detail}
            chevron={false}
            last={i === clients.length - 1}
          />
        {/each}
      {/if}
    </List>
  {/if}

  <SectionLabel hint={$_('presentations.outputFolderHint')}>
    {$_('presentations.outputFolder')}
  </SectionLabel>
  {#if canPick}
    <div class="output-folder-row">
      <code>{outputFolder || $_('presentations.outputFolderNone')}</code>
      <Button compact variant="secondary" onclick={pickOutputFolder} disabled={outputFolderSaving}>
        {$_('presentations.outputFolderChoose')}
      </Button>
    </div>
    {#if outputFolderError}<p class="folder-error">{outputFolderError}</p>{/if}
  {:else}
    <div class="folder-add">
      <LabelledInput
        label={$_('presentations.outputFolder')}
        placeholder={$_('presentations.folderPathPlaceholder')}
        bind:value={outputFolderDraft}
        error={outputFolderError}
      />
      <Button
        compact
        variant="secondary"
        loading={outputFolderSaving}
        disabled={!outputFolderDraft.trim()}
        onclick={() => saveOutputFolder(outputFolderDraft.trim())}
      >
        {$_('presentations.outputFolderSave')}
      </Button>
    </div>
  {/if}

  <SectionLabel>{$_('presentations.searchFolders')}</SectionLabel>

  <div class="folders-bar">
    <span class="folders-count"
      >{$_('presentations.foldersHint', { values: { n: folders.length } })}</span
    >
    {#if canPick}
      <button class="add-folder-btn" type="button" disabled={adding} onclick={addFolderFlow}>
        + {$_('presentations.addFolder')}
      </button>
    {/if}
  </div>
  {#if canPick}
    {#if folderError}<p class="folder-error">{folderError}</p>{/if}
  {:else}
    <div class="folder-add">
      <LabelledInput
        label={$_('presentations.folderPath')}
        placeholder={$_('presentations.folderPathPlaceholder')}
        bind:value={newPath}
        error={folderError}
      />
      <button
        class="add-folder-btn"
        type="button"
        disabled={adding || newPath.trim().length === 0}
        onclick={() => submitFolder(newPath.trim())}
      >
        + {$_('presentations.addFolder')}
      </button>
    </div>
  {/if}
  <List>
    {#each folders as f, i (f.id)}
      <SwipeReveal onCommit={() => dropFolder(f.id)} commitLabel={$_('presentations.removeFolder')}>
        <div class="folder-item" class:folder-last={i === folders.length - 1}>
          <strong class="folder-name">{f.name}</strong>
          <span class="folder-path">{f.path}</span>
        </div>
      </SwipeReveal>
    {/each}
  </List>
</Sheet>

<style>
  .song-form {
    display: grid;
    gap: 18px;
    padding: 8px 24px 24px;
  }
  .font-icon {
    display: grid;
    width: 34px;
    height: 34px;
    place-items: center;
    border: 1px solid color-mix(in srgb, var(--text-primary) 16%, transparent);
    border-radius: 50%;
    font-family: var(--font-display);
  }
  .font-installed {
    color: var(--status-ok);
    font-family: var(--font-label);
    font-size: 10px;
    letter-spacing: 0.12em;
    text-transform: uppercase;
  }
  .song-lyrics :global(.section-label) {
    margin-inline: 0;
  }
  .song-error {
    margin: -8px 0 0;
    color: var(--status-error);
    font-size: var(--type-body-sm-size);
  }
  .song-structure {
    margin: -8px 0 0;
    color: var(--text-muted);
    font-size: var(--type-body-sm-size);
  }
  .song-output,
  .output-folder-row {
    display: grid;
    gap: 6px;
    margin: 0 24px 16px;
    padding: 12px;
    border: 1px solid color-mix(in srgb, var(--text-primary) 14%, transparent);
    background: var(--surface-raised);
  }
  .song-output {
    margin: 0;
  }
  .song-output span {
    color: var(--text-muted);
    font-size: var(--type-body-sm-size);
  }
  .song-output code,
  .output-folder-row code {
    overflow: hidden;
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .song-output button {
    justify-self: start;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    cursor: pointer;
    font: inherit;
  }
  .preview {
    margin: 0 24px 8px;
  }
  .preview .pos {
    font-family: var(--font-mono);
    font-size: 10px;
    letter-spacing: 1.4px;
    color: var(--text-muted);
  }
  .slide {
    position: relative;
    overflow: hidden;
    aspect-ratio: 16 / 9;
    border: 1px solid color-mix(in srgb, var(--text-primary) 14%, transparent);
    padding: 16px;
    margin-top: 6px;
    text-align: center;
  }
  .slide p {
    margin: 2px 0;
    font-family: var(--font-display);
    font-size: 16px;
    color: var(--text-primary);
  }
  .slide.editorial {
    isolation: isolate;
    container-type: inline-size;
    display: grid;
    place-items: center;
    padding: 8% 7.8% 7%;
    border: 0;
    background: #11100e;
    color: #f4efe3;
  }
  .slide.editorial::before {
    position: absolute;
    z-index: -2;
    inset: 0;
    background:
      radial-gradient(circle at 86% 8%, rgba(207, 171, 105, 0.105), transparent 27%),
      radial-gradient(circle at 54% 56%, rgba(255, 246, 224, 0.024), transparent 48%);
    content: '';
  }
  .slide.editorial::after {
    position: absolute;
    z-index: -1;
    top: -70%;
    right: -8%;
    width: 54%;
    height: 235%;
    transform: rotate(28deg);
    background: linear-gradient(
      90deg,
      transparent,
      rgba(240, 222, 184, 0.022) 44%,
      rgba(240, 222, 184, 0.052) 50%,
      rgba(240, 222, 184, 0.016) 57%,
      transparent
    );
    content: '';
  }
  .slide.editorial.blank::before,
  .slide.editorial.blank::after {
    display: none;
  }
  .slide.editorial p {
    color: inherit;
  }
  .preview-eyebrow,
  .preview-section,
  .preview-mark {
    margin: 0;
    color: #cfab69 !important;
    font-family: var(--font-mono) !important;
    font-weight: 400;
    letter-spacing: 0.3em;
    text-transform: uppercase;
  }
  .preview-eyebrow {
    position: absolute;
    top: 14%;
    left: 10%;
    right: 10%;
    font-size: 1.18cqi !important;
  }
  .preview-lyrics {
    width: min(88%, 28ch);
    font-family: var(--font-display);
    line-height: 1.18;
  }
  .preview-line {
    display: block;
    min-height: 1.18em;
    white-space: nowrap;
  }
  .preview-scripture p {
    margin: 0;
    font: inherit;
    color: #f4efe3;
  }
  .preview-section {
    position: absolute;
    bottom: 12.5%;
    left: 50%;
    transform: translateX(-50%);
    font-size: 1.08cqi !important;
  }
  .preview-mark {
    position: absolute;
    right: 4.8%;
    bottom: 5.2%;
    color: rgba(244, 239, 227, 0.44) !important;
    font-size: 0.95cqi !important;
  }
  .slide.editorial.bible {
    grid-template-columns: 19% 1fr;
    grid-template-rows: auto auto;
    place-items: initial;
    align-content: center;
    column-gap: 7.2%;
    padding: 7.7% 8.3% 8.2% 7.4%;
    text-align: left;
  }
  .bible .preview-eyebrow {
    position: static;
    grid-column: 2;
    align-self: end;
    margin-bottom: 5.8%;
  }
  .preview-verse-number {
    grid-column: 1;
    grid-row: 2;
    align-self: start;
    margin: 0;
    color: #cfab69 !important;
    font-family: var(--font-display) !important;
    font-size: 16.8cqi !important;
    font-weight: 500;
    line-height: 0.76;
    text-align: right;
  }
  .preview-scripture {
    grid-column: 2;
    grid-row: 2;
    max-width: 19ch;
    font-family: var(--font-display);
    font-size: 4.45cqi;
    line-height: 1.14;
    text-wrap: pretty;
  }
  .editorial.bible[data-density='medium'] .preview-scripture {
    max-width: 25ch;
    font-size: 3.5cqi;
  }
  .editorial.bible[data-density='dense'] .preview-scripture {
    max-width: 31ch;
    font-size: 2.8cqi;
  }
  .url-bar {
    display: flex;
    margin: 0 24px 8px;
    border: 1px solid color-mix(in srgb, var(--text-primary) 14%, transparent);
    background: var(--surface-raised);
  }
  .url-bar span {
    flex: 1;
    min-width: 0;
    padding: 12px;
    font-family: var(--font-mono);
    font-size: 11px;
    color: var(--text-primary);
    word-break: break-all;
  }
  .url-bar button {
    padding: 0 16px;
    border: 0;
    background: var(--surface-inverse);
    color: var(--text-inverse);
    cursor: pointer;
    font-family: var(--font-label);
    font-size: 10px;
    letter-spacing: 1.4px;
    text-transform: uppercase;
    white-space: nowrap;
    min-height: var(--ui-target-min);
  }
  .no-clients {
    padding: 12px 24px;
    font-family: var(--font-display);
    font-style: italic;
    font-size: 14px;
    color: var(--text-muted);
  }
  .mode-seg {
    padding: 0 24px 16px;
  }
  .folders-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 12px 24px;
    border-top: 1px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
    border-bottom: 1px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
  }
  .folders-count {
    font-family: var(--font-label);
    font-size: 10px;
    letter-spacing: 1.4px;
    text-transform: uppercase;
    color: var(--text-muted);
  }
  .add-folder-btn {
    background: var(--surface-inverse);
    color: var(--text-inverse);
    border: 0;
    padding: 0 14px;
    min-height: var(--ui-target-min);
    font-family: var(--font-label);
    font-size: 10px;
    letter-spacing: 1.4px;
    text-transform: uppercase;
    cursor: pointer;
  }
  .add-folder-btn:disabled {
    opacity: 0.45;
    cursor: not-allowed;
  }
  .folder-add {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    padding: 12px 24px;
  }
  .folder-add :global(.chamber) {
    flex: 1;
    min-width: 0;
  }
  .folder-error {
    margin: 0;
    padding: 8px 24px;
    font-size: 12px;
    color: var(--status-error);
  }
  .folder-item {
    display: flex;
    align-items: center;
    gap: 14px;
    padding: 16px 24px;
    border-bottom: 1px solid color-mix(in srgb, var(--text-primary) 10%, transparent);
  }
  .folder-last {
    border-bottom: 0;
  }
  .folder-name {
    font-size: 15px;
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    flex-shrink: 0;
  }
  .folder-path {
    flex: 1;
    min-width: 0;
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .workspace,
  .remote-col,
  .secondary-col {
    display: contents;
  }

  @media (min-width: 760px) {
    .workspace {
      display: grid;
      grid-template-columns: minmax(300px, 1fr) minmax(250px, 330px);
      gap: 0 18px;
      align-items: start;
      padding: 0 18px 56px;
    }
    .remote-col,
    .secondary-col {
      display: block;
      min-width: 0;
    }
    .secondary-col {
      position: sticky;
      top: 18px;
    }
    .preview {
      margin-inline: 0;
    }
  }
  @media (min-width: 1360px) {
    .workspace {
      grid-template-columns: minmax(620px, 1fr) 400px;
      gap: 0 32px;
      padding-inline: 32px;
    }
  }
</style>

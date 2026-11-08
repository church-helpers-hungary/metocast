<script lang="ts">
  import { onMount, onDestroy, tick } from 'svelte';
  import { page } from '$app/stores';
  import { connectPresenterWs } from '@metocast/core-client';
  import { WsMessageSchema } from '@metocast/core-client/schemas/ws-messages';
  import type { PresenterState, PresenterTheme } from '@metocast/core-client/schemas/ws-messages';

  // ── Standalone WS connection ──────────────────────────────────────────────

  let standaloneSocket: ReturnType<typeof connectPresenterWs> | null = null;
  let standaloneState = $state<PresenterState | null>(null);
  let token = $state<string | null>(null);
  let svgObjectUrl = $state<string | null>(null);
  let presenterTheme = $state<PresenterTheme>('classic');

  function send(msg: object) {
    if (standaloneSocket?.readyState === 1) {
      standaloneSocket.send(JSON.stringify(msg));
    }
  }

  /** Tells the server the current slide is on screen (`presenter.ack`) after the next paint. */
  function ackShown() {
    void tick().then(() =>
      requestAnimationFrame(() => {
        if (standaloneState?.loaded) {
          send({
            type: 'presenter.ack',
            slide: standaloneState.currentSlide,
            shown_at: new Date().toISOString(),
          });
        }
      }),
    );
  }

  function connect() {
    // wsPort param lets the presenter page connect to the API server even when
    // the frontend is served from a different port (e.g. Vite dev server).
    const wsPort = $page.url.searchParams.get('wsPort');
    standaloneSocket = connectPresenterWs({
      token,
      wsPort,
      onMessage: (raw) => {
        const result = WsMessageSchema.safeParse(raw);
        if (!result.success) return;
        const msg = result.data;
        if (msg.type === 'presenter.state') {
          standaloneState = msg.state;
          ackShown();
        } else if (msg.type === 'presentation.settings') {
          presenterTheme = msg.presenterTheme;
        } else if (msg.type === 'presenter.slide_changed') {
          if (standaloneState) {
            standaloneState = {
              ...standaloneState,
              currentSlide: msg.currentSlide,
              totalSlides: msg.totalSlides,
            };
            ackShown();
          }
        } else if (msg.type === 'ping') {
          send({ type: 'pong', ping_id: msg.pingId });
        }
      },
    });

    standaloneSocket.addEventListener('open', () => {
      send({ type: 'presenter.register', label: 'Presenter Display' });
      send({ type: 'presenter.status' });
    });

    standaloneSocket.addEventListener('close', () => {
      setTimeout(() => connect(), 3000);
    });
  }

  // ── Font scaling ──────────────────────────────────────────────────────────
  // Font sizes are read directly from the PPTX (in points) and scaled to the
  // web container using the ratio of container pixels to slide EMUs.
  // 1 point = 12 700 EMU.

  const EMU_PER_PT = 12700;
  const DEFAULT_FONT_SIZE_PT = 28.0;
  const COUNTER_FONT_SIZE_PT = 18.0;

  let slideAreaEl = $state<HTMLElement | null>(null);
  let scaleFactor = $state(0);

  $effect(() => {
    const el = slideAreaEl;
    const state = standaloneState;

    if (!el || !state?.loaded) {
      scaleFactor = 0;
      return;
    }

    function recalc() {
      if (!el || !state) return;
      const style = getComputedStyle(el);
      const pw = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
      const ph = parseFloat(style.paddingTop) + parseFloat(style.paddingBottom);
      const w = el.clientWidth - pw;
      const h = el.clientHeight - ph;
      if (w > 0 && h > 0) {
        scaleFactor = Math.min(w / state.slideWidthEmu, h / state.slideHeightEmu);
      }
    }

    const obs = new ResizeObserver(recalc);
    obs.observe(el);
    recalc();
    return () => obs.disconnect();
  });

  function fontSizePx(fontSizePt: number): number {
    const pt = fontSizePt > 0 ? fontSizePt : DEFAULT_FONT_SIZE_PT;
    return pt * EMU_PER_PT * scaleFactor;
  }

  function counterFontSizePx(): number {
    return COUNTER_FONT_SIZE_PT * EMU_PER_PT * scaleFactor;
  }

  // ── Keyboard / navigation ─────────────────────────────────────────────────

  onMount(() => {
    token = $page.url.searchParams.get('token');
    connect();
    window.addEventListener('keydown', handleKey);
  });

  onDestroy(() => {
    standaloneSocket?.close();
    window.removeEventListener('keydown', handleKey);
  });

  function handleKey(e: KeyboardEvent) {
    if (e.key === 'ArrowRight' || e.key === 'ArrowDown' || e.key === ' ') {
      navigate('next');
    } else if (e.key === 'ArrowLeft' || e.key === 'ArrowUp') {
      navigate('prev');
    } else if (e.key === 'Home') {
      navigate('first');
    } else if (e.key === 'End') {
      navigate('last');
    } else if ((e.key === 'b' || e.key === 'B') && token) {
      toggleMute();
    }
  }

  function navigate(direction: 'next' | 'prev' | 'first' | 'last') {
    send({ type: `presenter.${direction}` });
  }

  function toggleMute() {
    const cmd = standaloneState?.muted ? 'presenter.unmute' : 'presenter.mute';
    send({ type: cmd });
  }

  // ── Derived display values ────────────────────────────────────────────────

  const currentSlide = $derived(
    standaloneState && standaloneState.loaded && standaloneState.currentSlide > 0
      ? (standaloneState.slides.find((s) => s.index === standaloneState!.currentSlide) ?? null)
      : null,
  );

  const currentSvgSlide = $derived(
    standaloneState && standaloneState.loaded && standaloneState.currentSlide > 0
      ? (standaloneState.svgSlides.find((s) => s.index === standaloneState!.currentSlide) ?? null)
      : null,
  );

  $effect(() => {
    const svg = currentSvgSlide?.svg ?? null;
    if (!svg) {
      svgObjectUrl = null;
      return;
    }

    const url = URL.createObjectURL(new Blob([svg], { type: 'image/svg+xml' }));
    svgObjectUrl = url;
    return () => URL.revokeObjectURL(url);
  });

  const slideParagraphs = $derived(currentSlide?.paragraphs ?? []);
  const slideIndex = $derived(standaloneState?.currentSlide ?? 0);
  const slideTotal = $derived(standaloneState?.totalSlides ?? 0);
  const isLoaded = $derived(standaloneState?.loaded ?? false);
  const isMuted = $derived(standaloneState?.muted ?? false);
  const renderMode = $derived(standaloneState?.renderMode ?? 'text');
  const shouldRenderSvg = $derived(
    presenterTheme === 'classic' && renderMode === 'svg' && currentSvgSlide !== null,
  );

  // ── Counter paragraph detection ───────────────────────────────────────────
  // The counter (slide number / verse reference) is a center-aligned paragraph
  // whose fontSizePt is < 85 % of the maximum on the slide.  In some PPTXes
  // it comes first in the XML (its text box was inserted before the lyrics
  // text box), in others it is last — so we check both ends.

  function findCounterIdx(paras: typeof slideParagraphs): number {
    if (paras.length < 2) return -1;
    const maxPt = paras.reduce((m, p) => Math.max(m, p.fontSizePt), 0);
    if (maxPt === 0) return -1;
    const isCounter = (p: (typeof paras)[0]) =>
      p.fontSizePt > 0 && p.fontSizePt < maxPt * 0.85 && p.align === 'center';
    const first = paras[0];
    if (first && isCounter(first)) return 0;
    const lastIdx = paras.length - 1;
    const lastPara = paras[lastIdx];
    if (lastPara && isCounter(lastPara)) return lastIdx;
    return -1;
  }

  const counterIdx = $derived(findCounterIdx(slideParagraphs));
  const counterParagraph = $derived(counterIdx >= 0 ? slideParagraphs[counterIdx] : null);
  const mainParagraphs = $derived(
    counterIdx >= 0 ? slideParagraphs.filter((_, i) => i !== counterIdx) : slideParagraphs,
  );

  function paragraphText(paragraph: (typeof slideParagraphs)[number] | null | undefined): string {
    return paragraph?.lines.join(' ').trim() ?? '';
  }

  function isBibleCounter(paragraph: (typeof slideParagraphs)[number] | null | undefined): boolean {
    return /^(Textus|Lekció)\b/u.test(paragraphText(paragraph));
  }

  function bibleLabel(paragraph: (typeof slideParagraphs)[number] | null | undefined): string {
    return paragraphText(paragraph)
      .replace(/\s*\(\d+\/\d+\)\s*$/u, '')
      .replace(/\s*\|\s*/gu, ' · ');
  }

  function bibleVerseNumber(
    paragraph: (typeof slideParagraphs)[number] | null | undefined,
  ): string {
    const references = [...paragraphText(paragraph).matchAll(/\b\d+:(\d+)/gu)];
    return references.at(-1)?.[1] ?? '';
  }

  function deckLabel(path: string | null | undefined): string {
    const fileName = path
      ?.split(/[/\\]/u)
      .at(-1)
      ?.replace(/\.pptx$/iu, '')
      .trim();
    return fileName ? `Ének · ${fileName}` : 'Ének';
  }

  const editorialBible = $derived(isBibleCounter(counterParagraph));
  const editorialTextLength = $derived(
    mainParagraphs.reduce(
      (length, paragraph) =>
        length + paragraph.lines.reduce((lineLength, line) => lineLength + line.length, 0),
      0,
    ),
  );
  const editorialDensity = $derived(
    editorialTextLength > 300 ? 'dense' : editorialTextLength > 180 ? 'medium' : 'short',
  );
  const editorialBlank = $derived(
    mainParagraphs.every((paragraph) => paragraph.lines.every((line) => line.trim().length === 0)),
  );
  const editorialSongTitle = $derived(!editorialBible && slideIndex === 1);
  const editorialSongLines = $derived(mainParagraphs.flatMap((paragraph) => paragraph.lines));
  const editorialSongFontSize = $derived.by(() => {
    const longest = Math.max(1, ...editorialSongLines.map((line) => line.length));
    const lineCount = Math.max(1, editorialSongLines.length);
    const preferred = editorialSongTitle ? 5.4 : 4.1;
    return Math.min(preferred, 77 / (longest * 0.55), 28 / lineCount);
  });
</script>

<svelte:head>
  <title>Presenter</title>
</svelte:head>

<div class="presenter-root">
  {#if isMuted}
    <div class="mute-overlay" aria-label="Display muted">
      {#if token}
        <button class="unmute-hint" onclick={toggleMute} aria-label="Unmute display">
          Click or press B to unmute
        </button>
      {/if}
    </div>
  {:else if !isLoaded}
    <div class="waiting">
      <p>Waiting for presentation…</p>
      <p class="hint">Load a .pptx file from the Presentations page to begin.</p>
    </div>
  {:else}
    {#if shouldRenderSvg}
      <div class="svg-stage">
        {#if svgObjectUrl}
          <img class="svg-slide" src={svgObjectUrl} alt="Current slide" />
        {/if}
      </div>
    {:else if presenterTheme === 'editorial' && editorialBlank}
      <div class="editorial-stage blank"></div>
    {:else if presenterTheme === 'editorial'}
      <div class="editorial-stage" class:bible={editorialBible} data-density={editorialDensity}>
        {#if editorialBible}
          <p class="editorial-eyebrow">{bibleLabel(counterParagraph)}</p>
          <p class="editorial-verse-number">{bibleVerseNumber(counterParagraph)}</p>
          <div class="editorial-scripture">
            {#each mainParagraphs as paragraph, i (i)}
              <p>
                {#each paragraph.lines as line, j (j)}
                  {#if j > 0}<br />{/if}{line}
                {/each}
              </p>
            {/each}
          </div>
        {:else}
          <p class="editorial-eyebrow">
            {editorialSongTitle
              ? isBibleCounter(mainParagraphs[0]) ? 'Ige' : 'Ének'
              : deckLabel(standaloneState?.filePath)}
          </p>
          <div
            class="editorial-lyrics"
            class:title={editorialSongTitle}
            style:font-size={`${editorialSongFontSize}vw`}
          >
            {#each editorialSongLines as line, i (i)}
              <span class="editorial-line">{line || '\u00a0'}</span>
            {/each}
          </div>
          {#if !editorialSongTitle}<p class="editorial-section">{slideIndex}. dia</p>{/if}
        {/if}
        <p class="editorial-mark">Metocast</p>
      </div>
    {:else}
      <div class="slide-area" bind:this={slideAreaEl}>
        <div class="main-content" style:visibility={scaleFactor > 0 ? 'visible' : 'hidden'}>
          <div class="text-container">
            {#each mainParagraphs as para, i (i)}
              <p
                class="slide-text"
                class:no-wrap={!counterParagraph}
                style="text-align: {para.align}; font-size: {fontSizePx(para.fontSizePt)}px"
              >
                {#each para.lines as line, i (i)}
                  {#if i > 0}<br />{/if}{line}
                {/each}
              </p>
            {/each}
          </div>
        </div>
        {#if counterParagraph}
          <p
            class="counter-text"
            style:visibility={scaleFactor > 0 ? 'visible' : 'hidden'}
            style="font-size: {counterFontSizePx()}px"
          >
            {#each counterParagraph.lines as line, i (i)}
              {#if i > 0}<br />{/if}{line}
            {/each}
          </p>
        {/if}
      </div>
    {/if}

    {#if token}
      <div class="nav-bar">
        <button class="nav-btn" onclick={() => navigate('prev')} aria-label="Previous slide"
          >◀</button
        >
        <span class="slide-counter">{slideIndex} / {slideTotal}</span>
        <button class="nav-btn" onclick={() => navigate('next')} aria-label="Next slide">▶</button>
      </div>
    {/if}
  {/if}
</div>

<style>
  @font-face {
    font-family: 'Cormorant Garamond';
    font-style: normal;
    font-weight: 500;
    font-display: swap;
    src: url('/presenter-fonts/CormorantGaramond-500-latin-ext.woff2') format('woff2');
    unicode-range:
      U+0100-02BA, U+02BD-02C5, U+02C7-02CC, U+02CE-02D7, U+02DD-02FF, U+0304, U+0308, U+0329,
      U+1D00-1DBF, U+1E00-1E9F, U+1EF2-1EFF, U+2020, U+20A0-20AB, U+20AD-20C0, U+2113, U+2C60-2C7F,
      U+A720-A7FF;
  }

  @font-face {
    font-family: 'Cormorant Garamond';
    font-style: normal;
    font-weight: 500;
    font-display: swap;
    src: url('/presenter-fonts/CormorantGaramond-500-latin.woff2') format('woff2');
    unicode-range:
      U+0000-00FF, U+0131, U+0152-0153, U+02BB-02BC, U+02C6, U+02DA, U+02DC, U+0304, U+0308, U+0329,
      U+2000-206F, U+20AC, U+2122, U+2191, U+2193, U+2212, U+2215, U+FEFF, U+FFFD;
  }

  @font-face {
    font-family: 'Geist Mono';
    font-style: normal;
    font-weight: 400;
    font-display: swap;
    src: url('/presenter-fonts/GeistMono-400-latin-ext.woff2') format('woff2');
    unicode-range:
      U+0100-02BA, U+02BD-02C5, U+02C7-02CC, U+02CE-02D7, U+02DD-02FF, U+0304, U+0308, U+0329,
      U+1D00-1DBF, U+1E00-1E9F, U+1EF2-1EFF, U+2020, U+20A0-20AB, U+20AD-20C0, U+2113, U+2C60-2C7F,
      U+A720-A7FF;
  }

  @font-face {
    font-family: 'Geist Mono';
    font-style: normal;
    font-weight: 400;
    font-display: swap;
    src: url('/presenter-fonts/GeistMono-400-latin.woff2') format('woff2');
    unicode-range:
      U+0000-00FF, U+0131, U+0152-0153, U+02BB-02BC, U+02C6, U+02DA, U+02DC, U+0304, U+0308, U+0329,
      U+2000-206F, U+20AC, U+2122, U+2191, U+2193, U+2212, U+2215, U+FEFF, U+FFFD;
  }

  :global(html),
  :global(body) {
    margin: 0;
    padding: 0;
    background: #000;
    color: #fff;
    overflow: hidden;
    height: 100%;
    width: 100%;
  }

  .presenter-root {
    position: fixed;
    inset: 0;
    background: #000;
    color: #fff;
  }

  /* ── Slide area — full screen, nav bar overlays it ────────────────────── */

  .slide-area {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    padding: 3vw 4vw 1.5vw;
    box-sizing: border-box;
  }

  .svg-stage {
    position: absolute;
    inset: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    background: #000;
    overflow: hidden;
  }

  .svg-slide {
    width: 100%;
    height: 100%;
    object-fit: contain;
    display: block;
    background: #000;
  }

  /* ── Editorial theme — shared by song decks and generated Bible slides ─ */

  .editorial-stage {
    position: absolute;
    inset: 0;
    isolation: isolate;
    overflow: hidden;
    box-sizing: border-box;
    display: grid;
    place-items: center;
    padding: 8% 7.8% 7%;
    background: #11100e;
    color: #f4efe3;
    text-align: center;
  }

  .editorial-stage::before {
    position: absolute;
    z-index: -2;
    inset: 0;
    background:
      radial-gradient(circle at 86% 8%, rgba(207, 171, 105, 0.105), transparent 27%),
      radial-gradient(circle at 54% 56%, rgba(255, 246, 224, 0.024), transparent 48%);
    content: '';
  }

  .editorial-stage::after {
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
    pointer-events: none;
  }
  .editorial-stage.blank::before,
  .editorial-stage.blank::after {
    display: none;
  }

  .editorial-eyebrow,
  .editorial-section,
  .editorial-mark {
    margin: 0;
    color: #cfab69;
    font-family: 'Geist Mono', ui-monospace, SFMono-Regular, Menlo, monospace;
    font-weight: 400;
    letter-spacing: 0.34em;
    text-transform: uppercase;
  }

  .editorial-eyebrow {
    position: absolute;
    top: 14%;
    left: 10%;
    right: 10%;
    font-size: clamp(0.65rem, 0.78vw, 1.35rem);
  }

  .editorial-lyrics {
    width: 86%;
    transform: translateY(2%);
    font-family: 'Cormorant Garamond', Georgia, serif;
    font-weight: 500;
    line-height: 1.18;
    letter-spacing: -0.018em;
    white-space: nowrap;
  }

  .editorial-line {
    display: block;
    min-height: 1.18em;
  }

  .editorial-lyrics.title {
    line-height: 1.08;
  }

  .editorial-scripture p {
    margin: 0;
  }

  .editorial-section {
    position: absolute;
    bottom: 12.5%;
    left: 50%;
    transform: translateX(-50%);
    font-size: clamp(0.55rem, 0.64vw, 1.05rem);
    letter-spacing: 0.31em;
  }

  .editorial-mark {
    position: absolute;
    right: 4.8%;
    bottom: 5.2%;
    color: rgba(244, 239, 227, 0.44);
    font-size: clamp(0.5rem, 0.55vw, 0.95rem);
    letter-spacing: 0.27em;
  }

  .editorial-stage.bible {
    grid-template-columns: 19% 1fr;
    grid-template-rows: auto auto;
    place-items: initial;
    align-content: center;
    column-gap: 7.2%;
    padding: 7.7% 8.3% 8.2% 7.4%;
    text-align: left;
  }

  .bible .editorial-eyebrow {
    position: static;
    grid-column: 2;
    align-self: end;
    margin-bottom: 5.8%;
    font-size: clamp(0.65rem, 0.78vw, 1.35rem);
    letter-spacing: 0.31em;
  }

  .editorial-verse-number {
    grid-column: 1;
    grid-row: 2;
    align-self: start;
    margin: 0;
    color: #cfab69;
    font-family: 'Cormorant Garamond', Georgia, serif;
    font-size: clamp(5.5rem, 12.8vw, 21rem);
    font-weight: 500;
    font-variant-numeric: oldstyle-nums;
    line-height: 0.76;
    text-align: right;
  }

  .editorial-scripture {
    grid-column: 2;
    grid-row: 2;
    max-width: 19ch;
    font-family: 'Cormorant Garamond', Georgia, serif;
    font-size: clamp(2.4rem, 4.1vw, 6.8rem);
    font-weight: 500;
    line-height: 1.14;
    letter-spacing: -0.018em;
    text-wrap: pretty;
  }

  .editorial-scripture p + p {
    margin-top: 0.28em;
  }

  .editorial-stage.bible[data-density='medium'] .editorial-scripture {
    max-width: 25ch;
    font-size: clamp(2rem, 3.35vw, 5.6rem);
  }

  .editorial-stage.bible[data-density='dense'] .editorial-scripture {
    max-width: 31ch;
    font-size: clamp(1.65rem, 2.7vw, 4.5rem);
  }

  .main-content {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    min-height: 0;
  }

  .text-container {
    width: 100%;
    display: flex;
    flex-direction: column;
    align-items: stretch;
    justify-content: center;
    font-family: Helvetica, Arial, sans-serif;
    font-weight: 700;
  }

  .slide-text {
    margin: 0;
    padding: 0;
    font-family: Helvetica, Arial, sans-serif;
    font-weight: 700;
    line-height: 1.2;
    color: #fff;
    width: 100%;
  }

  .slide-text.no-wrap {
    white-space: nowrap;
  }

  /* Paragraph spacing proportional to each paragraph's own font size. */
  .slide-text + .slide-text {
    margin-top: 0.35em;
  }

  .counter-text {
    position: absolute;
    left: 0;
    right: 0;
    bottom: 8%;
    box-sizing: border-box;
    margin: 0;
    padding: 0 10%;
    text-align: center;
    font-family: Helvetica, Arial, sans-serif;
    font-weight: 700;
    line-height: 1.2;
    color: #fff;
  }

  /* ── Navigation bar — overlays the slide content ──────────────────────── */

  .nav-bar {
    position: fixed;
    bottom: 0;
    left: 0;
    right: 0;
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 2.5rem;
    padding: 1rem 2rem 1.5rem;
    opacity: 0.07;
    transition: opacity 0.25s ease;
  }

  .nav-bar:hover,
  .nav-bar:focus-within {
    opacity: 1;
    background: rgba(0, 0, 0, 0.72);
  }

  /* Large touch targets — important for smart TV remotes and tablets */
  .nav-btn {
    min-width: 4rem;
    min-height: 4rem;
    padding: 0.75rem 1.5rem;
    background: rgba(255, 255, 255, 0.18);
    color: #fff;
    border: 1px solid rgba(255, 255, 255, 0.35);
    border-radius: 0.5rem;
    cursor: pointer;
    font-size: 1.75rem;
    line-height: 1;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .nav-btn:hover,
  .nav-btn:focus-visible {
    background: rgba(255, 255, 255, 0.35);
    outline: 2px solid rgba(255, 255, 255, 0.6);
    outline-offset: 2px;
  }

  .slide-counter {
    font-size: 1.5rem;
    font-family: Helvetica, Arial, sans-serif;
    font-weight: 700;
    opacity: 0.9;
    min-width: 6rem;
    text-align: center;
    letter-spacing: 0.05em;
  }

  /* ── Mute overlay ─────────────────────────────────────────────────────── */

  .mute-overlay {
    position: fixed;
    inset: 0;
    background: #000;
    display: flex;
    align-items: flex-end;
    justify-content: center;
    padding-bottom: 1rem;
  }

  .unmute-hint {
    background: transparent;
    border: none;
    color: rgba(255, 255, 255, 0.15);
    font-size: 0.75rem;
    cursor: pointer;
    padding: 0.5rem 1rem;
    transition: color 0.2s;
  }

  .unmute-hint:hover {
    color: rgba(255, 255, 255, 0.5);
  }

  /* ── Waiting state ────────────────────────────────────────────────────── */

  .waiting {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    opacity: 0.5;
  }

  .waiting p {
    margin: 0;
    font-size: 1.5rem;
  }

  .hint {
    font-size: 1rem !important;
    opacity: 0.7;
  }
</style>

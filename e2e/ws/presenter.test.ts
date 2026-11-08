/**
 * E2E tests for Presenter WebSocket commands.
 */

import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { WsTestClient } from '../helpers/ws-client.js';

const isLive = !!process.env.TAURI_TEST_TOKEN;

describe.skipIf(!isLive)('Presenter WebSocket Commands', () => {
  let ws: WsTestClient;

  beforeAll(async () => {
    ws = new WsTestClient();
    await ws.waitForConnect();
  });

  afterAll(() => {
    ws.close();
  });

  it('presenter.status → presenter.state response', async () => {
    const msg = await ws.command({ type: 'presenter.status' }, 'presenter.state');
    expect(msg['type']).toBe('presenter.state');
    expect(msg['state']).toHaveProperty('loaded');
    expect(msg['state']).toHaveProperty('currentSlide');
    expect(msg['state']).toHaveProperty('totalSlides');
    expect(msg['state']).toHaveProperty('renderMode');
    expect(Array.isArray((msg['state'] as { svgSlides: unknown[] }).svgSlides)).toBe(true);
    expect(Array.isArray((msg['state'] as { slides: unknown[] }).slides)).toBe(true);
    expect(msg['svgOmitted']).toBe(false);
  });

  it('a presenter=frames client gets the state without SVGs unless it asks for them', async () => {
    const frames = new WsTestClient('presenter=frames');
    try {
      await frames.waitForConnect();
      const light = await frames.command({ type: 'presenter.status' }, 'presenter.state');
      expect(light['svgOmitted']).toBe(true);
      const svgSlides = (light['state'] as { svgSlides: { svg: string }[] }).svgSlides;
      expect(svgSlides.every((slide) => slide.svg === '')).toBe(true);

      const full = await frames.command({ type: 'presenter.status', svg: true }, 'presenter.state');
      expect(full['svgOmitted']).toBe(false);
    } finally {
      frames.close();
    }
  });

  it('presentation.set_presenter_theme updates the shared presenter design', async () => {
    // Consume the settings snapshot pushed automatically when this client connected.
    await ws.waitForMessage('presentation.settings');

    const editorial = await ws.command(
      { type: 'presentation.set_presenter_theme', theme: 'editorial' },
      'presentation.settings',
    );
    expect(editorial['presenterTheme']).toBe('editorial');

    const restored = await ws.command(
      { type: 'presentation.set_presenter_theme', theme: 'classic' },
      'presentation.settings',
    );
    expect(restored['presenterTheme']).toBe('classic');
  });

  it('presenter.load with non-existent file → error response', async () => {
    const msg = await ws.command(
      { type: 'presenter.load', file_path: '/tmp/e2e-nonexistent.pptx' },
      'error',
    );
    expect(msg['type']).toBe('error');
    expect(typeof msg['message']).toBe('string');
  });

  it('events.presenter_list → backend-selected presenter event list', async () => {
    const msg = await ws.command({ type: 'events.presenter_list' }, 'events.presenter_list');
    expect(msg['type']).toBe('events.presenter_list');
    expect(Array.isArray(msg['events'])).toBe(true);
    expect(Object.prototype.hasOwnProperty.call(msg, 'selectedEventId')).toBe(true);
  });

  it('presenter.next on unloaded state → no crash (no-op)', async () => {
    // First ensure unloaded state
    await ws.command({ type: 'presenter.unload' }, 'presenter.state');
    // next on an empty presenter should silently no-op; request status to verify
    ws.send({ type: 'presenter.next' });
    const msg = await ws.command({ type: 'presenter.status' }, 'presenter.state');
    expect((msg['state'] as { loaded: boolean }).loaded).toBe(false);
  });
});

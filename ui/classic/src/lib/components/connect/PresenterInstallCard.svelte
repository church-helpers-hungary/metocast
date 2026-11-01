<script lang="ts">
  import { presenterInstallCommand } from '@metocast/core-client';
  import { serverUrl, localNetworkUrl } from '$lib/stores/server-url.js';

  type Platform = 'linux-x86_64' | 'linux-arm64' | 'macos-arm64';

  const platforms: { id: Platform; label: string }[] = [
    { id: 'linux-arm64', label: 'Linux ARM64 (RPi)' },
    { id: 'linux-x86_64', label: 'Linux (x86_64)' },
    { id: 'macos-arm64', label: 'macOS (Apple Silicon)' },
  ];

  const binaryName: Record<Platform, string> = {
    'linux-x86_64': 'presenter-receiver-linux-x86_64',
    'linux-arm64': 'presenter-receiver-linux-arm64',
    'macos-arm64': 'presenter-receiver-macos-arm64',
  };
  const githubRepository = 'church-helpers-hungary/metocast';

  let selectedPlatform = $state<Platform>('linux-arm64');
  let autoStart = $state(false);
  let copiedInstall = $state(false);
  let copiedManual = $state(false);

  const wsUrl = $derived(($localNetworkUrl || $serverUrl).replace(/^http/, 'ws') + '/ws');

  const installCommand = $derived(presenterInstallCommand(wsUrl, autoStart));

  const manualCommand = $derived(
    `curl -fsSL https://github.com/${githubRepository}/releases/latest/download/${binaryName[selectedPlatform]} -o presenter-receiver\nchmod +x presenter-receiver\n./presenter-receiver ${wsUrl}`,
  );

  async function copyInstall() {
    await navigator.clipboard.writeText(installCommand);
    copiedInstall = true;
    setTimeout(() => {
      copiedInstall = false;
    }, 2000);
  }

  async function copyManual() {
    await navigator.clipboard.writeText(manualCommand);
    copiedManual = true;
    setTimeout(() => {
      copiedManual = false;
    }, 2000);
  }
</script>

<section class="info-card">
  <h2>Presenter Receiver — Install</h2>
  <p class="note">
    Install the presenter receiver on the display device (e.g. Raspberry Pi). It connects to this
    server via WebSocket and renders slides on-screen via the framebuffer.
  </p>

  <h3>One-line install</h3>
  <p class="note">
    Auto-detects platform (Linux arm64/x86_64, macOS arm64). Downloads the binary, installs
    system dependencies if needed, and starts immediately.
  </p>

  <label class="autostart-toggle">
    <input type="checkbox" bind:checked={autoStart} />
    Auto-start on boot
    <span class="hint">(enables console auto-login on TTY1, Linux / systemd only)</span>
  </label>
  {#if autoStart}
    <p class="autostart-note">
      Sets up getty auto-login for the current user and adds a restart loop to
      <code>~/.bash_profile</code>. The presenter launches on boot with no login prompt. To update
      the server URL later, re-run this command with the new address.
    </p>
  {/if}

  <div class="cmd-row">
    <code class="cmd">{installCommand}</code>
    <button onclick={copyInstall}>{copiedInstall ? 'Copied!' : 'Copy'}</button>
  </div>

  <h3>Manual download</h3>
  <div class="platform-tabs">
    {#each platforms as p (p.id)}
      <button
        class="tab"
        class:active={selectedPlatform === p.id}
        onclick={() => (selectedPlatform = p.id)}>{p.label}</button
      >
    {/each}
  </div>
  <div class="cmd-row">
    <code class="cmd multiline">{manualCommand}</code>
    <button onclick={copyManual}>{copiedManual ? 'Copied!' : 'Copy'}</button>
  </div>
</section>

<style>
  .info-card {
    padding: 1.25rem;
    background: var(--content-bg);
    border: 1px solid var(--border);
    border-radius: 0.5rem;
    max-width: 600px;
  }

  h2 {
    margin: 0 0 0.75rem;
  }

  h3 {
    margin: 1.25rem 0 0.25rem;
    font-size: 0.95rem;
  }

  .note {
    font-size: 0.85rem;
    color: var(--text-secondary, #666);
    margin: 0 0 0.75rem;
  }

  .autostart-toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.85rem;
    cursor: pointer;
    margin-bottom: 0.5rem;
    user-select: none;
  }

  .hint {
    color: var(--text-secondary, #666);
    font-size: 0.78rem;
  }

  .autostart-note {
    font-size: 0.78rem;
    color: var(--text-secondary, #888);
    border-left: 2px solid var(--border);
    padding-left: 0.6rem;
    margin: 0 0 0.75rem;
  }

  .autostart-note code {
    font-size: 0.78rem;
    font-family: monospace;
  }

  .cmd-row {
    display: flex;
    align-items: flex-start;
    gap: 0.5rem;
  }

  .cmd-row button {
    padding: 0.2rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: 0.375rem;
    background: var(--glass-card-bg);
    cursor: pointer;
    font-size: 0.78rem;
    white-space: nowrap;
  }

  .cmd-row button:hover {
    background: var(--nav-item-hover);
  }

  .cmd {
    flex: 1;
    font-size: 0.75rem;
    word-break: break-all;
    white-space: pre-wrap;
    font-family: monospace;
  }

  .cmd.multiline {
    white-space: pre;
  }

  .platform-tabs {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-bottom: 0.6rem;
  }

  .tab {
    padding: 0.25rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: 0.375rem;
    background: var(--glass-card-bg);
    cursor: pointer;
    font-size: 0.78rem;
    color: var(--text-secondary, #666);
  }

  .tab:hover {
    background: var(--nav-item-hover);
  }

  .tab.active {
    background: var(--nav-item-active-bg);
    color: var(--nav-item-active-text);
    border-color: transparent;
  }
</style>

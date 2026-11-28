<script lang="ts">
  import { onMount } from 'svelte';
  import Sidebar from './lib/layout/Sidebar.svelte';
  import DraftBar from './lib/layout/DraftBar.svelte';
  import Button from './lib/ui/Button.svelte';
  import Alert from './lib/ui/Alert.svelte';
  import Login from './pages/Login.svelte';
  import Setup from './pages/Setup.svelte';
  import Overview from './pages/Overview.svelte';
  import Interfaces from './pages/Interfaces.svelte';
  import Services from './pages/Services.svelte';
  import Routes from './pages/Routes.svelte';
  import Firewall from './pages/Firewall.svelte';
  import Vpn from './pages/Vpn.svelte';
  import Containers from './pages/Containers.svelte';
  import FirewallPending from './lib/layout/FirewallPending.svelte';
  import History from './pages/History.svelte';
  import AiReview from './pages/AiReview.svelte';
  import Settings from './pages/Settings.svelte';
  import NotFound from './pages/NotFound.svelte';
  import { session, loadSession, logout } from './lib/session.svelte';
  import { route } from './lib/router.svelte';
  import { live, connectLive } from './lib/live.svelte';
  import { gw, loadGateway } from './lib/gateway.svelte';
  import AskAiPanel from './lib/askai/AskAiPanel.svelte';
  import { askai, setDialog, setScreen, toggleAskAi } from './lib/askai.svelte';
  import { overlays } from './lib/overlay.svelte';

  const user = $derived(session.info?.user ?? null);

  onMount(() => {
    loadSession();
  });

  // Live data and the configuration, while logged in.
  $effect(() => {
    if (!user) return;
    loadGateway();
    return connectLive();
  });

  // Someone else applied a change (or a rollback ran): reload.
  $effect(() => {
    const latest = live.status?.latest_revision ?? null;
    if (user && gw.loaded && latest !== null && latest !== (gw.revision?.id ?? null)) {
      loadGateway();
    }
  });

  const pages = {
    '/': { title: 'Overview', component: Overview },
    '/interfaces': { title: 'Public interfaces', component: Interfaces },
    '/services': { title: 'Services', component: Services },
    '/routes': { title: 'Routes', component: Routes },
    '/containers': { title: 'Containers', component: Containers },
    '/vpn': { title: 'VPN peers', component: Vpn },
    '/firewall': { title: 'Firewall', component: Firewall },
    '/ai': { title: 'AI review', component: AiReview },
    '/history': { title: 'History', component: History },
    '/settings': { title: 'Settings', component: Settings },
  } as const;
  type Section = keyof typeof pages;
  const section = $derived('/' + (route.path.split('/')[1] ?? ''));
  const page = $derived(section in pages ? pages[section as Section] : null);

  $effect(() => {
    document.title = page && user ? `${page.title} · Hearthgate` : 'Hearthgate';
  });

  // Ask AI follows the user: the screen, the item selected on it (the
  // second part of the address, or a settings section) and the open dialog.
  const selection = $derived.by(() => {
    const item = route.path.split('/')[2];
    if (item && item !== 'new') return decodeURIComponent(item);
    return section === '/settings' ? new URLSearchParams(route.search).get('section') : null;
  });
  $effect(() => {
    if (!user) return;
    setScreen({ title: page?.title ?? 'Page not found', path: route.path, selection });
  });
  $effect(() => {
    setDialog(overlays.top?.title ?? null);
  });

  let panel: ReturnType<typeof AskAiPanel> | undefined = $state();
  const PANEL_WIDTH = 420;
  let wasOpen = askai.open;
  $effect(() => {
    const open = !!user && askai.open;
    document.documentElement.style.setProperty('--hg-askai-width', open ? `${PANEL_WIDTH}px` : '0px');
    // Focus the message box when the user opens the panel (not when a
    // reload restores it open).
    if (open && !wasOpen) queueMicrotask(() => panel?.focus());
    wasOpen = open;
  });

  // ⌘J or Ctrl+J opens and closes the panel, from anywhere.
  function shortcut(event: KeyboardEvent) {
    if (!user || event.key.toLowerCase() !== 'j' || !(event.metaKey || event.ctrlKey) || event.altKey) return;
    event.preventDefault();
    toggleAskAi();
  }

  const dialogOpen = $derived(overlays.stack.length > 0);

  // Move focus to the new page's heading after navigation, for screen
  // reader and keyboard users.
  let main: HTMLElement | undefined = $state();
  let first = true;
  $effect(() => {
    void section;
    if (first) {
      first = false;
      return;
    }
    queueMicrotask(() => main?.querySelector<HTMLElement>('h1')?.focus());
  });
</script>

<svelte:window onkeydown={shortcut} />

<a class="skip" href="#main">Skip to content</a>

{#if session.info === null}
  {#if session.error}
    <main class="center">
      <Alert>{session.error}</Alert>
      <Button variant="secondary" onclick={loadSession}>Try again</Button>
    </main>
  {/if}
{:else if session.info.setup_required}
  <Setup />
{:else if !user}
  <Login />
{:else}
  <div class="shell" class:with-panel={askai.open} style:--panel-width="{PANEL_WIDTH}px">
    <Sidebar status={live.status} inert={dialogOpen} />
    <main class="main" id="main" bind:this={main} inert={dialogOpen}>
      <div class="account">
        {#if !live.connected && live.status}
          <span class="small secondary" role="status">Live updates paused, reconnecting…</span>
        {/if}
        <span class="small secondary">Logged in as <span class="strong">{user.username}</span></span>
        <Button variant="ghost" size="small" onclick={logout}>Log out</Button>
      </div>
      <FirewallPending />
      {#if gw.error}
        <Alert>{gw.error}</Alert>
      {/if}
      {#if page}
        {@const Page = page.component}
        <Page />
      {:else}
        <NotFound />
      {/if}
      <DraftBar />
    </main>
    {#if askai.open}
      <aside class="askai" aria-label="Ask AI">
        <AskAiPanel bind:this={panel} />
      </aside>
    {/if}
  </div>
{/if}

<style>
  .skip {
    position: absolute;
    left: -9999px;
    top: 8px;
    z-index: 10;
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    background: var(--hg-color-bg-surface);
    border-radius: var(--hg-radius-md);
  }
  .skip:focus {
    left: 8px;
  }
  .shell {
    display: flex;
    min-height: 100vh;
  }
  .main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xl);
    padding: var(--hg-spacing-2xl);
    max-width: 1280px;
  }
  .askai {
    flex: none;
    width: var(--panel-width);
    height: 100vh;
    position: sticky;
    top: 0;
    border-left: 1px solid var(--hg-color-border-default);
    /* Above dialog backdrops: the panel stays usable while one is open. */
    z-index: 150;
  }
  /* Narrow windows: the panel covers the page instead of pushing it. */
  @media (max-width: 1100px) {
    .askai {
      position: fixed;
      right: 0;
      width: min(var(--panel-width), 100vw);
      box-shadow: -8px 0 32px rgb(11 20 38 / 0.16);
    }
  }
  .main :global(h1) {
    outline: none;
  }
  .account {
    display: flex;
    justify-content: flex-end;
    align-items: center;
    gap: var(--hg-spacing-md);
    margin-bottom: calc(-1 * var(--hg-spacing-md));
  }
  .center {
    min-height: 100vh;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: var(--hg-spacing-lg);
  }
</style>

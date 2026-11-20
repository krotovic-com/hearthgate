<script lang="ts">
  import Icon, { type IconName } from '../ui/Icon.svelte';
  import logo from '../logo-on-light.svg';
  import { route } from '../router.svelte';
  import type { StatusResponse } from '../api';

  let { status }: { status: StatusResponse | null } = $props();

  // Sections that exist in this release. AI review joins the list in the
  // release that adds it.
  const items: { href: string; label: string; icon: IconName }[] = [
    { href: '/', label: 'Overview', icon: 'overview' },
    { href: '/interfaces', label: 'Public interfaces', icon: 'globe' },
    { href: '/services', label: 'Services', icon: 'server' },
    { href: '/routes', label: 'Routes', icon: 'route' },
    { href: '/vpn', label: 'VPN peers', icon: 'vpn' },
    { href: '/firewall', label: 'Firewall', icon: 'firewall' },
    { href: '/history', label: 'History', icon: 'arrow' },
    { href: '/settings', label: 'Settings', icon: 'settings' },
  ];

  const isActive = (href: string) =>
    href === '/' ? route.path === '/' : route.path === href || route.path.startsWith(href + '/');

  const healthy = $derived(status !== null && status.problems.length === 0);
  const down = $derived(status?.upstreams.filter((u) => !u.healthy || u.ejected).length ?? 0);
</script>

<nav class="sidebar" aria-label="Main">
  <a href="/" class="logo"><img src={logo} alt="Hearthgate" width="157" height="26" /></a>
  <ul>
    {#each items as item (item.href)}
      <li>
        <a href={item.href} class="nav-item strong" aria-current={isActive(item.href) ? 'page' : undefined}>
          <Icon name={item.icon} />
          {item.label}
        </a>
      </li>
    {/each}
  </ul>
  <div class="status" aria-live="polite">
    {#if status}
      <p class="state strong">
        <span class="dot" class:warn={!healthy}></span>
        {healthy ? 'Gateway healthy' : `${status.problems.length} ${status.problems.length === 1 ? 'problem' : 'problems'}`}
      </p>
      <p class="mono-small secondary">
        {status.revision ? `Revision ${status.revision} running` : 'No configuration yet'}
      </p>
      <p class="mono-small secondary">
        {status.listeners.length} {status.listeners.length === 1 ? 'listener' : 'listeners'} · {down} {down === 1 ? 'upstream' : 'upstreams'} down
      </p>
      <p class="mono-small secondary">Hearthgate {status.version}</p>
    {:else}
      <p class="mono-small secondary">Checking the gateway…</p>
    {/if}
  </div>
</nav>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-2xs);
    width: 248px;
    height: 100vh;
    position: sticky;
    top: 0;
    padding: var(--hg-spacing-xl) var(--hg-spacing-lg);
    background: var(--hg-color-bg-surface);
    border-right: 1px solid var(--hg-color-border-default);
  }
  .logo {
    display: block;
    width: fit-content;
    margin-bottom: var(--hg-spacing-xl);
  }
  .logo img {
    display: block;
    height: 26.4px;
    width: auto;
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-2xs);
  }
  .nav-item {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    border-radius: var(--hg-radius-md);
    color: var(--hg-color-text-secondary);
    text-decoration: none;
  }
  .nav-item:hover {
    background: var(--hg-color-bg-canvas);
    color: var(--hg-color-text-primary);
  }
  .nav-item[aria-current='page'] {
    background: var(--hg-color-accent-subtle);
    color: var(--hg-color-text-link);
  }
  .status {
    margin-top: auto;
    display: flex;
    flex-direction: column;
    gap: 6px;
    padding: var(--hg-spacing-md);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-canvas);
  }
  .state {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-sm);
  }
  .dot {
    width: 8px;
    height: 8px;
    border-radius: var(--hg-radius-full);
    background: var(--hg-color-status-healthy-fg);
  }
  .dot.warn {
    background: var(--hg-color-status-warning-fg);
  }
</style>

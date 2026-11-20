<script lang="ts">
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import General from './settings/General.svelte';
  import Access from './settings/Access.svelte';
  import Certificates from './settings/Certificates.svelte';
  import Account from './settings/Account.svelte';

  const sections = [
    { id: 'general', label: 'General' },
    { id: 'access', label: 'Dashboard access' },
    { id: 'certificates', label: 'Certificates' },
    { id: 'account', label: 'Account' },
  ];
  let active = $state('general');

  function go(event: MouseEvent, id: string) {
    event.preventDefault();
    active = id;
    const el = document.getElementById(id);
    el?.scrollIntoView({ behavior: 'smooth', block: 'start' });
    el?.focus({ preventScroll: true });
  }
</script>

<PageHeader title="Settings" subtitle="Settings for this gateway and your account." />

<div class="layout">
  <nav aria-label="Settings sections">
    <ul>
      {#each sections as s (s.id)}
        <li>
          <a href="#{s.id}" aria-current={active === s.id ? 'true' : undefined} onclick={(e) => go(e, s.id)}>{s.label}</a>
        </li>
      {/each}
    </ul>
  </nav>
  <div class="sections">
    <General />
    <Access />
    <Certificates />
    <Account />
  </div>
</div>

<style>
  .layout {
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    gap: var(--hg-spacing-2xl);
    align-items: start;
  }
  nav {
    position: sticky;
    top: var(--hg-spacing-xl);
  }
  ul {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-2xs);
  }
  a {
    display: block;
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    border-radius: var(--hg-radius-md);
    color: var(--hg-color-text-secondary);
    text-decoration: none;
  }
  a:hover {
    background: var(--hg-color-bg-subtle);
  }
  a[aria-current='true'] {
    background: var(--hg-color-bg-subtle);
    color: var(--hg-color-text-primary);
  }
  .sections {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xl);
    min-width: 0;
  }
</style>

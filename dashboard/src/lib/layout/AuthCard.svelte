<script lang="ts">
  import type { Snippet } from 'svelte';
  import logo from '../logo-on-light.svg';
  import logoDark from '../logo-on-dark.svg';

  let { title, intro, children }: { title: string; intro: string; children: Snippet } = $props();
</script>

<main class="page">
  <div class="card">
    <img src={logo} alt="Hearthgate" width="157" height="26" class="logo on-light" />
    <img src={logoDark} alt="Hearthgate" width="157" height="26" class="logo on-dark" />
    <div class="heading">
      <h1>{title}</h1>
      <p class="secondary">{intro}</p>
    </div>
    {@render children()}
  </div>
</main>

<style>
  .page {
    min-height: 100vh;
    display: grid;
    place-items: center;
    padding: var(--hg-spacing-xl);
  }
  .card {
    width: min(420px, 100%);
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xl);
    padding: var(--hg-spacing-2xl);
    background: var(--hg-color-bg-surface);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-lg);
  }
  .logo {
    height: 26.4px;
    width: auto;
    align-self: flex-start;
  }
  .on-light {
    display: var(--hg-logo-on-light);
  }
  .on-dark {
    display: var(--hg-logo-on-dark);
  }
  .heading {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
  }
</style>

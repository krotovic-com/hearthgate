<script lang="ts">
  import PageHeader from '../lib/layout/PageHeader.svelte';
  import Card from '../lib/ui/Card.svelte';
  import Badge from '../lib/ui/Badge.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Alert from '../lib/ui/Alert.svelte';
  import DiffView from '../lib/ui/DiffView.svelte';
  import ConfirmDialog from '../lib/ui/ConfirmDialog.svelte';
  import { api } from '../lib/api';
  import { gw, isDirty, rollbackTo } from '../lib/gateway.svelte';
  import { ago } from '../lib/format';
  import { navigate, route } from '../lib/router.svelte';

  const selectedId = $derived(Number(route.path.split('/')[2]) || gw.revisions[0]?.id || 0);
  const selected = $derived(gw.revisions.find((r) => r.id === selectedId) ?? null);
  const currentId = $derived(gw.revision?.id ?? null);

  let diff = $state<{ id: number; previous: number | null; text: string } | null>(null);
  let error = $state<string | null>(null);
  let confirmOpen = $state(false);
  let notice = $state<string | null>(null);

  $effect(() => {
    const id = selectedId;
    if (!id) return;
    error = null;
    api
      .revisionDiff(id)
      .then((d) => {
        if (id === selectedId) diff = { id, previous: d.previous, text: d.diff };
      })
      .catch((e) => (error = e instanceof Error ? e.message : String(e)));
  });

  const author = (a: string) =>
    a.startsWith('dashboard:') ? a.slice(10) : a === 'api:admin-token' ? 'admin token' : a.replace(/^api:token-/, 'API token ');

  async function rollback() {
    if (!selected) return;
    const r = await rollbackTo(selected.id);
    notice = `Revision ${selected.id} is running again, stored as revision ${r.id}.`;
    navigate(`/history/${r.id}`);
  }
</script>

<PageHeader
  title="History"
  subtitle="Every applied change is kept as a revision. Rolling back stores the old configuration as a new revision, so nothing is lost."
/>

{#if notice}<Alert tone="neutral">{notice}</Alert>{/if}

<div class="layout">
  <Card title="Revisions">
    {#if gw.revisions.length === 0}
      <p class="secondary">No revisions yet. They appear here when you apply your first change.</p>
    {:else}
      <ol class="revisions">
        {#each gw.revisions as r (r.id)}
          <li>
            <a href="/history/{r.id}" aria-current={r.id === selectedId ? 'true' : undefined}>
              <span class="top">
                <span class="mono-small">#{r.id}</span>
                <span class="strong msg">{r.message}</span>
                {#if r.id === currentId}<Badge status="healthy" dot>Running</Badge>{/if}
              </span>
              <span class="small secondary">
                {ago(new Date(r.created_at))} · {author(r.author)}{r.rollback_of ? ` · rollback to #${r.rollback_of}` : ''}
              </span>
            </a>
          </li>
        {/each}
      </ol>
    {/if}
  </Card>

  {#if selected}
    <Card title="Revision {selected.id}">
      {#snippet actions()}
        {#if selected.id !== currentId}
          <Button variant="secondary" size="small" onclick={() => (confirmOpen = true)}>Roll back to this</Button>
        {/if}
      {/snippet}
      <p class="secondary">
        {selected.message} · {new Date(selected.created_at).toLocaleString()} · {author(selected.author)}
      </p>
      {#if error}
        <Alert>{error}</Alert>
      {:else if diff && diff.id === selected.id}
        <p class="small secondary">
          {diff.previous ? `Changes since revision ${diff.previous}:` : 'The first configuration:'}
        </p>
        <DiffView diff={diff.text} />
      {:else}
        <p class="secondary">Loading…</p>
      {/if}
    </Card>
  {/if}
</div>

<ConfirmDialog
  bind:open={confirmOpen}
  title="Roll back to revision {selected?.id}?"
  message={`The configuration of revision ${selected?.id} runs again, stored as a new revision.${isDirty() ? ' Your unapplied draft is discarded.' : ''}`}
  confirm="Roll back"
  danger={false}
  onconfirm={rollback}
/>

<style>
  .layout {
    display: grid;
    grid-template-columns: 400px minmax(0, 1fr);
    gap: var(--hg-spacing-xl);
    align-items: start;
  }
  .revisions {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .revisions li + li {
    border-top: 1px solid var(--hg-color-border-default);
  }
  .revisions a {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-2xs);
    padding: var(--hg-spacing-md) var(--hg-spacing-sm);
    margin: 0 calc(-1 * var(--hg-spacing-sm));
    border-radius: var(--hg-radius-md);
    color: var(--hg-color-text-primary);
    text-decoration: none;
  }
  .revisions a[aria-current='true'] {
    background: var(--hg-color-accent-subtle);
  }
  .revisions a[aria-current='true'] .secondary {
    color: var(--hg-color-text-body);
  }
  .revisions a:hover .msg {
    text-decoration: underline;
  }
  .top {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-sm);
  }
  .msg {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  @media (max-width: 1100px) {
    .layout {
      grid-template-columns: minmax(0, 1fr);
    }
  }
</style>

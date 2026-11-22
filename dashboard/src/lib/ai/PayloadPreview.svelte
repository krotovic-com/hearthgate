<script lang="ts">
  // The exact request a review would send, after redaction. Asking for it
  // sends nothing.
  import Alert from '../ui/Alert.svelte';
  import { api, type AiPreview, type AiScope, type AiSettings } from '../api';

  let { scope, settings }: { scope: AiScope; settings?: AiSettings } = $props();

  let preview = $state<AiPreview | null>(null);
  let error = $state<string | null>(null);

  $effect(() => {
    const s = settings ? $state.snapshot(settings) : undefined;
    const sc = $state.snapshot(scope);
    error = null;
    api
      .aiPreview(sc, s)
      .then((p) => (preview = p))
      .catch((e) => (error = e instanceof Error ? e.message : String(e)));
  });
</script>

<div class="preview">
  {#if error}
    <Alert tone="warning">{error}</Alert>
  {:else if preview}
    <p class="small secondary">
      POST <span class="mono-small">{preview.url}</span> · {preview.redacted} values replaced by placeholders such as
      <span class="mono-small">&lt;PRIVATE_IP_1&gt;</span>. The API key is sent as a header and not shown.
    </p>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <pre class="mono-small body" tabindex="0" aria-label="Request body">{JSON.stringify(preview.body, null, 2)}</pre>
  {:else}
    <p class="small secondary">Preparing the preview…</p>
  {/if}
</div>

<style>
  .preview {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
  }
  .preview p {
    margin: 0;
  }
  .body {
    margin: 0;
    max-height: 320px;
    overflow: auto;
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    background: var(--hg-color-bg-subtle);
    border-radius: var(--hg-radius-md);
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>

<script lang="ts">
  // The Ask AI side panel: docked beside the page, it keeps its
  // conversation across screens and sees what the user sees.
  import Button from '../ui/Button.svelte';
  import Icon from '../ui/Icon.svelte';
  import ProposalCard from './ProposalCard.svelte';
  import { ai, loadAi } from '../ai.svelte';
  import { askai, currentPlan, newChat, send, toggleAskAi } from '../askai.svelte';
  import { planProgress, starters } from '../askai';
  import { overlays } from '../overlay.svelte';

  let { headingId = 'askai-title' }: { headingId?: string } = $props();

  let text = $state('');
  let input: HTMLTextAreaElement | undefined = $state();
  let log: HTMLElement | undefined = $state();

  $effect(() => {
    if (!ai.loaded) loadAi();
  });

  const enabled = $derived(ai.info?.enabled ?? false);
  const plan = $derived(currentPlan());
  const progress = $derived(plan ? planProgress(plan) : null);
  const last = $derived(askai.items[askai.items.length - 1]);
  const suggestions = $derived(
    last?.role === 'assistant' && last.suggestions?.length
      ? last.suggestions
      : askai.items.length === 0
        ? starters(askai.screen.path, askai.screen.selection)
        : [],
  );
  const dialog = $derived(overlays.top?.title ?? null);

  // Keep the newest message in view.
  $effect(() => {
    void askai.items.length;
    void askai.busy;
    queueMicrotask(() => log?.scrollTo({ top: log.scrollHeight }));
  });

  async function submit(question = text) {
    if (!question.trim() || askai.busy) return;
    text = '';
    await send(question);
    input?.focus();
  }

  function keydown(event: KeyboardEvent) {
    if (event.key === 'Enter' && !event.shiftKey && !event.isComposing) {
      event.preventDefault();
      submit();
    }
  }

  /** Focuses the message box, when the panel opens. */
  export function focus() {
    input?.focus();
  }
</script>

<section class="panel" aria-labelledby={headingId}>
  <header class="head">
    <span class="mark"><Icon name="ai" /></span>
    <h2 id={headingId} class="strong">Ask AI</h2>
    <div class="head-actions">
      {#if askai.items.length}
        <Button variant="ghost" size="small" onclick={newChat}><Icon name="plus" />New chat</Button>
      {/if}
      <Button variant="ghost" size="small" aria-label="Close Ask AI" onclick={() => toggleAskAi(false)}>✕</Button>
    </div>
  </header>

  {#if plan && progress}
    <div class="plan" aria-label="Setup plan">
      <div class="plan-head">
        <p class="strong">{plan.title}</p>
        <span class="mono-small secondary">{progress.done} of {progress.total} done</span>
      </div>
      <div class="bar" aria-hidden="true"><span style:width="{(progress.done / Math.max(progress.total, 1)) * 100}%"></span></div>
      <ol>
        {#each plan.steps as step, i (i)}
          <li class:done={step.done} class:current={progress.current === i}>
            <span class="step-mark" aria-hidden="true">{#if step.done}<Icon name="check" />{:else}{i + 1}{/if}</span>
            <span class="small">{step.title}{#if step.done}<span class="visually-hidden"> (done)</span>{/if}</span>
          </li>
        {/each}
      </ol>
    </div>
  {/if}

  <div class="log" bind:this={log} aria-live="polite" aria-busy={askai.busy}>
    {#if ai.loaded && !enabled}
      <div class="notice">
        <p class="strong">AI is turned off</p>
        <p class="small secondary">
          Ask AI uses the AI provider you choose. Turn it on in <a href="/settings?section=ai">Settings › AI assistant</a>.
        </p>
      </div>
    {:else if askai.items.length === 0}
      <div class="intro">
        <p class="strong">How can I help?</p>
        <p class="small secondary">
          Ask about what you see, or let me guide you through a setup, such as a new service from container to public
          route. I propose each change and you decide whether to apply it.
        </p>
      </div>
    {/if}
    {#each askai.items as item (item.id)}
      {#if item.role === 'context'}
        <p class="marker small secondary"><span aria-hidden="true">↳</span> {item.text}</p>
      {:else if item.role === 'user'}
        <div class="msg user"><p class="small">{item.text}</p></div>
      {:else}
        <div class="msg assistant">
          <p class="small reply">{item.text}</p>
          {#each item.proposals ?? [] as proposal, i (i)}
            <ProposalCard {proposal} item={item.id} index={i} />
          {/each}
        </div>
      {/if}
    {/each}
    {#if askai.busy}
      <p class="small secondary thinking" role="status">Thinking…</p>
    {/if}
    {#if askai.error}
      <p class="small problem" role="alert">{askai.error}</p>
    {/if}
  </div>

  <div class="composer">
    {#if suggestions.length && enabled}
      <div class="suggestions">
        {#each suggestions as s (s)}
          <button type="button" class="suggestion small" disabled={askai.busy} onclick={() => submit(s)}>{s}</button>
        {/each}
      </div>
    {/if}
    <div class="chips" aria-label="What Ask AI can see">
      <span class="chip mono-small">{askai.screen.title || 'Dashboard'}</span>
      {#if askai.screen.selection}<span class="chip mono-small">{askai.screen.selection}</span>{/if}
      {#if dialog}<span class="chip mono-small">Dialog: {dialog}</span>{/if}
    </div>
    <form
      class="box"
      onsubmit={(e) => {
        e.preventDefault();
        submit();
      }}
    >
      <label for="askai-input" class="visually-hidden">Message Ask AI</label>
      <textarea
        id="askai-input"
        bind:this={input}
        bind:value={text}
        rows="2"
        placeholder={enabled ? 'Ask anything about your gateway…' : 'Turn on AI in Settings to ask'}
        disabled={!enabled}
        onkeydown={keydown}
      ></textarea>
      <Button type="submit" size="small" aria-label="Send" disabled={!enabled || !text.trim()} busy={askai.busy}
        ><Icon name="arrow-up" /></Button
      >
    </form>
    <p class="mono-small secondary disclaimer">
      Logs and secrets are redacted before sending. Nothing changes until you apply it.
    </p>
  </div>
</section>

<style>
  .panel {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--hg-color-bg-surface);
  }
  .head {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    border-bottom: 1px solid var(--hg-color-border-default);
  }
  .mark {
    display: inline-flex;
    padding: 6px;
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-accent-subtle);
    color: var(--hg-color-text-link);
  }
  h2 {
    margin: 0;
    font-size: 1rem;
  }
  .head-actions {
    margin-left: auto;
    display: flex;
    gap: var(--hg-spacing-2xs);
  }
  .plan {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg);
    border-bottom: 1px solid var(--hg-color-border-default);
    background: var(--hg-color-bg-canvas);
  }
  .plan-head {
    display: flex;
    justify-content: space-between;
    gap: var(--hg-spacing-sm);
  }
  .plan p {
    margin: 0;
  }
  .bar {
    height: 4px;
    border-radius: var(--hg-radius-full);
    background: var(--hg-color-border-default);
    overflow: hidden;
  }
  .bar span {
    display: block;
    height: 100%;
    background: var(--hg-color-accent-default);
  }
  ol {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  li {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-sm);
    color: var(--hg-color-text-secondary);
  }
  li.current {
    color: var(--hg-color-text-primary);
    font-weight: 600;
  }
  .step-mark {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border-radius: var(--hg-radius-full);
    border: 1px solid var(--hg-color-border-strong);
    font-size: 11px;
    flex: none;
  }
  li.done .step-mark {
    border-color: var(--hg-color-status-healthy-fg);
    background: var(--hg-color-status-healthy-bg);
    color: var(--hg-color-status-healthy-text);
  }
  li.current .step-mark {
    border-color: var(--hg-color-accent-default);
    color: var(--hg-color-text-link);
  }
  .log {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-lg);
  }
  .log p {
    margin: 0;
  }
  .intro,
  .notice {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
  }
  .notice {
    padding: var(--hg-spacing-md);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-status-warning-bg);
  }
  .msg {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
  }
  .msg.user {
    align-self: flex-end;
    max-width: 85%;
    padding: var(--hg-spacing-sm) var(--hg-spacing-md);
    border-radius: var(--hg-radius-lg) var(--hg-radius-lg) 4px var(--hg-radius-lg);
    background: var(--hg-color-accent-subtle);
    color: var(--hg-color-text-primary);
  }
  .msg.user p,
  .reply {
    white-space: pre-wrap;
    word-break: break-word;
  }
  .marker {
    align-self: center;
    text-align: center;
    padding: 2px var(--hg-spacing-sm);
    border-radius: var(--hg-radius-full);
    background: var(--hg-color-bg-canvas);
  }
  .problem {
    color: var(--hg-color-status-critical-text);
  }
  .composer {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-md) var(--hg-spacing-lg) var(--hg-spacing-lg);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .suggestions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--hg-spacing-xs);
  }
  .suggestion {
    padding: 4px var(--hg-spacing-sm);
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-full);
    background: var(--hg-color-bg-surface);
    color: var(--hg-color-text-link);
    cursor: pointer;
    text-align: left;
  }
  .suggestion:hover:not(:disabled) {
    background: var(--hg-color-accent-subtle);
  }
  .chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--hg-spacing-2xs);
  }
  .chip {
    padding: 2px var(--hg-spacing-sm);
    border-radius: var(--hg-radius-sm, 4px);
    background: var(--hg-color-bg-canvas);
    color: var(--hg-color-text-secondary);
  }
  .box {
    display: flex;
    align-items: flex-end;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-sm);
    border: 1px solid var(--hg-color-border-strong);
    border-radius: var(--hg-radius-md);
    background: var(--hg-color-bg-surface);
  }
  .box:focus-within {
    border-color: var(--hg-color-border-focus);
  }
  textarea {
    flex: 1;
    resize: none;
    border: 0;
    outline: none;
    background: transparent;
    color: var(--hg-color-text-primary);
    font: inherit;
    font-size: 14px;
  }
  .disclaimer {
    margin: 0;
  }
</style>

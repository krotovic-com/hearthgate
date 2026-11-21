<script lang="ts">
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import TextField from '../ui/TextField.svelte';
  import TextArea from '../ui/TextArea.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import type { Stack } from '../api';
  import { draft, edit } from '../gateway.svelte';
  import { nameProblem } from './common';
  import { parseEnv, showEnv } from '../docker';

  let { open = $bindable(false), original = null }: { open?: boolean; original?: Stack | null } = $props();

  let name = $state('');
  let compose = $state('');
  let envText = $state('');
  let running = $state(true);
  let tried = $state(false);

  const example = 'services:\n  app:\n    image: nginx:1.27\n    restart: unless-stopped\n';

  $effect(() => {
    if (!open) return;
    name = original?.name ?? '';
    compose = original?.compose ?? '';
    envText = showEnv(original?.env);
    running = original?.running !== false;
    tried = false;
  });

  const taken = $derived(draft.stacks.map((s) => s.name).filter((n) => n !== original?.name));
  const nameError = $derived(nameProblem(name, taken, 'stack'));
  const composeError = $derived(
    !compose.trim()
      ? 'Paste a Compose file.'
      : /\t/.test(compose)
        ? 'Compose files are YAML: indent with spaces, not tabs.'
        : !/^services\s*:/m.test(compose)
          ? 'A Compose file needs a services: section.'
          : null,
  );
  const env = $derived(parseEnv(envText));

  function save() {
    tried = true;
    if (nameError || composeError || env.problem) return;
    const stack: Stack = { name, compose: compose.endsWith('\n') ? compose : `${compose}\n`, env: env.env, running };
    edit((s) => {
      s.stacks ??= [];
      const at = s.stacks.findIndex((x) => x.name === original?.name);
      if (at >= 0) s.stacks[at] = stack;
      else s.stacks.push(stack);
    });
    open = false;
  }
</script>

<Dialog
  bind:open
  wide
  title={original ? `Edit stack ${original.name}` : 'Add a stack'}
  description="A Compose file, run with docker compose. Its containers also join Hearthgate's network, so routes reach them as NAME-SERVICE-1."
>
  <TextField
    label="Name"
    bind:value={name}
    mono
    disabled={!!original}
    hint={original ? 'The Compose project name. Remove and add the stack to rename it.' : 'The Compose project name.'}
    error={tried ? nameError : null}
  />
  <TextArea label="Compose file" bind:value={compose} mono rows={14} placeholder={example} error={tried ? composeError : null} />
  <TextArea
    label="Variables (.env)"
    bind:value={envText}
    mono
    rows={3}
    placeholder="POSTGRES_PASSWORD=…"
    hint="One KEY=value per line. Use them in the Compose file as ${'{'}KEY{'}'}."
    error={tried ? env.problem : null}
  />
  <div class="toggle-row">
    <Toggle label="Running" bind:checked={running} />
    <span>Running</span>
  </div>
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Cancel</Button>
    <Button onclick={save}>{original ? 'Save to draft' : 'Add to draft'}</Button>
  {/snippet}
</Dialog>

<style>
  .toggle-row {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
  }
</style>

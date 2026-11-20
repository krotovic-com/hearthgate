<script lang="ts">
  import Dialog from './Dialog.svelte';
  import Button from './Button.svelte';

  let {
    open = $bindable(false),
    title,
    message,
    confirm = 'Remove',
    danger = true,
    onconfirm,
  }: {
    open?: boolean;
    title: string;
    message: string;
    confirm?: string;
    danger?: boolean;
    onconfirm: () => void | Promise<void>;
  } = $props();

  let busy = $state(false);
</script>

<Dialog bind:open {title}>
  <p>{message}</p>
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Cancel</Button>
    <Button
      variant={danger ? 'danger' : 'primary'}
      {busy}
      onclick={async () => {
        busy = true;
        try {
          await onconfirm();
          open = false;
        } finally {
          busy = false;
        }
      }}>{confirm}</Button
    >
  {/snippet}
</Dialog>

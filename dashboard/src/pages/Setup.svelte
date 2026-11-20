<script lang="ts">
  import AuthCard from '../lib/layout/AuthCard.svelte';
  import TextField from '../lib/ui/TextField.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Alert from '../lib/ui/Alert.svelte';
  import { api } from '../lib/api';
  import { setSession } from '../lib/session.svelte';

  const MIN_LENGTH = 12;
  let setupCode = $state('');
  let username = $state('admin');
  let password = $state('');
  let confirm = $state('');
  let error = $state<string | null>(null);
  let busy = $state(false);
  let touched = $state(false);

  const passwordError = $derived(
    touched && password.length > 0 && password.length < MIN_LENGTH
      ? `Use at least ${MIN_LENGTH} characters.`
      : null,
  );
  const confirmError = $derived(touched && confirm.length > 0 && confirm !== password ? "The passwords don't match." : null);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    touched = true;
    if (password.length < MIN_LENGTH || confirm !== password) return;
    busy = true;
    error = null;
    try {
      setSession(await api.setup(setupCode, username, password));
    } catch (e) {
      error = e instanceof Error ? e.message : String(e);
    } finally {
      busy = false;
    }
  }
</script>

<AuthCard
  title="Create the admin account"
  intro="No account exists yet. The setup code proves you have access to the server."
>
  <form onsubmit={submit} class="form">
    {#if error}
      <Alert>{error}</Alert>
    {/if}
    <TextField
      label="Setup code"
      bind:value={setupCode}
      mono
      autocomplete="off"
      placeholder="XXXX-XXXX-XXXX"
      hint="It's in the service log, or run: sudo -u hearthgate hearthgate setup-code"
      required
      autofocus
    />
    <TextField label="Username" bind:value={username} autocomplete="username" required />
    <TextField
      label="Password"
      type="password"
      bind:value={password}
      autocomplete="new-password"
      hint={`At least ${MIN_LENGTH} characters. A passphrase of a few words works well.`}
      error={passwordError}
      onblur={() => (touched = true)}
      required
    />
    <TextField
      label="Repeat password"
      type="password"
      bind:value={confirm}
      autocomplete="new-password"
      error={confirmError}
      required
    />
    <Button type="submit" {busy}>{busy ? 'Creating…' : 'Create account'}</Button>
  </form>
</AuthCard>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-lg);
  }
</style>

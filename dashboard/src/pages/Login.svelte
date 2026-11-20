<script lang="ts">
  import AuthCard from '../lib/layout/AuthCard.svelte';
  import TextField from '../lib/ui/TextField.svelte';
  import Button from '../lib/ui/Button.svelte';
  import Alert from '../lib/ui/Alert.svelte';
  import { api, ApiError } from '../lib/api';
  import { setSession } from '../lib/session.svelte';

  let username = $state('');
  let password = $state('');
  let code = $state('');
  let needsCode = $state(false);
  let error = $state<string | null>(null);
  let busy = $state(false);

  async function submit(event: SubmitEvent) {
    event.preventDefault();
    busy = true;
    error = null;
    try {
      setSession(await api.login(username, password, needsCode ? code : undefined));
    } catch (e) {
      if (e instanceof ApiError && e.problem.totp_required) {
        // Ask for the code only once the password is known to be right.
        if (needsCode) error = e.message;
        needsCode = true;
        code = '';
      } else {
        error = e instanceof Error ? e.message : String(e);
      }
    } finally {
      busy = false;
    }
  }
</script>

<AuthCard title="Log in" intro="Manage your gateway. This page is only reachable from your network.">
  <form onsubmit={submit} class="form">
    {#if error}
      <Alert>{error}</Alert>
    {/if}
    {#if needsCode}
      <TextField
        label="Authenticator code"
        bind:value={code}
        mono
        autocomplete="one-time-code"
        inputmode="numeric"
        hint="Enter the 6-digit code from your authenticator app."
        required
        autofocus
      />
    {:else}
      <TextField label="Username" bind:value={username} autocomplete="username" required autofocus />
      <TextField label="Password" type="password" bind:value={password} autocomplete="current-password" required />
    {/if}
    <Button type="submit" {busy}>{busy ? 'Logging in…' : 'Log in'}</Button>
  </form>
</AuthCard>

<style>
  .form {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-lg);
  }
</style>

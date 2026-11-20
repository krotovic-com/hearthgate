<script lang="ts">
  import { onMount } from 'svelte';
  import { renderSVG } from 'uqr';
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import Button from '../../lib/ui/Button.svelte';
  import TextField from '../../lib/ui/TextField.svelte';
  import Alert from '../../lib/ui/Alert.svelte';
  import Dialog from '../../lib/ui/Dialog.svelte';
  import { api, type ApiTokenInfo, type CreatedToken, type TotpSecret } from '../../lib/api';
  import { session, loadSession } from '../../lib/session.svelte';
  import { ago } from '../../lib/format';

  const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

  // Password
  let current = $state('');
  let next = $state('');
  let repeat = $state('');
  let pwError = $state<string | null>(null);
  let pwDone = $state(false);
  let pwBusy = $state(false);

  async function changePassword() {
    pwError = null;
    pwDone = false;
    if (next !== repeat) {
      pwError = 'The new passwords do not match.';
      return;
    }
    pwBusy = true;
    try {
      await api.changePassword(current, next);
      pwDone = true;
      current = next = repeat = '';
    } catch (e) {
      pwError = message(e);
    } finally {
      pwBusy = false;
    }
  }

  // Two-factor login
  let totpOpen = $state(false);
  let secret = $state<TotpSecret | null>(null);
  let code = $state('');
  let totpError = $state<string | null>(null);
  let disableOpen = $state(false);
  let disablePassword = $state('');
  const qr = $derived(secret ? renderSVG(secret.uri, { border: 2 }) : '');

  async function beginTotp() {
    totpError = null;
    code = '';
    try {
      secret = await api.totpBegin();
      totpOpen = true;
    } catch (e) {
      totpError = message(e);
    }
  }

  async function enableTotp() {
    if (!secret) return;
    totpError = null;
    try {
      await api.totpEnable(secret.secret, code);
      totpOpen = false;
      secret = null;
      await loadSession();
    } catch (e) {
      totpError = message(e);
    }
  }

  async function disableTotp() {
    totpError = null;
    try {
      await api.totpDisable(disablePassword);
      disableOpen = false;
      disablePassword = '';
      await loadSession();
    } catch (e) {
      totpError = message(e);
    }
  }

  // API tokens
  let tokens = $state<ApiTokenInfo[]>([]);
  let tokenName = $state('');
  let created = $state<CreatedToken | null>(null);
  let tokenError = $state<string | null>(null);

  const loadTokens = () =>
    api
      .tokens()
      .then((t) => (tokens = t))
      .catch((e) => (tokenError = message(e)));
  onMount(loadTokens);

  async function createToken() {
    tokenError = null;
    try {
      created = await api.createToken(tokenName.trim());
      tokenName = '';
      await loadTokens();
    } catch (e) {
      tokenError = message(e);
    }
  }

  async function revoke(t: ApiTokenInfo) {
    tokenError = null;
    try {
      await api.deleteToken(t.id);
      await loadTokens();
    } catch (e) {
      tokenError = message(e);
    }
  }
</script>

<Section id="account" title="Account" description="Logged in as {session.info?.user?.username}.">
  <form
    class="body"
    onsubmit={(e) => {
      e.preventDefault();
      changePassword();
    }}
  >
    <h3>Password</h3>
    <TextField label="Current password" type="password" bind:value={current} autocomplete="current-password" required />
    <TextField label="New password" type="password" bind:value={next} autocomplete="new-password" required minlength={12} hint="At least 12 characters." />
    <TextField label="Repeat new password" type="password" bind:value={repeat} autocomplete="new-password" required />
    {#if pwError}<Alert>{pwError}</Alert>{/if}
    {#if pwDone}<Alert tone="neutral">Password changed. Other sessions were logged out.</Alert>{/if}
    <div><Button type="submit" busy={pwBusy}>Change password</Button></div>
  </form>

  <SettingRow
    label="Two-factor login"
    description={session.info?.user?.totp_enabled
      ? 'On: logging in also needs a code from your authenticator app.'
      : 'Off: add a code from an authenticator app to every login.'}
  >
    {#if session.info?.user?.totp_enabled}
      <Button variant="secondary" onclick={() => (disableOpen = true)}>Turn off</Button>
    {:else}
      <Button variant="secondary" onclick={beginTotp}>Set up</Button>
    {/if}
  </SettingRow>
  {#if totpError && !totpOpen && !disableOpen}<div class="body"><Alert>{totpError}</Alert></div>{/if}

  <div class="body">
    <h3>API tokens</h3>
    <p class="small secondary">
      For scripts and other tools: send <span class="mono-small">Authorization: Bearer TOKEN</span>. A token can do
      everything you can, so give each tool its own and revoke what you no longer use.
    </p>
    {#if tokens.length}
      <ul class="tokens">
        {#each tokens as t (t.id)}
          <li>
            <span class="strong">{t.name}</span>
            <span class="small secondary">
              created {ago(new Date(t.created_at * 1000))} · {t.last_used_at ? `last used ${ago(new Date(t.last_used_at * 1000))}` : 'never used'}
            </span>
            <Button variant="ghost" size="small" onclick={() => revoke(t)}>Revoke<span class="visually-hidden"> {t.name}</span></Button>
          </li>
        {/each}
      </ul>
    {/if}
    <form
      class="new-token"
      onsubmit={(e) => {
        e.preventDefault();
        createToken();
      }}
    >
      <TextField label="New token name" bind:value={tokenName} placeholder="backup script" required />
      <Button type="submit" variant="secondary">Create token</Button>
    </form>
    {#if created}
      <Alert tone="neutral">
        <p class="strong">Copy the token for {created.name} now. It is not shown again.</p>
        <p class="mono-small token">{created.token}</p>
      </Alert>
    {/if}
    {#if tokenError}<Alert>{tokenError}</Alert>{/if}
  </div>
</Section>

<Dialog bind:open={totpOpen} title="Set up two-factor login" description="Scan the code with an authenticator app, then enter the 6-digit code it shows.">
  {#if secret}
    <div class="qr" role="img" aria-label="QR code for your authenticator app">{@html qr}</div>
    <p class="small secondary">Can't scan? Enter this key: <span class="mono-small">{secret.secret}</span></p>
    <TextField label="Code from the app" bind:value={code} mono inputmode="numeric" autocomplete="one-time-code" maxlength={6} />
    {#if totpError}<Alert>{totpError}</Alert>{/if}
  {/if}
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (totpOpen = false)}>Cancel</Button>
    <Button onclick={enableTotp} disabled={code.length !== 6}>Turn on</Button>
  {/snippet}
</Dialog>

<Dialog bind:open={disableOpen} title="Turn off two-factor login?" description="Confirm with your password.">
  <TextField label="Password" type="password" bind:value={disablePassword} autocomplete="current-password" />
  {#if totpError}<Alert>{totpError}</Alert>{/if}
  {#snippet footer()}
    <Button variant="secondary" onclick={() => (disableOpen = false)}>Cancel</Button>
    <Button variant="danger" onclick={disableTotp}>Turn off</Button>
  {/snippet}
</Dialog>

<style>
  .body {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl) var(--hg-spacing-xl);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .body > :global(*) {
    max-width: 640px;
  }
  .tokens {
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .tokens li {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-sm) 0;
    border-top: 1px solid var(--hg-color-border-default);
  }
  .tokens li .small {
    flex: 1;
  }
  .new-token {
    display: flex;
    gap: var(--hg-spacing-sm);
    align-items: end;
  }
  .token {
    overflow-wrap: anywhere;
    user-select: all;
  }
  .qr {
    width: 200px;
    height: 200px;
    align-self: center;
  }
  .qr :global(svg) {
    width: 100%;
    height: 100%;
  }
</style>

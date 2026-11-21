<script lang="ts">
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import TextField from '../../lib/ui/TextField.svelte';
  import Select from '../../lib/ui/Select.svelte';
  import Button from '../../lib/ui/Button.svelte';
  import Alert from '../../lib/ui/Alert.svelte';
  import Toggle from '../../lib/ui/Toggle.svelte';
  import { api, type EmailSettings, type EventKind, type NotificationTestResult, type Webhook } from '../../lib/api';
  import { edit, gw, compactNotifications } from '../../lib/gateway.svelte';

  const n = $derived(compactNotifications(gw.draft.notifications));
  const kinds: { kind: EventKind; label: string; description: string }[] = [
    { kind: 'certificates', label: 'Certificates', description: 'A certificate could not be issued or renewed.' },
    { kind: 'health', label: 'Service health', description: 'A service stopped answering its health checks for a minute, and when it is back.' },
    { kind: 'security', label: 'Security', description: 'A firewall change was undone because nobody confirmed it.' },
    { kind: 'backups', label: 'Backups', description: 'The daily backup failed.' },
    { kind: 'updates', label: 'New releases', description: 'A newer Hearthgate is available. Nothing is installed by itself.' },
  ];

  function wanted(kind: EventKind): boolean {
    return !n.events?.length || n.events.includes(kind);
  }

  function toggleKind(kind: EventKind, on: boolean) {
    edit((s) => {
      const current = compactNotifications(s.notifications);
      let events = current.events?.length ? [...current.events] : kinds.map((k) => k.kind);
      events = on ? [...new Set([...events, kind])] : events.filter((k) => k !== kind);
      // All of them is the default, written as no list.
      current.events = events.length === kinds.length ? [] : events;
      s.notifications = current;
    });
  }

  // New webhook form.
  let hookName = $state('');
  let hookUrl = $state('');
  let hookFormat = $state<'json' | 'ntfy'>('json');
  const hookProblem = $derived.by(() => {
    if (!hookName && !hookUrl) return null;
    if (!/^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$/.test(hookName)) return 'Name: lowercase letters, digits and -';
    if ((n.webhooks ?? []).some((w) => w.name === hookName)) return 'A webhook with this name exists';
    if (!/^https?:\/\/\S+$/.test(hookUrl)) return 'The URL must start with https:// or http://';
    return null;
  });

  function addHook() {
    const hook: Webhook = { name: hookName, url: hookUrl.trim() };
    if (hookFormat !== 'json') hook.format = hookFormat;
    edit((s) => {
      const current = compactNotifications(s.notifications);
      current.webhooks = [...(current.webhooks ?? []), hook];
      s.notifications = current;
    });
    hookName = '';
    hookUrl = '';
    hookFormat = 'json';
  }

  function removeHook(name: string) {
    edit((s) => {
      const current = compactNotifications(s.notifications);
      current.webhooks = (current.webhooks ?? []).filter((w) => w.name !== name);
      s.notifications = current;
    });
  }

  // Email, edited as text fields and saved when one loses focus.
  let mail = $state({ server: '', security: 'starttls', username: '', password_file: '', from: '', to: '' });
  $effect(() => {
    const e = n.email;
    mail = {
      server: e?.server ?? '',
      security: e?.security ?? 'starttls',
      username: e?.username ?? '',
      password_file: e?.password_file ?? '',
      from: e?.from ?? '',
      to: (e?.to ?? []).join(', '),
    };
  });
  let emailOn = $state(false);
  $effect(() => {
    emailOn = !!n.email;
  });

  function saveEmail() {
    const to = mail.to
      .split(/[\s,;]+/)
      .map((t) => t.trim())
      .filter(Boolean);
    const email: EmailSettings = { server: mail.server.trim(), from: mail.from.trim(), to };
    if (mail.security !== 'starttls') email.security = mail.security as EmailSettings['security'];
    if (mail.username.trim()) email.username = mail.username.trim();
    if (mail.password_file.trim()) email.password_file = mail.password_file.trim();
    edit((s) => {
      const current = compactNotifications(s.notifications);
      current.email = email;
      s.notifications = current;
    });
  }

  function setEmail(on: boolean) {
    emailOn = on;
    if (on) {
      if (mail.server || mail.from || mail.to) saveEmail();
    } else {
      edit((s) => {
        const current = compactNotifications(s.notifications);
        delete current.email;
        s.notifications = current;
      });
    }
  }

  let testing = $state(false);
  let results = $state<NotificationTestResult[] | null>(null);
  let testError = $state<string | null>(null);
  async function test() {
    testing = true;
    testError = null;
    results = null;
    try {
      results = await api.testNotifications(n);
    } catch (e) {
      testError = e instanceof Error ? e.message : String(e);
    } finally {
      testing = false;
    }
  }
  const anyTarget = $derived((n.webhooks?.length ?? 0) > 0 || !!n.email);
</script>

<Section
  id="notifications"
  title="Notifications"
  description="Where Hearthgate tells you about problems. Each one is sent once, and again after 6 hours if it persists. Changes go into your draft."
>
  {#each kinds as k (k.kind)}
    <SettingRow label={k.label} description={k.description}>
      <Toggle label={k.label} checked={wanted(k.kind)} onchange={(on) => toggleKind(k.kind, on)} />
    </SettingRow>
  {/each}

  <div class="block">
    <h3 class="label secondary">Webhooks</h3>
    <p class="small secondary">
      Hearthgate posts JSON with a <span class="mono-small">text</span> field, which Slack, Mattermost and Discord show
      directly. Choose ntfy for an ntfy topic URL. The URL is stored with the configuration.
    </p>
    {#if n.webhooks?.length}
      <ul class="hooks">
        {#each n.webhooks as hook (hook.name)}
          <li>
            <span class="name">{hook.name}</span>
            <span class="mono-small url">{hook.url}</span>
            <span class="small secondary">{hook.format === 'ntfy' ? 'ntfy' : 'JSON'}</span>
            <Button variant="ghost" size="small" onclick={() => removeHook(hook.name)}>Remove</Button>
          </li>
        {/each}
      </ul>
    {/if}
    <form
      class="add"
      onsubmit={(e) => {
        e.preventDefault();
        if (!hookProblem && hookName) addHook();
      }}
    >
      <TextField label="Name" bind:value={hookName} placeholder="phone" />
      <TextField label="URL" bind:value={hookUrl} placeholder="https://ntfy.sh/your-topic" />
      <Select
        label="Format"
        bind:value={hookFormat}
        options={[
          { value: 'json', label: 'JSON' },
          { value: 'ntfy', label: 'ntfy' },
        ]}
      />
      <Button type="submit" variant="secondary" disabled={!!hookProblem || !hookName}>Add webhook</Button>
    </form>
    {#if hookProblem}<p class="small problem">{hookProblem}</p>{/if}
  </div>

  <SettingRow label="Email" description="Send notifications through your mail server.">
    <Toggle label="Email" checked={emailOn} onchange={setEmail} />
  </SettingRow>
  {#if emailOn}
    <div class="email">
      <TextField label="SMTP server" bind:value={mail.server} onchange={saveEmail} placeholder="smtp.example.com:587" />
      <Select
        label="Security"
        bind:value={mail.security}
        onchange={saveEmail}
        options={[
          { value: 'starttls', label: 'STARTTLS (port 587)' },
          { value: 'tls', label: 'TLS (port 465)' },
          { value: 'none', label: 'None (local relay only)' },
        ]}
      />
      <TextField label="Username" bind:value={mail.username} onchange={saveEmail} placeholder="Optional" />
      <TextField
        label="Password file"
        bind:value={mail.password_file}
        onchange={saveEmail}
        placeholder="/etc/hearthgate/smtp-password"
      />
      <TextField label="From" bind:value={mail.from} onchange={saveEmail} placeholder="hearthgate@example.com" />
      <TextField label="To" bind:value={mail.to} onchange={saveEmail} placeholder="you@example.com" />
    </div>
  {/if}

  <div class="block">
    <div class="test">
      <Button variant="secondary" onclick={test} disabled={!anyTarget || testing}>
        {testing ? 'Sending…' : 'Send a test notification'}
      </Button>
      <span class="small secondary">Uses the settings above, applied or not.</span>
    </div>
    {#if testError}<Alert tone="critical">{testError}</Alert>{/if}
    {#if results}
      <ul class="results">
        {#each results as r (r.target)}
          <li>
            <Alert tone={r.ok ? "neutral" : "critical"}>{r.target}: {r.ok ? 'sent' : r.error}</Alert>
          </li>
        {/each}
      </ul>
    {/if}
  </div>
</Section>

<style>
  .block {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .hooks,
  .results {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-xs);
  }
  .hooks li {
    display: grid;
    grid-template-columns: 120px minmax(0, 1fr) auto auto;
    gap: var(--hg-spacing-md);
    align-items: center;
  }
  .name {
    font-weight: 500;
  }
  .url {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .add {
    display: grid;
    grid-template-columns: 140px minmax(0, 1fr) 120px auto;
    gap: var(--hg-spacing-md);
    align-items: end;
  }
  .email {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--hg-spacing-md);
    padding: 0 var(--hg-spacing-xl) var(--hg-spacing-lg);
  }
  .test {
    display: flex;
    gap: var(--hg-spacing-md);
    align-items: center;
  }
  .problem {
    color: var(--hg-color-status-critical-text);
  }
</style>

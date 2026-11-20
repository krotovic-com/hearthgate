<script lang="ts">
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import Toggle from '../../lib/ui/Toggle.svelte';
  import TextField from '../../lib/ui/TextField.svelte';
  import Select from '../../lib/ui/Select.svelte';
  import Badge from '../../lib/ui/Badge.svelte';
  import { gw, edit } from '../../lib/gateway.svelte';

  const acme = $derived(gw.draft.acme ?? {});
  let email = $state('');
  $effect(() => {
    email = acme.email ?? '';
  });

  function set(change: Record<string, unknown>) {
    edit((s) => {
      s.acme = { ...(s.acme ?? {}), ...change };
    });
  }

  const challenge = $derived(acme.challenge ?? '');
</script>

<Section
  id="certificates"
  title="Certificates"
  description="HTTPS interfaces get certificates from Let's Encrypt automatically, renewed 30 days before they expire. Changes here go into your draft."
>
  <SettingRow label="Accept Let's Encrypt's terms" description="Needed before any certificate is requested.">
    <Toggle label="Accept Let's Encrypt's terms" checked={acme.accept_terms ?? false} onchange={(on) => set({ accept_terms: on })} />
  </SettingRow>
  <SettingRow label="Contact email" description="Let's Encrypt writes here if a certificate is about to expire.">
    <TextField
      label="Contact email"
      hideLabel
      type="email"
      bind:value={email}
      onchange={() => set({ email: email.trim() || null })}
      placeholder="you@example.com"
    />
  </SettingRow>
  <SettingRow label="Challenge" description="How Let's Encrypt checks you own the domain.">
    <Select
      label="Challenge"
      hideLabel
      value={challenge}
      onchange={(e) => set({ challenge: (e.currentTarget as HTMLSelectElement).value || null })}
      options={[
        { value: '', label: 'Automatic: HTTP on port 80 if open, else TLS on 443' },
        { value: 'http-01', label: 'HTTP-01 on port 80' },
        { value: 'tls-alpn-01', label: 'TLS-ALPN-01 on port 443' },
      ]}
    />
  </SettingRow>
  <SettingRow label="Use the staging server" description="Untrusted test certificates with generous limits. For trying things out.">
    <Toggle label="Use the staging server" checked={acme.staging ?? false} onchange={(on) => set({ staging: on })} />
  </SettingRow>
  <div class="list">
    <h3 class="label secondary">Stored certificates</h3>
    {#if gw.certificates.length === 0}
      <p class="small secondary">None yet. They appear after the first HTTPS interface is applied.</p>
    {:else}
      <table>
        <thead>
          <tr><th scope="col">Domains</th><th scope="col">Issuer</th><th scope="col">Expires</th></tr>
        </thead>
        <tbody>
          {#each gw.certificates as c (c.name)}
            <tr>
              <td class="mono-small">{c.domains.join(', ')}</td>
              <td class="small">{c.issuer}{c.source === 'manual' ? ' (uploaded)' : ''}</td>
              <td>
                <Badge status={c.days_left < 7 ? 'critical' : c.days_left < 21 ? 'warning' : 'healthy'} dot>
                  {new Date(c.not_after * 1000).toLocaleDateString()} · {c.days_left} days
                </Badge>
              </td>
            </tr>
          {/each}
        </tbody>
      </table>
      <p class="small secondary">Upload your own with <span class="mono-small">hearthgate cert import</span>.</p>
    {/if}
  </div>
</Section>

<style>
  .list {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl) var(--hg-spacing-xl);
    border-top: 1px solid var(--hg-color-border-default);
  }
  table {
    width: 100%;
    border-collapse: collapse;
  }
  th {
    text-align: left;
    font: 500 11px/16px var(--hg-font-body);
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--hg-color-text-secondary);
    padding: var(--hg-spacing-sm) 0;
  }
  td {
    padding: var(--hg-spacing-sm) var(--hg-spacing-md) var(--hg-spacing-sm) 0;
    border-top: 1px solid var(--hg-color-border-default);
  }
</style>

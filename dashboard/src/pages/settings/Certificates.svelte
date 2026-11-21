<script lang="ts">
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import Toggle from '../../lib/ui/Toggle.svelte';
  import TextField from '../../lib/ui/TextField.svelte';
  import Select from '../../lib/ui/Select.svelte';
  import Badge from '../../lib/ui/Badge.svelte';
  import { gw, edit } from '../../lib/gateway.svelte';
  import type { AcmeDns } from '../../lib/api';

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
  const dns = $derived<AcmeDns>(acme.dns ?? { provider: 'cloudflare' });

  // Text fields of the DNS provider, saved when a field loses focus.
  const dnsFields = ['api_token_file', 'server', 'zone', 'key_name', 'key_file', 'propagation'] as const;
  type DnsField = (typeof dnsFields)[number];
  let text = $state<Record<DnsField, string>>({
    api_token_file: '',
    server: '',
    zone: '',
    key_name: '',
    key_file: '',
    propagation: '',
  });
  $effect(() => {
    for (const f of dnsFields) text[f] = dns[f] ?? '';
  });

  function setDns(change: Partial<AcmeDns>) {
    const next: Record<string, unknown> = { ...dns, ...change };
    for (const [k, v] of Object.entries(next)) {
      if (v === '' || v === null || v === undefined) delete next[k];
    }
    set({ dns: next });
  }

  function setChallenge(value: string) {
    if (value === 'dns-01') {
      set({ challenge: value, dns: acme.dns ?? { provider: 'cloudflare' } });
    } else {
      set({ challenge: value || null, dns: null });
    }
  }
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
      onchange={(e) => setChallenge((e.currentTarget as HTMLSelectElement).value)}
      options={[
        { value: '', label: 'Automatic: HTTP on port 80 if open, else TLS on 443' },
        { value: 'http-01', label: 'HTTP-01 on port 80' },
        { value: 'tls-alpn-01', label: 'TLS-ALPN-01 on port 443' },
        { value: 'dns-01', label: 'DNS-01 with your DNS provider (wildcards, no open ports)' },
      ]}
    />
  </SettingRow>
  {#if challenge === 'dns-01'}
    <SettingRow label="DNS provider" description="Where Hearthgate sets the _acme-challenge TXT records.">
      <Select
        label="DNS provider"
        hideLabel
        value={dns.provider}
        onchange={(e) => setDns({ provider: (e.currentTarget as HTMLSelectElement).value as AcmeDns['provider'] })}
        options={[
          { value: 'cloudflare', label: 'Cloudflare' },
          { value: 'rfc2136', label: 'RFC 2136 (BIND, Knot, PowerDNS and others)' },
        ]}
      />
    </SettingRow>
    {#if dns.provider === 'cloudflare'}
      <SettingRow
        label="API token file"
        description="A file on the server holding a Cloudflare API token with the Zone, DNS, Edit permission. The token itself never goes into the configuration."
      >
        <TextField
          label="API token file"
          hideLabel
          bind:value={text.api_token_file}
          onchange={() => setDns({ api_token_file: text.api_token_file.trim() })}
          placeholder="/etc/hearthgate/cloudflare-token"
        />
      </SettingRow>
    {:else}
      <SettingRow label="DNS server" description="The server that accepts updates, as host or host:port.">
        <TextField
          label="DNS server"
          hideLabel
          bind:value={text.server}
          onchange={() => setDns({ server: text.server.trim() })}
          placeholder="192.168.1.2:53"
        />
      </SettingRow>
      <SettingRow label="Zone" description="The zone holding your domains.">
        <TextField label="Zone" hideLabel bind:value={text.zone} onchange={() => setDns({ zone: text.zone.trim() })} placeholder="example.com" />
      </SettingRow>
      <SettingRow label="TSIG key name" description="As configured on the DNS server.">
        <TextField
          label="TSIG key name"
          hideLabel
          bind:value={text.key_name}
          onchange={() => setDns({ key_name: text.key_name.trim() })}
          placeholder="hearthgate"
        />
      </SettingRow>
      <SettingRow label="TSIG algorithm">
        <Select
          label="TSIG algorithm"
          hideLabel
          value={dns.key_algorithm ?? 'hmac-sha256'}
          onchange={(e) => setDns({ key_algorithm: (e.currentTarget as HTMLSelectElement).value as AcmeDns['key_algorithm'] })}
          options={[
            { value: 'hmac-sha256', label: 'HMAC-SHA256' },
            { value: 'hmac-sha384', label: 'HMAC-SHA384' },
            { value: 'hmac-sha512', label: 'HMAC-SHA512' },
          ]}
        />
      </SettingRow>
      <SettingRow label="TSIG key file" description="A file on the server with the base64 secret, or a BIND key file.">
        <TextField
          label="TSIG key file"
          hideLabel
          bind:value={text.key_file}
          onchange={() => setDns({ key_file: text.key_file.trim() })}
          placeholder="/etc/hearthgate/tsig.key"
        />
      </SettingRow>
    {/if}
    <SettingRow label="Wait for propagation" description="How long the records get to reach the provider's servers before Let's Encrypt checks them.">
      <TextField
        label="Wait for propagation"
        hideLabel
        bind:value={text.propagation}
        onchange={() => setDns({ propagation: text.propagation.trim() })}
        placeholder="30s"
      />
    </SettingRow>
  {/if}
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

<script lang="ts">
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import TextField from '../ui/TextField.svelte';
  import Select from '../ui/Select.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import Icon from '../ui/Icon.svelte';
  import Alert from '../ui/Alert.svelte';
  import type { Interface, ListenerMode, Protocol } from '../api';
  import { gw, draft, edit, routesUsingInterface } from '../gateway.svelte';
  import { list, nameProblem, portProblem, slug } from './common';
  import { modeLabels } from '../format';

  let {
    open = $bindable(false),
    original = null,
  }: { open?: boolean; original?: Interface | null } = $props();

  interface Row {
    port: string;
    mode: ListenerMode;
    protocol: Protocol;
  }

  let name = $state('');
  let nameTouched = $state(false);
  let domains = $state('');
  let rows = $state<Row[]>([]);
  let certificate = $state('');
  let bind = $state('');
  let rateLimit = $state('');
  let acceptTerms = $state(false);
  let email = $state('');
  let tried = $state(false);

  // Reset the form each time it opens.
  $effect(() => {
    if (!open) return;
    const i = original;
    name = i?.name ?? '';
    nameTouched = !!i;
    domains = (i?.domains ?? []).join(', ');
    rows = (i?.listeners ?? [
      { port: 443, mode: 'https' as const },
      { port: 80, mode: 'http' as const },
    ]).map((l) => ({ port: String(l.port), mode: l.mode, protocol: l.protocol ?? 'tcp' }));
    certificate = i?.certificate ?? '';
    bind = (i?.bind ?? []).join(', ');
    rateLimit = i?.rate_limit ? String(i.rate_limit) : '';
    acceptTerms = gw.draft.acme?.accept_terms ?? false;
    email = gw.draft.acme?.email ?? '';
    tried = false;
  });

  // Suggest a name from the first domain until the user types one.
  $effect(() => {
    if (!nameTouched) name = slug(list(domains)[0] ?? '');
  });

  const taken = $derived(draft.interfaces.map((i) => i.name).filter((n) => n !== original?.name));
  const nameError = $derived(nameProblem(name, taken, 'interface'));
  const portErrors = $derived(rows.map((r) => portProblem(r.port)));
  const duplicate = $derived(
    rows.some((r, i) => rows.findIndex((o) => o.port === r.port) !== i) ? 'Each port can be listed once.' : null,
  );
  const usesHttps = $derived(rows.some((r) => r.mode === 'https'));
  const needsDomains = $derived(
    rows.some((r) => r.mode === 'https' || r.mode === 'tls-passthrough') && list(domains).length === 0,
  );
  const manualCerts = $derived(gw.certificates.filter((c) => c.source === 'manual'));
  const needsTerms = $derived(usesHttps && !certificate && !acceptTerms);
  const rateError = $derived(
    rateLimit.trim() === '' || /^[1-9]\d{0,5}$/.test(rateLimit.trim()) ? null : 'Use a whole number above 0, or leave it empty.',
  );
  const valid = $derived(
    !rateError &&
    !nameError && portErrors.every((e) => !e) && !duplicate && rows.length > 0 && !needsDomains && !needsTerms,
  );
  const renamedWithRoutes = $derived(
    original && original.name !== name ? routesUsingInterface(original.name).length : 0,
  );

  const modeOptions = (Object.keys(modeLabels) as ListenerMode[]).map((m) => ({ value: m, label: modeLabels[m]! }));
  const protocolOptions: { value: Protocol; label: string }[] = [
    { value: 'tcp', label: 'TCP' },
    { value: 'udp', label: 'UDP' },
    { value: 'both', label: 'TCP and UDP' },
  ];

  function save() {
    tried = true;
    if (!valid) return;
    const iface: Interface = {
      name,
      domains: list(domains).map((d) => d.toLowerCase()),
      listeners: rows
        .map((r) => ({
          port: Number(r.port),
          mode: r.mode,
          protocol: r.mode === 'raw' ? r.protocol : ('tcp' as Protocol),
        }))
        .sort((a, b) => a.port - b.port),
      bind: list(bind),
      certificate: certificate || null,
      ...(rateLimit.trim() ? { rate_limit: Number(rateLimit) } : {}),
    };
    edit((s) => {
      const at = s.interfaces.findIndex((i) => i.name === original?.name);
      if (at >= 0) s.interfaces[at] = iface;
      else s.interfaces.push(iface);
      if (original && original.name !== name) {
        for (const r of s.routes) if (r.interface === original.name) r.interface = name;
      }
      if (usesHttps && !certificate) {
        s.acme = { ...(s.acme ?? {}), accept_terms: acceptTerms, email: email.trim() || null };
      }
    });
    open = false;
  }
</script>

<Dialog
  bind:open
  title={original ? `Edit ${original.name}` : 'Add a public interface'}
  description="Domains and ports reachable from the internet. Changes are added to your draft; nothing is exposed until you apply them."
>
  <TextField
    label="Domains"
    bind:value={domains}
    mono
    placeholder="cloud.example.com"
    hint="Separate several with commas. Point their DNS records at your public IP address."
    error={tried && needsDomains ? 'HTTPS and TLS passthrough need at least one domain.' : null}
  />
  <TextField
    label="Name"
    bind:value={name}
    mono
    oninput={() => (nameTouched = true)}
    hint="Used by routes and in the history."
    error={tried ? nameError : null}
  />

  <fieldset>
    <legend class="strong">Ports</legend>
    {#each rows as row, i (i)}
      <div class="port-row">
        <TextField label="Port" bind:value={row.port} mono inputmode="numeric" error={tried ? portErrors[i] : null} />
        <Select label="Handles" bind:value={row.mode} options={modeOptions} />
        {#if row.mode === 'raw'}
          <Select label="Protocol" bind:value={row.protocol} options={protocolOptions} />
        {:else}
          <div></div>
        {/if}
        <Button
          variant="ghost"
          size="small"
          aria-label="Remove port {row.port}"
          onclick={() => rows.splice(i, 1)}
          disabled={rows.length === 1}>Remove</Button
        >
      </div>
    {/each}
    {#if tried && duplicate}<p class="small error" role="alert">{duplicate}</p>{/if}
    <div>
      <Button variant="ghost" size="small" onclick={() => rows.push({ port: '', mode: 'raw', protocol: 'tcp' })}>
        <Icon name="plus" /> Add port
      </Button>
    </div>
    <p class="small secondary">
      HTTPS ends TLS here with a certificate. HTTP on port 80 redirects to HTTPS and answers certificate
      challenges. TLS passthrough forwards encrypted traffic by domain. Raw forwards any TCP or UDP traffic.
    </p>
  </fieldset>

  {#if usesHttps}
    <fieldset>
      <legend class="strong">Certificate</legend>
      <Select
        label="Certificate"
        bind:value={certificate}
        options={[
          { value: '', label: "Automatic (Let's Encrypt)" },
          ...manualCerts.map((c) => ({ value: c.name, label: `Uploaded: ${c.name}` })),
        ]}
      />
      {#if !certificate}
        <div class="terms">
          <Toggle label="Accept Let's Encrypt's terms" bind:checked={acceptTerms} />
          <p>
            I accept the <a href="https://letsencrypt.org/repository/" target="_blank" rel="noreferrer"
              >Let's Encrypt subscriber agreement</a
            >
          </p>
        </div>
        <TextField
          label="Contact email (optional)"
          type="email"
          bind:value={email}
          hint="Let's Encrypt writes here if a certificate is about to expire."
        />
        {#if tried && needsTerms}
          <Alert tone="warning">Accept the terms to get a certificate automatically, or choose an uploaded one.</Alert>
        {/if}
      {/if}
    </fieldset>
  {/if}

  <details>
    <summary class="strong">Advanced</summary>
    <TextField
      label="Bind addresses"
      bind:value={bind}
      mono
      placeholder="All addresses"
      hint="Local addresses to listen on. Leave empty to listen on every address."
    />
    <TextField
      label="Rate limit"
      bind:value={rateLimit}
      mono
      inputmode="numeric"
      placeholder="No limit"
      hint="New connections a second from one client address, across these ports. The firewall drops the rest."
      error={tried ? rateError : null}
    />
  </details>

  {#if renamedWithRoutes}
    <Alert tone="neutral">{renamedWithRoutes} route{renamedWithRoutes === 1 ? '' : 's'} will follow the new name.</Alert>
  {/if}

  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Cancel</Button>
    <Button onclick={save}>{original ? 'Save to draft' : 'Add to draft'}</Button>
  {/snippet}
</Dialog>

<style>
  fieldset {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
    margin: 0;
    padding: 0;
    border: none;
  }
  legend {
    padding: 0;
    margin-bottom: var(--hg-spacing-sm);
  }
  .port-row {
    display: grid;
    grid-template-columns: 96px 1fr 140px auto;
    gap: var(--hg-spacing-sm);
    align-items: end;
  }
  .terms {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
  }
  details summary {
    cursor: pointer;
    margin-bottom: var(--hg-spacing-md);
  }
  .error {
    color: var(--hg-color-status-critical-fg);
  }
</style>

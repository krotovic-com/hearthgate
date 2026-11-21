<script lang="ts">
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import TextField from '../ui/TextField.svelte';
  import Select from '../ui/Select.svelte';
  import Alert from '../ui/Alert.svelte';
  import type { FirewallRule, Protocol, RuleAction } from '../api';
  import { draft, editFirewall } from '../gateway.svelte';
  import { list, nameProblem } from './common';
  import { parseNet, parsePort } from '../firewall';

  let {
    open = $bindable(false),
    original = null,
  }: { open?: boolean; original?: FirewallRule | null } = $props();

  let name = $state('');
  let action = $state<RuleAction>('allow');
  let sources = $state('');
  let ports = $state('');
  let protocol = $state<Protocol>('tcp');
  let comment = $state('');
  let tried = $state(false);

  $effect(() => {
    if (!open) return;
    const r = original;
    name = r?.name ?? '';
    action = r?.action ?? 'allow';
    sources = (r?.sources ?? []).join(', ');
    ports = (r?.ports ?? []).join(', ');
    protocol = r?.protocol ?? 'tcp';
    comment = r?.comment ?? '';
    tried = false;
  });

  const taken = $derived(draft.firewall.rules.map((r) => r.name).filter((n) => n !== original?.name));
  const nameError = $derived(nameProblem(name, taken, 'rule'));
  const parsedSources = $derived(list(sources).map((s) => ({ text: s, net: parseNet(s) })));
  const parsedPorts = $derived(list(ports).map((p) => ({ text: p, port: parsePort(p) })));
  const sourceError = $derived.by(() => {
    const bad = parsedSources.find((s) => !s.net);
    return bad ? `${bad.text} is not an address or network, like 203.0.113.7 or 203.0.113.0/24.` : null;
  });
  const portError = $derived.by(() => {
    const bad = parsedPorts.find((p) => p.port === null);
    return bad ? `${bad.text} is not a port or range, like 22 or 8000-8100.` : null;
  });
  const opensEverything = $derived(action === 'allow' && parsedSources.length === 0 && parsedPorts.length === 0);
  const valid = $derived(!nameError && !sourceError && !portError && !opensEverything);

  const actions: { value: RuleAction; label: string }[] = [
    { value: 'allow', label: 'Allow' },
    { value: 'deny', label: 'Block' },
  ];
  const protocols: { value: Protocol; label: string }[] = [
    { value: 'tcp', label: 'TCP' },
    { value: 'udp', label: 'UDP' },
    { value: 'both', label: 'TCP and UDP' },
  ];

  function save() {
    tried = true;
    if (!valid) return;
    const rule: FirewallRule = {
      name,
      action,
      sources: parsedSources.map((s) => s.net!),
      ports: parsedPorts.map((p) => p.port!),
      protocol,
      comment: comment.trim(),
    };
    editFirewall((fw) => {
      const at = fw.rules.findIndex((r) => r.name === original?.name);
      if (at >= 0) fw.rules[at] = rule;
      else fw.rules.push(rule);
    });
    open = false;
  }
</script>

<Dialog
  bind:open
  title={original ? `Edit ${original.name}` : 'Add a firewall rule'}
  description="Rules are checked top to bottom, and the first match decides. Changes are added to your draft."
>
  <div class="row">
    <Select label="Action" bind:value={action} options={actions} />
    <TextField label="Name" bind:value={name} mono error={tried ? nameError : null} placeholder="ssh-from-office" />
  </div>
  <TextField
    label="From"
    bind:value={sources}
    mono
    placeholder="Any address"
    hint="Addresses or networks, separated by commas. Leave empty for any address."
    error={tried ? sourceError : null}
  />
  <div class="row ports">
    <TextField
      label="Ports"
      bind:value={ports}
      mono
      placeholder="Every port"
      hint="Ports or ranges, like 22 or 8000-8100."
      error={tried ? portError : null}
    />
    <Select label="Protocol" bind:value={protocol} options={protocols} />
  </div>
  <TextField label="Note (optional)" bind:value={comment} hint="Shown in the rule list." />
  {#if action === 'allow' && parsedPorts.length > 0 && parsedSources.length === 0}
    <Alert tone="warning">This opens the ports to the whole internet. Public interfaces are the usual way to publish a port.</Alert>
  {/if}
  {#if tried && opensEverything}
    <Alert>An allow rule needs addresses, ports or both. Without them it would open every port to everyone.</Alert>
  {/if}

  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Cancel</Button>
    <Button onclick={save}>{original ? 'Save to draft' : 'Add to draft'}</Button>
  {/snippet}
</Dialog>

<style>
  .row {
    display: grid;
    grid-template-columns: 160px 1fr;
    gap: var(--hg-spacing-md);
    align-items: start;
  }
  .row.ports {
    grid-template-columns: 1fr 160px;
  }
</style>

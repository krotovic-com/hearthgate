<script lang="ts">
  import { onMount } from 'svelte';
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import TextField from '../../lib/ui/TextField.svelte';
  import TextArea from '../../lib/ui/TextArea.svelte';
  import Select from '../../lib/ui/Select.svelte';
  import Button from '../../lib/ui/Button.svelte';
  import Toggle from '../../lib/ui/Toggle.svelte';
  import Alert from '../../lib/ui/Alert.svelte';
  import PayloadPreview from '../../lib/ai/PayloadPreview.svelte';
  import { api, type AiProvider } from '../../lib/api';
  import { providerLabel } from '../../lib/ai';
  import { ai, loadAi } from '../../lib/ai.svelte';
  import { compactAi, editAi, gw } from '../../lib/gateway.svelte';

  onMount(() => {
    loadAi();
  });

  const s = $derived(compactAi(gw.draft.ai) ?? { enabled: false, provider: 'anthropic' as AiProvider });
  const provider = $derived(ai.info?.providers.find((p) => p.id === (s.provider ?? 'anthropic')) ?? null);

  // Text fields, saved into the draft when one loses focus.
  let form = $state({ model: '', endpoint: '', api_key_file: '', redact: '', timeout: '2m' });
  $effect(() => {
    form = {
      model: s.model ?? '',
      endpoint: s.endpoint ?? '',
      api_key_file: s.api_key_file ?? '',
      redact: (s.redact ?? []).join('\n'),
      timeout: s.timeout ?? '2m',
    };
  });

  function save() {
    editAi((a) => {
      a.model = form.model.trim() || null;
      a.endpoint = form.endpoint.trim() || null;
      a.api_key_file = form.api_key_file.trim() || null;
      a.redact = form.redact
        .split('\n')
        .map((l) => l.trim())
        .filter(Boolean);
      a.timeout = form.timeout.trim() || '2m';
    });
  }

  function setProvider(value: string) {
    editAi((a) => {
      if (a.provider !== value) {
        a.provider = value as AiProvider;
        a.model = null;
        a.endpoint = null;
      }
    });
  }

  let key = $state('');
  let keyBusy = $state(false);
  let keyMessage = $state<{ ok: boolean; text: string } | null>(null);
  async function storeKey(value: string) {
    keyBusy = true;
    keyMessage = null;
    try {
      await api.setAiKey(value);
      key = '';
      keyMessage = { ok: true, text: value ? 'Key stored.' : 'Key removed.' };
      await loadAi();
    } catch (e) {
      keyMessage = { ok: false, text: e instanceof Error ? e.message : String(e) };
    } finally {
      keyBusy = false;
    }
  }

  let previewOpen = $state(false);
  const providers: AiProvider[] = ['anthropic', 'openai', 'gemini', 'openai-compatible'];
</script>

<Section
  id="ai"
  title="AI assistant"
  description="Optional. An AI model reviews the setup, the firewall rules and failing services, and suggests fixes you review as a diff. While it is off, Hearthgate never connects to a provider. Changes go into your draft."
>
  <SettingRow label="Use the AI assistant" description="Nothing is sent until you start a review, or the weekly review runs.">
    <Toggle label="Use the AI assistant" checked={s.enabled ?? false} onchange={(on) => editAi((a) => (a.enabled = on))} />
  </SettingRow>

  <div class="grid">
    <Select
      label="Provider"
      value={s.provider ?? 'anthropic'}
      onchange={(e) => setProvider((e.currentTarget as HTMLSelectElement).value)}
      options={providers.map((p) => ({ value: p, label: providerLabel[p] }))}
    />
    <TextField
      label="Model"
      bind:value={form.model}
      onchange={save}
      mono
      placeholder={provider?.default_model ?? 'llama3.1'}
      hint={provider?.default_model ? 'Leave empty for the recommended model.' : 'Required for this provider.'}
    />
    <TextField
      label="Endpoint"
      bind:value={form.endpoint}
      onchange={save}
      mono
      placeholder={provider?.default_endpoint ?? 'http://192.168.1.20:11434/v1'}
      hint={provider?.default_endpoint ? 'Leave empty for the provider’s API.' : 'Such as Ollama at http://HOST:11434/v1.'}
    />
    <TextField label="Answer timeout" bind:value={form.timeout} onchange={save} mono placeholder="2m" hint="Local models can be slow." />
  </div>

  <div class="block">
    <h3 class="label secondary">API key</h3>
    {#if ai.info?.key_from_file}
      <p class="small secondary">Read from <span class="mono-small">{s.api_key_file}</span>. Change it in that file.</p>
    {:else}
      <p class="small secondary">
        {ai.info?.key_set ? 'A key is stored.' : 'No key stored.'}
        The key is kept in Hearthgate’s data directory, readable only by Hearthgate, and never in the configuration or its
        history. It takes effect at once. Local models usually need none.
      </p>
      <form
        class="key"
        onsubmit={(e) => {
          e.preventDefault();
          if (key.trim()) storeKey(key.trim());
        }}
      >
        <TextField label="New API key" type="password" autocomplete="off" bind:value={key} mono />
        <Button type="submit" variant="secondary" busy={keyBusy} disabled={!key.trim()}>Store key</Button>
        {#if ai.info?.key_set}
          <Button variant="ghost" disabled={keyBusy} onclick={() => storeKey('')}>Remove key</Button>
        {/if}
      </form>
      {#if keyMessage}<Alert tone={keyMessage.ok ? 'neutral' : 'critical'}>{keyMessage.text}</Alert>{/if}
    {/if}
    <TextField
      label="Key file (optional)"
      bind:value={form.api_key_file}
      onchange={save}
      mono
      placeholder="/etc/hearthgate/ai-key"
      hint="Read the key from this file instead, for example one managed by your secrets tool."
    />
  </div>

  <div class="block">
    <TextArea
      label="Also hide"
      bind:value={form.redact}
      onchange={save}
      mono
      rows={3}
      hint="One regular expression per line, such as a family name or a project. IP addresses, domains, email addresses, URLs, keys and passwords are always replaced by placeholders."
    />
    <div class="test">
      <Button variant="secondary" onclick={() => (previewOpen = !previewOpen)}>
        {previewOpen ? 'Hide what is sent' : 'Show exactly what is sent'}
      </Button>
      <span class="small secondary">Uses the settings above, applied or not. Sends nothing.</span>
    </div>
    {#if previewOpen}<PayloadPreview scope={{ kind: 'all' }} settings={s} />{/if}
  </div>

  <SettingRow
    label="Weekly review"
    description="Review the whole setup once a week. Results appear in AI review and as a notification."
  >
    <Toggle label="Weekly review" checked={s.weekly_review ?? false} onchange={(on) => editAi((a) => (a.weekly_review = on))} />
  </SettingRow>
</Section>

<style>
  .grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .block {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-sm);
    padding: var(--hg-spacing-lg) var(--hg-spacing-xl);
    border-top: 1px solid var(--hg-color-border-default);
  }
  .block p {
    margin: 0;
  }
  .key {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto auto;
    gap: var(--hg-spacing-md);
    align-items: end;
  }
  .test {
    display: flex;
    gap: var(--hg-spacing-md);
    align-items: center;
  }
</style>

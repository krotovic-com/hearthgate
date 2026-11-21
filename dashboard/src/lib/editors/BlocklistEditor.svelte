<script lang="ts">
  import Dialog from '../ui/Dialog.svelte';
  import Button from '../ui/Button.svelte';
  import TextField from '../ui/TextField.svelte';
  import Toggle from '../ui/Toggle.svelte';
  import Icon from '../ui/Icon.svelte';
  import type { Blocklist } from '../api';
  import { draft, editFirewall } from '../gateway.svelte';
  import { list, nameProblem, slug } from './common';
  import { durationProblem, parseNet, showNet, suggestedBlocklists } from '../firewall';

  let { open = $bindable(false) }: { open?: boolean } = $props();

  let feeds = $state<Blocklist[]>([]);
  let countries = $state('');
  let block = $state('');
  let allow = $state('');
  let adding = $state(false);
  let newName = $state('');
  let newUrl = $state('');
  let newRefresh = $state('12h');
  let tried = $state(false);
  let triedAdd = $state(false);

  $effect(() => {
    if (!open) return;
    const fw = draft.firewall;
    feeds = fw.blocklists.map((b) => ({ refresh: '12h', enabled: true, ...b }));
    countries = fw.countries.map((c) => c.toUpperCase()).join(', ');
    block = fw.block.map(showNet).join(', ');
    allow = fw.allow.map(showNet).join(', ');
    adding = false;
    tried = false;
  });

  const suggestions = $derived(suggestedBlocklists.filter((s) => !feeds.some((f) => f.url === s.url || f.name === s.name)));
  const countryList = $derived(list(countries).map((c) => c.toLowerCase()));
  const countryError = $derived.by(() => {
    const bad = countryList.find((c) => !/^[a-z]{2}$/.test(c));
    return bad ? `${bad.toUpperCase()} is not a two-letter country code, like CN or RU.` : null;
  });
  const netsError = (text: string) => {
    const bad = list(text).find((t) => !parseNet(t));
    return bad ? `${bad} is not an address or network.` : null;
  };
  const blockError = $derived(netsError(block));
  const allowError = $derived(netsError(allow));
  const newNameError = $derived(nameProblem(newName, feeds.map((f) => f.name), 'blocklist'));
  const newUrlError = $derived(/^https:\/\/[^\s/]+\S*$/.test(newUrl.trim()) ? null : 'Use an https:// address.');
  const newRefreshError = $derived(durationProblem(newRefresh));

  $effect(() => {
    // Suggest a name from the address.
    if (adding && !newName && newUrl) {
      try {
        newName = slug(new URL(newUrl).hostname.replace(/^www\./, '').split('.')[0] ?? '');
      } catch {
        // Not a URL yet.
      }
    }
  });

  function addCustom() {
    triedAdd = true;
    if (newNameError || newUrlError || newRefreshError) return;
    feeds.push({ name: newName, url: newUrl.trim(), refresh: newRefresh.trim(), enabled: true });
    adding = false;
    newName = '';
    newUrl = '';
    newRefresh = '12h';
    triedAdd = false;
  }

  function save() {
    tried = true;
    if (countryError || blockError || allowError) return;
    editFirewall((fw) => {
      fw.blocklists = [...$state.snapshot(feeds)].sort((a, b) => (a.name < b.name ? -1 : 1));
      fw.countries = [...new Set(countryList)].sort();
      fw.block = list(block).map((t) => parseNet(t)!);
      fw.allow = list(allow).map((t) => parseNet(t)!);
    });
    open = false;
  }
</script>

<Dialog
  bind:open
  wide
  title="Blocklists"
  description="Addresses on these lists can't reach any port. Lists are downloaded again on a schedule. Changes are added to your draft."
>
  <fieldset>
    <legend class="strong">Subscribed lists</legend>
    {#if feeds.length === 0}
      <p class="small secondary">No lists yet. Add one of the suggestions below, or your own.</p>
    {/if}
    <ul class="feeds">
      {#each feeds as feed, i (feed.name)}
        <li class="feed">
          <Toggle label="Use {feed.name}" bind:checked={feed.enabled} />
          <div class="text">
            <p class="strong">{feed.name}</p>
            <p class="mono-small secondary url">{feed.url} · every {feed.refresh}</p>
          </div>
          <Button variant="ghost" size="small" onclick={() => feeds.splice(i, 1)}>
            Remove<span class="visually-hidden"> {feed.name}</span>
          </Button>
        </li>
      {/each}
    </ul>
    {#if suggestions.length}
      <p class="small strong">Suggested</p>
      <ul class="feeds">
        {#each suggestions as s (s.name)}
          <li class="feed">
            <div class="text">
              <p class="strong">{s.name}</p>
              <p class="small secondary">{s.description}</p>
            </div>
            <Button
              variant="secondary"
              size="small"
              onclick={() => feeds.push({ name: s.name, url: s.url, refresh: '12h', enabled: true })}
            >
              Add<span class="visually-hidden"> {s.name}</span>
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
    {#if adding}
      <div class="custom">
        <TextField label="Address" bind:value={newUrl} mono placeholder="https://example.com/list.txt" error={triedAdd ? newUrlError : null} hint="One address or network per line. Comments after # are ignored." />
        <div class="row">
          <TextField label="Name" bind:value={newName} mono error={triedAdd ? newNameError : null} />
          <TextField label="Download every" bind:value={newRefresh} mono error={triedAdd ? newRefreshError : null} />
        </div>
        <div class="buttons">
          <Button variant="secondary" size="small" onclick={() => (adding = false)}>Cancel</Button>
          <Button size="small" onclick={addCustom}>Add list</Button>
        </div>
      </div>
    {:else}
      <div>
        <Button variant="ghost" size="small" onclick={() => (adding = true)}><Icon name="plus" /> Add your own list</Button>
      </div>
    {/if}
  </fieldset>

  <TextField
    label="Countries"
    bind:value={countries}
    mono
    placeholder="None"
    hint="Two-letter codes, separated by commas. Address ranges come from ipdeny.com."
    error={tried ? countryError : null}
  />
  <TextField
    label="Always block"
    bind:value={block}
    mono
    placeholder="None"
    hint="Addresses or networks that never reach any port."
    error={tried ? blockError : null}
  />
  <TextField
    label="Never block"
    bind:value={allow}
    mono
    placeholder="None"
    hint="Addresses that skip blocklists, countries and bans, like your office. They still only reach open ports."
    error={tried ? allowError : null}
  />

  {#snippet footer()}
    <Button variant="secondary" onclick={() => (open = false)}>Cancel</Button>
    <Button onclick={save}>Save to draft</Button>
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
  .feeds {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    border: 1px solid var(--hg-color-border-default);
    border-radius: var(--hg-radius-md);
  }
  .feeds:empty {
    display: none;
  }
  .feed {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-md);
  }
  .feed + .feed {
    border-top: 1px solid var(--hg-color-border-default);
  }
  .text {
    flex: 1;
    min-width: 0;
  }
  .url {
    overflow-wrap: anywhere;
  }
  .custom {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
    padding: var(--hg-spacing-md);
    background: var(--hg-color-bg-canvas);
    border-radius: var(--hg-radius-md);
  }
  .row {
    display: grid;
    grid-template-columns: 1fr 160px;
    gap: var(--hg-spacing-md);
    align-items: start;
  }
  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: var(--hg-spacing-sm);
  }
</style>

<script lang="ts">
  import Section from '../../lib/ui/Section.svelte';
  import SettingRow from '../../lib/ui/SettingRow.svelte';
  import { live } from '../../lib/live.svelte';
  import { gw } from '../../lib/gateway.svelte';
  import { duration } from '../../lib/format';
</script>

<Section id="general" title="General" description="This gateway and its configuration.">
  <SettingRow label="Host" description={live.status?.host.os ?? undefined}>
    <span class="mono-small">{live.status?.host.hostname ?? '…'}</span>
  </SettingRow>
  <SettingRow label="Hearthgate" description="Running since the last restart.">
    <span class="mono-small">{live.status ? `${live.status.version} · up ${duration(live.status.host.uptime)}` : '…'}</span>
  </SettingRow>
  <SettingRow
    label="Configuration file"
    description="The running configuration as TOML. Restore it with `hearthgate import FILE`."
  >
    <a class="download" href="/api/v1/state/export" download>Export revision {gw.revision?.id ?? 0}</a>
  </SettingRow>
</Section>

<style>
  .download {
    padding: calc(var(--hg-spacing-sm) - 1px) calc(var(--hg-spacing-lg) - 1px);
    border: 1px solid var(--hg-color-border-strong);
    border-radius: var(--hg-radius-md);
    color: var(--hg-color-text-primary);
    font: 500 14px/20px var(--hg-font-body);
    text-decoration: none;
    white-space: nowrap;
  }
  .download:hover {
    background: var(--hg-color-bg-subtle);
  }
</style>

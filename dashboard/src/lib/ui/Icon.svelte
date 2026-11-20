<script lang="ts" module>
  // Icons from the Figma kit (Components page, Icon/*). Strokes use
  // currentColor, so an icon takes the color of its text.
  const files = import.meta.glob('../icons/*.svg', { query: '?raw', import: 'default', eager: true }) as Record<string, string>;
  const icons: Record<string, string> = {};
  for (const [path, svg] of Object.entries(files)) {
    icons[path.replace('../icons/', '').replace('.svg', '')] = svg;
  }
  export type IconName =
    | 'overview' | 'globe' | 'server' | 'route' | 'vpn' | 'firewall' | 'peers' | 'settings'
    | 'plus' | 'chevron' | 'lock' | 'more' | 'arrow' | 'container' | 'terminal' | 'ai';
</script>

<script lang="ts">
  let { name, label }: { name: IconName; label?: string } = $props();
</script>

<span class="icon" role={label ? 'img' : undefined} aria-label={label} aria-hidden={label ? undefined : 'true'}>
  {@html icons[name]}
</span>

<style>
  .icon {
    display: inline-flex;
    width: 16px;
    height: 16px;
    flex-shrink: 0;
  }
  .icon :global(svg) {
    display: block;
  }
</style>

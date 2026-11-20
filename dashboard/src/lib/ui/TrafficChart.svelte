<script lang="ts">
  import type { TrafficBucket } from '../api';
  import { clock, number } from '../format';

  let { buckets }: { buckets: TrafficBucket[] } = $props();

  const max = $derived(Math.max(1, ...buckets.map((b) => b.requests)));
  const total = $derived(buckets.reduce((s, b) => s + b.requests, 0));
  const failed = $derived(buckets.reduce((s, b) => s + b.failures, 0));
  const height = (n: number) => `h${Math.round((n / max) * 100)}`;
  const failHeight = (b: TrafficBucket) =>
    `h${Math.round((Math.min(b.failures, b.requests) / Math.max(1, b.requests)) * 100)}`;
  const label = (b: TrafficBucket) => clock(new Date(b.start * 1000));
</script>

<div class="chart">
  <ul class="legend small secondary" aria-hidden="true">
    <li><span class="swatch ok"></span>Requests and connections</li>
    <li><span class="swatch failed"></span>Failed</li>
  </ul>
  <div
    class="bars"
    role="img"
    aria-label="{number(total)} requests and connections in the last 24 hours, {number(failed)} failed."
  >
    {#each buckets as b (b.start)}
      <div class="bar" title="{label(b)}: {number(b.requests)} requests, {number(b.failures)} failed">
        <div class="stack {height(b.requests)}">
          <div class="fail {failHeight(b)}"></div>
        </div>
      </div>
    {/each}
  </div>
  <div class="axis mono-small secondary" aria-hidden="true">
    {#each buckets as b, i (b.start)}
      <span>{i % 6 === 0 ? label(b) : i === buckets.length - 1 ? 'now' : ''}</span>
    {/each}
  </div>
  <table class="visually-hidden">
    <caption>Traffic per hour</caption>
    <thead><tr><th>Hour</th><th>Requests</th><th>Failed</th></tr></thead>
    <tbody>
      {#each buckets as b (b.start)}
        <tr><td>{label(b)}</td><td>{b.requests}</td><td>{b.failures}</td></tr>
      {/each}
    </tbody>
  </table>
</div>

<style>
  .chart {
    display: flex;
    flex-direction: column;
    gap: var(--hg-spacing-md);
  }
  .legend {
    display: flex;
    gap: var(--hg-spacing-lg);
    list-style: none;
    margin: 0;
    padding: 0;
  }
  .legend li {
    display: flex;
    align-items: center;
    gap: var(--hg-spacing-xs);
  }
  .swatch {
    width: 10px;
    height: 10px;
    border-radius: 2px;
  }
  .ok {
    background: var(--hg-color-accent-default);
  }
  .failed {
    background: var(--hg-color-status-critical-fg);
  }
  .bars {
    display: grid;
    grid-template-columns: repeat(24, 1fr);
    gap: 4px;
    height: 220px;
    align-items: end;
  }
  .bar {
    height: 100%;
    display: flex;
    align-items: flex-end;
  }
  .stack {
    width: 100%;
    min-height: 2px;
    background: var(--hg-color-accent-default);
    border-radius: 2px 2px 0 0;
    display: flex;
    align-items: flex-end;
    overflow: hidden;
  }
  .fail {
    width: 100%;
    background: var(--hg-color-status-critical-fg);
  }
  .axis {
    display: grid;
    grid-template-columns: repeat(24, 1fr);
    gap: 4px;
    white-space: nowrap;
  }
  /* Heights in whole percent: no inline styles under the page's CSP. */
  .bars .h0 {
    height: 0%;
  }
  .bars .h1 {
    height: 1%;
  }
  .bars .h2 {
    height: 2%;
  }
  .bars .h3 {
    height: 3%;
  }
  .bars .h4 {
    height: 4%;
  }
  .bars .h5 {
    height: 5%;
  }
  .bars .h6 {
    height: 6%;
  }
  .bars .h7 {
    height: 7%;
  }
  .bars .h8 {
    height: 8%;
  }
  .bars .h9 {
    height: 9%;
  }
  .bars .h10 {
    height: 10%;
  }
  .bars .h11 {
    height: 11%;
  }
  .bars .h12 {
    height: 12%;
  }
  .bars .h13 {
    height: 13%;
  }
  .bars .h14 {
    height: 14%;
  }
  .bars .h15 {
    height: 15%;
  }
  .bars .h16 {
    height: 16%;
  }
  .bars .h17 {
    height: 17%;
  }
  .bars .h18 {
    height: 18%;
  }
  .bars .h19 {
    height: 19%;
  }
  .bars .h20 {
    height: 20%;
  }
  .bars .h21 {
    height: 21%;
  }
  .bars .h22 {
    height: 22%;
  }
  .bars .h23 {
    height: 23%;
  }
  .bars .h24 {
    height: 24%;
  }
  .bars .h25 {
    height: 25%;
  }
  .bars .h26 {
    height: 26%;
  }
  .bars .h27 {
    height: 27%;
  }
  .bars .h28 {
    height: 28%;
  }
  .bars .h29 {
    height: 29%;
  }
  .bars .h30 {
    height: 30%;
  }
  .bars .h31 {
    height: 31%;
  }
  .bars .h32 {
    height: 32%;
  }
  .bars .h33 {
    height: 33%;
  }
  .bars .h34 {
    height: 34%;
  }
  .bars .h35 {
    height: 35%;
  }
  .bars .h36 {
    height: 36%;
  }
  .bars .h37 {
    height: 37%;
  }
  .bars .h38 {
    height: 38%;
  }
  .bars .h39 {
    height: 39%;
  }
  .bars .h40 {
    height: 40%;
  }
  .bars .h41 {
    height: 41%;
  }
  .bars .h42 {
    height: 42%;
  }
  .bars .h43 {
    height: 43%;
  }
  .bars .h44 {
    height: 44%;
  }
  .bars .h45 {
    height: 45%;
  }
  .bars .h46 {
    height: 46%;
  }
  .bars .h47 {
    height: 47%;
  }
  .bars .h48 {
    height: 48%;
  }
  .bars .h49 {
    height: 49%;
  }
  .bars .h50 {
    height: 50%;
  }
  .bars .h51 {
    height: 51%;
  }
  .bars .h52 {
    height: 52%;
  }
  .bars .h53 {
    height: 53%;
  }
  .bars .h54 {
    height: 54%;
  }
  .bars .h55 {
    height: 55%;
  }
  .bars .h56 {
    height: 56%;
  }
  .bars .h57 {
    height: 57%;
  }
  .bars .h58 {
    height: 58%;
  }
  .bars .h59 {
    height: 59%;
  }
  .bars .h60 {
    height: 60%;
  }
  .bars .h61 {
    height: 61%;
  }
  .bars .h62 {
    height: 62%;
  }
  .bars .h63 {
    height: 63%;
  }
  .bars .h64 {
    height: 64%;
  }
  .bars .h65 {
    height: 65%;
  }
  .bars .h66 {
    height: 66%;
  }
  .bars .h67 {
    height: 67%;
  }
  .bars .h68 {
    height: 68%;
  }
  .bars .h69 {
    height: 69%;
  }
  .bars .h70 {
    height: 70%;
  }
  .bars .h71 {
    height: 71%;
  }
  .bars .h72 {
    height: 72%;
  }
  .bars .h73 {
    height: 73%;
  }
  .bars .h74 {
    height: 74%;
  }
  .bars .h75 {
    height: 75%;
  }
  .bars .h76 {
    height: 76%;
  }
  .bars .h77 {
    height: 77%;
  }
  .bars .h78 {
    height: 78%;
  }
  .bars .h79 {
    height: 79%;
  }
  .bars .h80 {
    height: 80%;
  }
  .bars .h81 {
    height: 81%;
  }
  .bars .h82 {
    height: 82%;
  }
  .bars .h83 {
    height: 83%;
  }
  .bars .h84 {
    height: 84%;
  }
  .bars .h85 {
    height: 85%;
  }
  .bars .h86 {
    height: 86%;
  }
  .bars .h87 {
    height: 87%;
  }
  .bars .h88 {
    height: 88%;
  }
  .bars .h89 {
    height: 89%;
  }
  .bars .h90 {
    height: 90%;
  }
  .bars .h91 {
    height: 91%;
  }
  .bars .h92 {
    height: 92%;
  }
  .bars .h93 {
    height: 93%;
  }
  .bars .h94 {
    height: 94%;
  }
  .bars .h95 {
    height: 95%;
  }
  .bars .h96 {
    height: 96%;
  }
  .bars .h97 {
    height: 97%;
  }
  .bars .h98 {
    height: 98%;
  }
  .bars .h99 {
    height: 99%;
  }
  .bars .h100 {
    height: 100%;
  }
</style>

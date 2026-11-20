// Firewall settings in the draft: defaults, address parsing and the order
// rules are checked in, as shown on the Firewall page.

import type { FirewallRule, FirewallSettings, Interface, Protocol } from './api';
import { portLabel } from './format';

export const defaultFirewall = (): FirewallSettings => ({
  enabled: false,
  allow_lan: true,
  allow_ping: true,
  allow_docker_ports: false,
  auto_ban: { enabled: true, threshold: 20, window: '10m', ban_time: '1h' },
});

/** The firewall settings with every field filled in. */
export function withDefaults(fw: FirewallSettings | undefined): Required<FirewallSettings> {
  const d = defaultFirewall();
  return {
    rules: [],
    allow: [],
    block: [],
    blocklists: [],
    countries: [],
    ...d,
    ...(fw ?? {}),
    auto_ban: { ...d.auto_ban, ...(fw?.auto_ban ?? {}) },
  } as Required<FirewallSettings>;
}

/**
 * The settings as the server stores them: empty lists and comments left
 * out, and nothing at all when everything has its default value. Keeps
 * the draft comparable with the running state.
 */
export function compact(fw: Required<FirewallSettings>): FirewallSettings | undefined {
  const out: FirewallSettings = {
    enabled: fw.enabled,
    allow_lan: fw.allow_lan,
    allow_ping: fw.allow_ping,
    allow_docker_ports: fw.allow_docker_ports,
    auto_ban: fw.auto_ban,
  };
  if (fw.rules.length) {
    out.rules = fw.rules.map((r) => {
      const rule: FirewallRule = { name: r.name, action: r.action, protocol: r.protocol ?? 'tcp' };
      if (r.sources?.length) rule.sources = r.sources;
      if (r.ports?.length) rule.ports = r.ports;
      if (r.comment) rule.comment = r.comment;
      return rule;
    });
  }
  if (fw.allow.length) out.allow = fw.allow;
  if (fw.block.length) out.block = fw.block;
  if (fw.blocklists.length) out.blocklists = fw.blocklists.map((b) => ({ refresh: '12h', enabled: true, ...b }));
  if (fw.countries.length) out.countries = fw.countries;
  return canonical(out) === canonical(defaultFirewall()) ? undefined : out;
}

/** JSON with sorted keys, so key order never makes two values differ. */
export function canonical(value: unknown): string {
  return JSON.stringify(value, (_key, v: unknown) =>
    v && typeof v === 'object' && !Array.isArray(v)
      ? Object.fromEntries(
          Object.entries(v as Record<string, unknown>)
            .filter(([, x]) => x !== undefined)
            .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
        )
      : v,
  );
}

const ipv4 = /^(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)(\.(25[0-5]|2[0-4]\d|1\d\d|[1-9]?\d)){3}$/;
const ipv6 = /^[0-9a-f:]+(:\d+\.\d+\.\d+\.\d+)?$/i;

/**
 * "203.0.113.5" → "203.0.113.5/32", "2001:db8::/32" stays. Null when the
 * text is not an address or network.
 */
export function parseNet(text: string): string | null {
  const t = text.trim().toLowerCase();
  const [addr, prefix, extra] = t.split('/');
  if (extra !== undefined || !addr) return null;
  const v4 = ipv4.test(addr);
  const v6 = !v4 && addr.includes(':') && ipv6.test(addr) && (addr.match(/::/g)?.length ?? 0) <= 1;
  if (!v4 && !v6) return null;
  const max = v4 ? 32 : 128;
  if (prefix === undefined) return `${addr}/${max}`;
  if (!/^\d{1,3}$/.test(prefix) || Number(prefix) > max) return null;
  return `${addr}/${Number(prefix)}`;
}

/** "22" → 22, "8000-8100" stays a string. Null when not a port or range. */
export function parsePort(text: string): number | string | null {
  const t = text.trim();
  const m = /^(\d{1,5})(?:\s*-\s*(\d{1,5}))?$/.exec(t);
  if (!m) return null;
  const first = Number(m[1]);
  const last = m[2] === undefined ? first : Number(m[2]);
  if (first < 1 || last > 65535 || first > last) return null;
  return first === last ? first : `${first}-${last}`;
}

/** "/32" and "/128" read as plain addresses. */
export function showNet(net: string): string {
  return net.replace(/\/(32|128)$/, (m, p: string) => (net.includes(':') === (p === '128') ? '' : m));
}

export function ruleMatch(rule: FirewallRule): string {
  const from = rule.sources?.length ? rule.sources.map(showNet).join(', ') : 'any address';
  const proto = rule.protocol ?? 'tcp';
  const to = rule.ports?.length
    ? rule.ports.map((p) => portLabel(p as number, proto)).join(', ')
    : proto === 'both'
      ? 'every port'
      : `every ${proto.toUpperCase()} port`;
  return `From ${from} to ${to}`;
}

export type RowKind = 'allow' | 'block' | 'limit' | 'open';

/** One line of the rule list, user rules and built-in steps alike. */
export interface RuleRow {
  key: string;
  kind: RowKind;
  title: string;
  detail: string;
  /** Index into `rules` for rows the user can edit, move and remove. */
  index?: number;
}

/** Everything the firewall checks, in the order it checks it. */
export function ruleRows(fw: Required<FirewallSettings>, interfaces: Interface[]): RuleRow[] {
  const rows: RuleRow[] = [];
  if (fw.allow.length) {
    rows.push({
      key: 'allow',
      kind: 'allow',
      title: 'Never block',
      detail: `${fw.allow.map(showNet).join(', ')} skip blocklists and bans`,
    });
  }
  const lists = fw.blocklists.filter((b) => b.enabled !== false);
  if (lists.length || fw.block.length || fw.countries.length) {
    const parts = [
      ...(lists.length ? [lists.map((b) => b.name).join(', ')] : []),
      ...(fw.countries.length ? [`countries ${fw.countries.map((c) => c.toUpperCase()).join(', ')}`] : []),
      ...(fw.block.length ? [fw.block.map(showNet).join(', ')] : []),
    ];
    rows.push({ key: 'lists', kind: 'block', title: 'Blocklists', detail: parts.join(' · ') });
  }
  if (fw.auto_ban.enabled) {
    rows.push({
      key: 'bans',
      kind: 'block',
      title: 'Banned clients',
      detail: `${fw.auto_ban.threshold} points within ${fw.auto_ban.window} bans for ${fw.auto_ban.ban_time}`,
    });
  }
  if (fw.allow_lan) {
    rows.push({ key: 'lan', kind: 'allow', title: 'Local network', detail: 'Everything from the LAN, like SSH' });
  }
  fw.rules.forEach((r, index) => {
    rows.push({
      key: `rule-${r.name}`,
      kind: r.action === 'allow' ? 'allow' : 'block',
      title: r.name,
      detail: r.comment ? `${ruleMatch(r)} · ${r.comment}` : ruleMatch(r),
      index,
    });
  });
  for (const i of interfaces) {
    if (i.rate_limit) {
      rows.push({
        key: `limit-${i.name}`,
        kind: 'limit',
        title: `Rate limit ${i.name}`,
        detail: `${i.rate_limit} new connections a second per client`,
      });
    }
  }
  const ports = interfaces.flatMap((i) => i.listeners.map((l) => portLabel(l.port, l.protocol)));
  rows.push({
    key: 'open',
    kind: 'open',
    title: 'Public interfaces',
    detail: ports.length ? `Open ${[...new Set(ports)].join(', ')}` : 'No ports open',
  });
  return rows;
}

/** Open public ports, as "port/protocol" keys. */
export function openPorts(interfaces: Interface[]): Set<string> {
  const out = new Set<string>();
  for (const i of interfaces) {
    for (const l of i.listeners) {
      const p: Protocol = l.protocol ?? 'tcp';
      if (p === 'tcp' || p === 'both') out.add(`${l.port}/tcp`);
      if (p === 'udp' || p === 'both') out.add(`${l.port}/udp`);
    }
  }
  return out;
}

/** Duration text the server accepts, like "10m", "1h 30m" or "7d". */
export function durationProblem(text: string): string | null {
  return /^(\d+\s*(s|m|h|d|w)\s*)+$/i.test(text.trim()) ? null : 'Use a duration like 10m, 1h or 7d.';
}

/** Feeds offered in the dashboard (the same list as the server's). */
export const suggestedBlocklists = [
  {
    name: 'spamhaus-drop',
    url: 'https://www.spamhaus.org/drop/drop_v4.json',
    description: 'Spamhaus DROP: networks hijacked or leased by professional spam and crime operations',
  },
  { name: 'spamhaus-drop-v6', url: 'https://www.spamhaus.org/drop/drop_v6.json', description: 'Spamhaus DROP for IPv6' },
  {
    name: 'firehol-level1',
    url: 'https://raw.githubusercontent.com/firehol/blocklist-ipsets/master/firehol_level1.netset',
    description: 'FireHOL level 1: a conservative mix of attack, malware and abuse lists',
  },
  {
    name: 'feodo-tracker',
    url: 'https://feodotracker.abuse.ch/downloads/ipblocklist_recommended.txt',
    description: 'abuse.ch Feodo Tracker: botnet command and control servers',
  },
];

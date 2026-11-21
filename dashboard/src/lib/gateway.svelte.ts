// The desired state and a draft of changes to it. Edits go into the draft;
// nothing reaches the gateway until the user reviews the diff and applies.

import {
  api,
  type CertificateInfo,
  type DesiredState,
  type Interface,
  type RevisionInfo,
  type Route,
  type Service,
  type FirewallSettings,
  type ManagedContainer,
  type Stack,
  type NotificationSettings,
  type EmailSettings,
} from './api';
import { canonical, compact, withDefaults } from './firewall';
import { compactDns, compactVpn, dnsWithDefaults, vpnWithDefaults, type FullDns, type FullVpn } from './vpn';

export interface Gateway {
  loaded: boolean;
  error: string | null;
  /** The current revision's state. */
  current: DesiredState;
  revision: RevisionInfo | null;
  /** The working copy the editors change. */
  draft: DesiredState;
  certificates: CertificateInfo[];
  revisions: RevisionInfo[];
}

export const gw = $state<Gateway>({
  loaded: false,
  error: null,
  current: {},
  revision: null,
  draft: {},
  certificates: [],
  revisions: [],
});

const clone = <T>(v: T): T => JSON.parse(JSON.stringify(v)) as T;

/** Fills every list so the editors never deal with missing arrays. */
function complete(
  state: DesiredState,
): Required<Pick<DesiredState, 'interfaces' | 'services' | 'routes' | 'containers' | 'stacks'>> & DesiredState {
  return {
    ...state,
    interfaces: state.interfaces ?? [],
    services: state.services ?? [],
    routes: state.routes ?? [],
    containers: state.containers ?? [],
    stacks: state.stacks ?? [],
  };
}

export const draft = {
  get interfaces(): Interface[] {
    return gw.draft.interfaces ?? [];
  },
  get services(): Service[] {
    return gw.draft.services ?? [];
  },
  get routes(): Route[] {
    return gw.draft.routes ?? [];
  },
  get firewall(): Required<FirewallSettings> {
    return withDefaults(gw.draft.firewall);
  },
  get upnp(): boolean {
    return gw.draft.router?.upnp ?? false;
  },
  get containers(): ManagedContainer[] {
    return gw.draft.containers ?? [];
  },
  get stacks(): Stack[] {
    return gw.draft.stacks ?? [];
  },
  get vpn(): FullVpn {
    return vpnWithDefaults(gw.draft.vpn);
  },
  get dns(): FullDns {
    return dnsWithDefaults(gw.draft.dns);
  },
};

export function isDirty(): boolean {
  return canonical(complete(gw.draft)) !== canonical(complete(gw.current));
}

export async function loadGateway(): Promise<void> {
  try {
    const [state, certificates, revisions] = await Promise.all([
      api.state(),
      api.certificates(),
      api.revisions(50),
    ]);
    const dirty = gw.loaded && isDirty();
    gw.current = complete(state.state);
    gw.revision = state.revision;
    if (!dirty) gw.draft = clone(gw.current);
    gw.certificates = certificates;
    gw.revisions = revisions;
    gw.loaded = true;
    gw.error = null;
  } catch (e) {
    gw.error = e instanceof Error ? e.message : String(e);
  }
}

/** Changes the draft. */
export function edit(change: (state: ReturnType<typeof complete>) => void): void {
  const next = complete(clone($state.snapshot(gw.draft)));
  change(next);
  // Same order as the server keeps, so unchanged lists compare equal.
  const byName = (a: { name: string }, b: { name: string }) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
  next.interfaces.sort(byName);
  next.services.sort(byName);
  next.routes.sort(byName);
  // Containers and stacks as the server stores them: defaults left out.
  const containers = (next.containers ?? []).map(compactContainer).sort(byName);
  if (containers.length) next.containers = containers;
  else delete (next as DesiredState).containers;
  const stacks = (next.stacks ?? []).map(compactStack).sort(byName);
  if (stacks.length) next.stacks = stacks;
  else delete (next as DesiredState).stacks;
  // Stored as the server stores them, so an unchanged draft compares equal.
  const fw = next.firewall ? compact(withDefaults(next.firewall)) : undefined;
  if (fw) next.firewall = fw;
  else delete next.firewall;
  if (!next.router?.upnp) delete next.router;
  const vpn = compactVpn(vpnWithDefaults(next.vpn));
  if (vpn) next.vpn = vpn;
  else delete next.vpn;
  const dns = compactDns(dnsWithDefaults(next.dns));
  if (dns) next.dns = dns;
  else delete next.dns;
  const notifications = compactNotifications(next.notifications);
  if (Object.keys(notifications).length) next.notifications = notifications;
  else delete next.notifications;
  for (const i of next.interfaces) if (!i.rate_limit) delete i.rate_limit;
  gw.draft = next;
}

function compactContainer(c: ManagedContainer): ManagedContainer {
  const out: ManagedContainer = { ...c };
  if (out.running !== false) delete out.running;
  if (!out.command?.length) delete out.command;
  if (!out.env || !Object.keys(out.env).length) delete out.env;
  if (!out.volumes?.length) delete out.volumes;
  if (!out.publish?.length) delete out.publish;
  if (!out.restart || out.restart === 'unless-stopped') delete out.restart;
  return out;
}

function compactStack(s: Stack): Stack {
  const out: Stack = { ...s };
  if (out.running !== false) delete out.running;
  if (!out.env || !Object.keys(out.env).length) delete out.env;
  return out;
}

/** Changes the draft's firewall settings. */
export function editFirewall(change: (fw: Required<FirewallSettings>) => void): void {
  edit((s) => {
    const fw = withDefaults(s.firewall);
    change(fw);
    s.firewall = fw;
  });
}

/** Changes the draft's VPN and DNS settings. */
export function editVpn(change: (vpn: FullVpn, dns: FullDns) => void): void {
  edit((s) => {
    const vpn = vpnWithDefaults(s.vpn);
    const dns = dnsWithDefaults(s.dns);
    change(vpn, dns);
    s.vpn = vpn;
    s.dns = dns;
  });
}

export function discard(): void {
  gw.draft = clone($state.snapshot(gw.current));
}

/** Stores the draft as a new revision. */
export async function applyDraft(message: string): Promise<RevisionInfo> {
  const result = await api.apply($state.snapshot(gw.draft), message);
  await loadGateway();
  gw.draft = clone($state.snapshot(gw.current));
  return result.revision;
}

export async function rollbackTo(id: number): Promise<RevisionInfo> {
  const result = await api.rollback(id);
  gw.draft = {};
  gw.loaded = false;
  await loadGateway();
  return result.revision;
}

/** Names used by routes, so deleting them can be refused or cascaded. */
export function routesUsingInterface(name: string): Route[] {
  return draft.routes.filter((r) => r.interface === name);
}

export function routesUsingService(name: string): Route[] {
  return draft.routes.filter((r) => r.targets.some((t) => t.service === name));
}

/** A short list of what differs between the draft and the current state. */
export function draftSummary(): string[] {
  const out: string[] = [];
  const cur = complete(gw.current);
  const next = complete(gw.draft);
  const kinds = [
    ['interfaces', 'public interface'],
    ['services', 'service'],
    ['routes', 'route'],
    ['containers', 'container'],
    ['stacks', 'stack'],
  ] as const;
  for (const [key, label] of kinds) {
    const before = new Map(cur[key].map((x) => [x.name, canonical(x)]));
    const after = new Map(next[key].map((x) => [x.name, canonical(x)]));
    for (const [name, json] of after) {
      if (!before.has(name)) out.push(`Add ${label} ${name}`);
      else if (before.get(name) !== json) out.push(`Change ${label} ${name}`);
    }
    for (const name of before.keys()) if (!after.has(name)) out.push(`Remove ${label} ${name}`);
  }
  if (canonical(cur.acme ?? {}) !== canonical(next.acme ?? {})) out.push('Change certificate settings');
  if (canonical(cur.limits ?? {}) !== canonical(next.limits ?? {})) out.push('Change limits');
  out.push(...firewallSummary(withDefaults(cur.firewall), withDefaults(next.firewall)));
  if ((cur.router?.upnp ?? false) !== (next.router?.upnp ?? false)) {
    out.push(next.router?.upnp ? 'Turn on router port forwarding' : 'Turn off router port forwarding');
  }
  out.push(...vpnSummary(vpnWithDefaults(cur.vpn), vpnWithDefaults(next.vpn)));
  if (canonical(dnsWithDefaults(cur.dns)) !== canonical(dnsWithDefaults(next.dns))) out.push('Change internal DNS');
  if (canonical(compactNotifications(cur.notifications)) !== canonical(compactNotifications(next.notifications))) {
    out.push('Change notifications');
  }
  return out;
}

/** Notification settings as the server stores them (empty parts left out). */
export function compactNotifications(n: NotificationSettings | undefined): NotificationSettings {
  const out: NotificationSettings = {};
  if (n?.events?.length) out.events = [...n.events];
  if (n?.webhooks?.length) {
    out.webhooks = n.webhooks.map((w) => (w.format && w.format !== 'json' ? { ...w } : { name: w.name, url: w.url }));
  }
  if (n?.email) {
    const e: EmailSettings = { server: n.email.server, from: n.email.from, to: [...n.email.to] };
    if (n.email.security && n.email.security !== 'starttls') e.security = n.email.security;
    if (n.email.username) e.username = n.email.username;
    if (n.email.password_file) e.password_file = n.email.password_file;
    out.email = e;
  }
  return out;
}

function vpnSummary(cur: FullVpn, next: FullVpn): string[] {
  const out: string[] = [];
  if (cur.enabled !== next.enabled) out.push(next.enabled ? 'Turn on the VPN' : 'Turn off the VPN');
  const before = new Map(cur.peers.map((p) => [p.name, canonical(p)]));
  const after = new Map(next.peers.map((p) => [p.name, canonical(p)]));
  for (const [name, json] of after) {
    if (!before.has(name)) out.push(`Add VPN device ${name}`);
    else if (before.get(name) !== json) out.push(`Change VPN device ${name}`);
  }
  for (const name of before.keys()) if (!after.has(name)) out.push(`Remove VPN device ${name}`);
  const general = (v: FullVpn) => canonical([v.port, v.subnet, v.endpoint ?? null]);
  if (general(cur) !== general(next)) out.push('Change VPN settings');
  return out;
}

function firewallSummary(cur: Required<FirewallSettings>, next: Required<FirewallSettings>): string[] {
  const out: string[] = [];
  if (cur.enabled !== next.enabled) out.push(next.enabled ? 'Turn on the firewall' : 'Turn off the firewall');
  const before = new Map(cur.rules.map((r) => [r.name, canonical(r)]));
  const after = new Map(next.rules.map((r) => [r.name, canonical(r)]));
  for (const [name, json] of after) {
    if (!before.has(name)) out.push(`Add firewall rule ${name}`);
    else if (before.get(name) !== json) out.push(`Change firewall rule ${name}`);
  }
  for (const name of before.keys()) if (!after.has(name)) out.push(`Remove firewall rule ${name}`);
  const order = (rules: { name: string }[], names: Map<string, string>) =>
    rules.filter((r) => names.has(r.name)).map((r) => r.name).join();
  if (out.length === 0 && order(cur.rules, after) !== order(next.rules, before)) out.push('Reorder firewall rules');
  if (canonical(cur.blocklists) !== canonical(next.blocklists) || canonical(cur.countries) !== canonical(next.countries)) {
    out.push('Change blocklists');
  }
  if (canonical(cur.allow) !== canonical(next.allow) || canonical(cur.block) !== canonical(next.block)) {
    out.push('Change allowed and blocked addresses');
  }
  const general = (f: Required<FirewallSettings>) =>
    canonical([f.allow_lan, f.allow_ping, f.allow_docker_ports, f.auto_ban]);
  if (general(cur) !== general(next)) out.push('Change firewall settings');
  return out;
}

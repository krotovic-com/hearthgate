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
} from './api';
import { canonical, compact, withDefaults } from './firewall';

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
function complete(state: DesiredState): Required<Pick<DesiredState, 'interfaces' | 'services' | 'routes'>> &
  DesiredState {
  return { ...state, interfaces: state.interfaces ?? [], services: state.services ?? [], routes: state.routes ?? [] };
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
  // Stored as the server stores them, so an unchanged draft compares equal.
  const fw = next.firewall ? compact(withDefaults(next.firewall)) : undefined;
  if (fw) next.firewall = fw;
  else delete next.firewall;
  if (!next.router?.upnp) delete next.router;
  for (const i of next.interfaces) if (!i.rate_limit) delete i.rate_limit;
  gw.draft = next;
}

/** Changes the draft's firewall settings. */
export function editFirewall(change: (fw: Required<FirewallSettings>) => void): void {
  edit((s) => {
    const fw = withDefaults(s.firewall);
    change(fw);
    s.firewall = fw;
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

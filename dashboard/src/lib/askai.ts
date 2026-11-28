// Pure helpers for the Ask AI panel.

import type { ChatRole, DesiredState, Plan, Proposal, ProposalKind } from './api';

/** The parts of a chat item that are sent back to the assistant. */
export interface TurnSource {
  role: ChatRole;
  text: string;
  proposals?: (Proposal & { state: string; revision?: number })[];
}

const stateWords: Record<string, string> = {
  pending: 'not decided yet',
  applied: 'applied',
  draft: 'put in the draft to edit in the form',
  skipped: 'skipped',
  undone: 'applied, then undone',
};

/** An item as text for the assistant. Its own earlier proposals are
 * summarised with what the user did with them, so it can pick up the
 * next step without re-proposing what is done. */
export function turnText(item: TurnSource): string {
  if (!item.proposals?.length) return item.text;
  const lines = item.proposals.map((p) => {
    const done = stateWords[p.state] ?? p.state;
    const rev = p.state === 'applied' && p.revision ? ` as revision ${p.revision}` : '';
    return `- Proposed (step ${p.step}): ${p.title} — ${done}${rev}`;
  });
  return `${item.text}\n\n${lines.join('\n')}`;
}

const sections: Record<ProposalKind, { path: string; list?: keyof DesiredState }> = {
  container: { path: '/containers', list: 'containers' },
  stack: { path: '/containers', list: 'stacks' },
  service: { path: '/services', list: 'services' },
  interface: { path: '/interfaces', list: 'interfaces' },
  route: { path: '/routes', list: 'routes' },
  firewall: { path: '/firewall' },
  vpn: { path: '/vpn' },
  settings: { path: '/settings' },
  other: { path: '/' },
};

/** The name of the item a proposal changes, from its first change: an
 * added item carries its name, an index points into `state`. */
export function proposalItem(p: Proposal, state: DesiredState): string | null {
  const list = sections[p.kind]?.list;
  if (!list) return null;
  for (const c of p.changes) {
    const parts = c.path.split('/').slice(1);
    if (parts[0] !== list) continue;
    const items = (state[list] ?? []) as { name?: string }[];
    if (parts.length === 1) continue;
    if (parts[1] === '-') {
      const v = c.value as { name?: unknown } | undefined;
      if (typeof v?.name === 'string') return v.name;
      // Added at the end: the state passed in already has it.
      return items[items.length - 1]?.name ?? null;
    }
    const index = Number(parts[1]);
    if (Number.isInteger(index)) {
      if (parts.length === 2 && c.op !== 'remove') {
        const v = c.value as { name?: unknown } | undefined;
        if (typeof v?.name === 'string') return v.name;
      }
      if (c.op === 'remove' && parts.length === 2) return null;
      return items[index]?.name ?? null;
    }
  }
  return null;
}

/** Where to edit a proposal in the normal form, once it is in the draft. */
export function proposalTarget(p: Proposal, state: DesiredState): string {
  const section = sections[p.kind] ?? sections.other;
  const name = proposalItem(p, state);
  if (!name) return section.path;
  const n = encodeURIComponent(name);
  switch (p.kind) {
    case 'route':
      return `/routes/${n}`;
    case 'stack':
      return `/containers?edit-stack=${n}`;
    default:
      return `${section.path}?edit=${n}`;
  }
}

/** Starter questions for the screen the user is on. */
export function starters(path: string, selection: string | null): string[] {
  const section = '/' + (path.split('/')[1] ?? '');
  switch (section) {
    case '/services':
      return selection
        ? [`Why is ${selection} unhealthy?`, `Publish ${selection} on a public interface`, `Restrict ${selection} to the VPN`]
        : ['Set up a new service from start to finish', 'Which services are down, and why?', 'Which services are reachable from the internet?'];
    case '/interfaces':
      return ['Add a public interface for a new domain', 'Why is a certificate not renewing?', 'Do I need HTTP on port 80?'];
    case '/routes':
      return ['Route a new subdomain to a service', 'Load balance two services', 'Which routes have no healthy target?'];
    case '/containers':
      return ['Run a new Docker container and publish it', 'Which containers have updates?', 'Why does a container keep restarting?'];
    case '/firewall':
      return ['Why is this traffic blocked?', 'Allow a port only from my LAN', 'Are my firewall rules too open?'];
    case '/vpn':
      return ['Add a VPN peer for my phone', 'Why can a peer not reach a service?', 'How does internal DNS work?'];
    case '/settings':
      return ['Explain these settings', 'Should I turn on UPnP?', 'How do automatic updates work?'];
    default:
      return ['Set up a new service from start to finish', 'Is anything wrong right now?', 'What is exposed to the internet?'];
  }
}

/** "2 of 5 done". */
export function planProgress(plan: Plan): { done: number; total: number; current: number | null } {
  const done = plan.steps.filter((s) => s.done).length;
  const next = plan.steps.findIndex((s) => !s.done);
  return { done, total: plan.steps.length, current: next < 0 ? null : next };
}

export const kindLabel: Record<ProposalKind, string> = {
  container: 'Container',
  stack: 'Stack',
  service: 'Service',
  interface: 'Public interface',
  route: 'Route',
  firewall: 'Firewall',
  vpn: 'VPN',
  settings: 'Settings',
  other: 'Change',
};

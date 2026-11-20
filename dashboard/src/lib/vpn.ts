// VPN and internal DNS settings: defaults as the server applies them, and
// the address arithmetic the add-device form needs.

import type { Access, DnsSettings, Peer, VpnSettings } from './api';
import { canonical } from './firewall';

export type FullVpn = Required<Omit<VpnSettings, 'endpoint'>> & { endpoint?: string };
export type FullDns = Required<DnsSettings>;

export const defaultVpn = (): FullVpn => ({ enabled: false, port: 51820, subnet: '10.88.0.0/24', peers: [] });
export const defaultDns = (): FullDns => ({ enabled: true, zone: 'home.internal', lan: false, upstream: [] });

export function vpnWithDefaults(v: VpnSettings | undefined): FullVpn {
  const out: FullVpn = { ...defaultVpn(), ...v, peers: (v?.peers ?? []).map((p) => ({ ...p, access: p.access ?? 'all' })) };
  if (!out.endpoint) delete out.endpoint;
  return out;
}

export function dnsWithDefaults(d: DnsSettings | undefined): FullDns {
  return { ...defaultDns(), ...d, upstream: d?.upstream ?? [] };
}

/** As the server stores it: sorted, and left out entirely when default. */
export function compactVpn(v: FullVpn): VpnSettings | undefined {
  const byName = (a: Peer, b: Peer) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);
  const out: VpnSettings = {
    ...v,
    peers: v.peers
      .map((p) => ({ ...p, access: Array.isArray(p.access) ? [...p.access].sort() : 'all' }) as Peer)
      .sort(byName),
  };
  if (out.endpoint) out.endpoint = out.endpoint.toLowerCase();
  else delete out.endpoint;
  if (canonical(out) === canonical(defaultVpn())) return undefined;
  if (!out.peers?.length) delete out.peers;
  return out;
}

export function compactDns(d: FullDns): DnsSettings | undefined {
  const out: DnsSettings = { ...d, zone: d.zone.toLowerCase() };
  if (canonical(out) === canonical(defaultDns())) return undefined;
  if (!out.upstream?.length) delete out.upstream;
  return out;
}

function toInt(ip: string): number | null {
  const parts = ip.split('.');
  if (parts.length !== 4) return null;
  let n = 0;
  for (const p of parts) {
    if (!/^\d{1,3}$/.test(p) || Number(p) > 255) return null;
    n = n * 256 + Number(p);
  }
  return n;
}

const toIp = (n: number) => [24, 16, 8, 0].map((s) => (n >>> s) & 255).join('.');

/** `[network, prefix]` of an IPv4 network like 10.88.0.0/24. */
export function parseSubnet(text: string): [number, number] | null {
  const [ip, len] = text.trim().split('/');
  const base = toInt(ip ?? '');
  const prefix = Number(len);
  if (base === null || !/^\d{1,2}$/.test(len ?? '') || prefix > 32) return null;
  const mask = prefix === 0 ? 0 : (0xffffffff << (32 - prefix)) >>> 0;
  return [(base & mask) >>> 0, prefix];
}

/** This machine's address in the VPN: the first host. */
export function serverAddress(subnet: string): string | null {
  const net = parseSubnet(subnet);
  return net ? toIp(net[0] + 1) : null;
}

/** The next address no peer uses, as the server picks it. */
export function freeAddress(vpn: FullVpn): string | null {
  const net = parseSubnet(vpn.subnet);
  if (!net) return null;
  const size = 2 ** (32 - net[1]);
  const used = new Set(vpn.peers.map((p) => p.address));
  for (let i = 2; i < size - 1; i++) {
    const ip = toIp(net[0] + i);
    if (!used.has(ip)) return ip;
  }
  return null;
}

/** Why a VPN subnet can't be used, or null. */
export function subnetProblem(text: string): string | null {
  const net = parseSubnet(text);
  if (!net || !text.includes('/')) return 'Use a network like 10.88.0.0/24.';
  if (net[1] < 16 || net[1] > 30) return 'Use a network from /16 to /30.';
  const [a, b] = [net[0] >>> 24, (net[0] >>> 16) & 255];
  const isPrivate = a === 10 || (a === 172 && b >= 16 && b < 32) || (a === 192 && b === 168);
  return isPrivate ? null : 'Use a private network (10/8, 172.16/12 or 192.168/16).';
}

/** A device name turned into a peer name: "Kitchen tablet" → "kitchen-tablet". */
export function peerName(label: string): string {
  return label
    .normalize('NFKD')
    .replace(/[̀-ͯ]/g, '')
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 63);
}

export function accessLabel(access: Access | undefined): string {
  if (!access || access === 'all') return 'Everything';
  if (access.length === 0) return 'Nothing';
  if (access.length <= 2) return `Only ${access.join(' and ')}`;
  return `${access.length} services`;
}

/** "1.2 GB", "80 MB", "512 B". */
export function bytes(n: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let i = 0;
  let v = n;
  while (v >= 1000 && i < units.length - 1) {
    v /= 1000;
    i++;
  }
  return `${i === 0 || v >= 10 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}

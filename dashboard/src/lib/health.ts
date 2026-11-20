// Turns raw status into what the pages show: a health badge per service and
// interface, and the "needs attention" list.

import type {
  CertificateInfo,
  DesiredState,
  Interface,
  Service,
  StatusResponse,
  UpstreamStatus,
} from './api';
import type { BadgeStatus } from './ui/Badge.svelte';

export interface Health {
  status: BadgeStatus;
  label: string;
}

export function serviceUpstreams(status: StatusResponse | null, name: string): UpstreamStatus[] {
  return status?.upstreams.filter((u) => u.service === name) ?? [];
}

export function upstreamDown(u: UpstreamStatus): boolean {
  return !u.healthy || u.ejected || u.endpoint === 'unresolved';
}

export function serviceHealth(status: StatusResponse | null, service: Service): Health {
  const ups = serviceUpstreams(status, service.name);
  if (!status) return { status: 'neutral', label: 'Checking' };
  if (ups.length === 0) return { status: 'neutral', label: 'Not routed' };
  const down = ups.filter(upstreamDown);
  if (down.length === ups.length) {
    const unresolved = down.every((u) => u.endpoint === 'unresolved');
    return { status: 'critical', label: unresolved ? 'Not found' : 'Down' };
  }
  if (down.length > 0) return { status: 'warning', label: `${down.length} of ${ups.length} down` };
  return { status: 'healthy', label: 'Running' };
}

/** Listening sockets look like `[::]:443/tcp` or `0.0.0.0:53/udp`. */
function socketPort(socket: string): { port: number; proto: string } | null {
  const m = /:(\d+)\/(tcp|udp)$/.exec(socket);
  return m ? { port: Number(m[1]), proto: m[2]! } : null;
}

export function interfaceHealth(
  status: StatusResponse | null,
  iface: Interface,
  certs: CertificateInfo[],
): Health {
  if (!status) return { status: 'neutral', label: 'Checking' };
  const open = status.listeners.map((l) => socketPort(l.socket)).filter((x) => x !== null);
  const missing = iface.listeners.filter((l) => !open.some((o) => o.port === l.port));
  if (missing.length === iface.listeners.length && iface.listeners.length > 0) {
    return { status: 'critical', label: 'Not listening' };
  }
  if (missing.length > 0) return { status: 'warning', label: `${missing.length} port not open` };
  if (iface.listeners.some((l) => l.mode === 'https')) {
    const cert = certificateFor(iface, certs);
    if (!cert) return { status: 'warning', label: 'No certificate yet' };
    if (cert.days_left < 14) return { status: 'warning', label: `Certificate: ${cert.days_left} days` };
  }
  return { status: 'healthy', label: 'Running' };
}

/** The certificate an interface uses: a named manual one or one covering its first domain. */
export function certificateFor(iface: Interface, certs: CertificateInfo[]): CertificateInfo | undefined {
  if (iface.certificate) return certs.find((c) => c.name === iface.certificate);
  const domains = iface.domains ?? [];
  return certs.find((c) =>
    domains.every((d) =>
      c.domains.some((cd) => cd === d || (cd.startsWith('*.') && d.endsWith(cd.slice(1)))),
    ),
  );
}

export interface Attention {
  key: string;
  title: string;
  detail: string;
  status: BadgeStatus;
  badge: string;
  href?: string;
  action?: string;
}

export function attentionItems(
  status: StatusResponse | null,
  state: DesiredState,
  certs: CertificateInfo[],
): Attention[] {
  const items: Attention[] = [];
  if (!status) return items;
  for (const service of state.services ?? []) {
    const ups = serviceUpstreams(status, service.name);
    const down = ups.filter(upstreamDown);
    if (down.length === 0) continue;
    const unresolved = down.some((u) => u.endpoint === 'unresolved');
    items.push({
      key: `svc-${service.name}`,
      title: `${service.name} is ${down.length === ups.length ? 'down' : 'partly down'}`,
      detail: unresolved
        ? service.kind === 'docker'
          ? `Container ${service.container ?? service.name} was not found or is not running.`
          : `${service.host ?? 'The host'} could not be resolved.`
        : `${down.map((u) => `${u.endpoint} (port ${u.port})`).join(', ')} failed ${down.some((u) => u.ejected) ? 'repeatedly and is taken out of rotation' : 'its health check'}.`,
      status: down.length === ups.length ? 'critical' : 'warning',
      badge: down.length === ups.length ? 'Down' : 'Degraded',
      href: `/services/${encodeURIComponent(service.name)}`,
      action: 'View service',
    });
  }
  for (const cert of certs) {
    if (cert.days_left >= 14) continue;
    items.push({
      key: `cert-${cert.name}`,
      title: `${cert.domains[0] ?? cert.name} certificate`,
      detail:
        cert.source === 'automatic'
          ? `Expires in ${cert.days_left} days and has not been renewed yet.`
          : `This uploaded certificate expires in ${cert.days_left} days. Upload a new one.`,
      status: cert.days_left < 3 ? 'critical' : 'warning',
      badge: `${cert.days_left} days`,
      href: '/interfaces',
      action: 'View interfaces',
    });
  }
  for (const problem of status.problems) {
    items.push({
      key: `p-${problem}`,
      title: problem.split(':')[0] ?? problem,
      detail: problem,
      status: 'warning',
      badge: 'Problem',
    });
  }
  return items;
}

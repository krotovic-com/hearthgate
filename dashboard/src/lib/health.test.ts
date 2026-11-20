import { describe, expect, it } from 'vitest';
import type { Service, StatusResponse, UpstreamStatus } from './api';
import { attentionItems, certificateFor, interfaceHealth, serviceHealth } from './health';

const up = (service: string, extra: Partial<UpstreamStatus> = {}): UpstreamStatus => ({
  service,
  port: 80,
  endpoint: '10.0.0.2:80',
  healthy: true,
  ejected: false,
  active: 0,
  rate: { requests: 0, failures: 0 },
  ...extra,
});

const status = (upstreams: UpstreamStatus[], sockets: string[] = []): StatusResponse => ({
  version: 'test',
  revision: 1,
  latest_revision: 1,
  listeners: sockets.map((socket) => ({ socket, kind: 'http', open_connections: 0 })),
  upstreams,
  problems: [],
  host: { hostname: 'h', os: null, uptime: 1 },
});

const web: Service = { name: 'web', kind: 'external', host: '10.0.0.2', ports: { http: 80 } };

describe('health', () => {
  it('reports services', () => {
    expect(serviceHealth(null, web).label).toBe('Checking');
    expect(serviceHealth(status([]), web).label).toBe('Not routed');
    expect(serviceHealth(status([up('web')]), web).status).toBe('healthy');
    expect(serviceHealth(status([up('web', { healthy: false })]), web).label).toBe('Down');
    expect(serviceHealth(status([up('web', { endpoint: 'unresolved' })]), web).label).toBe('Not found');
    expect(serviceHealth(status([up('web'), up('web', { port: 81, ejected: true })]), web).status).toBe('warning');
  });

  it('reports interfaces and their certificates', () => {
    const iface = { name: 'site', domains: ['a.example.com'], listeners: [{ port: 443, mode: 'https' as const }] };
    expect(interfaceHealth(status([]), iface, []).label).toBe('Not listening');
    expect(interfaceHealth(status([], ['[::]:443/tcp']), iface, []).label).toBe('No certificate yet');
    const cert = { name: 'x', source: 'automatic' as const, domains: ['*.example.com'], not_after: 0, days_left: 60, issuer: 'LE' };
    expect(certificateFor(iface, [cert])).toBe(cert);
    expect(interfaceHealth(status([], ['[::]:443/tcp']), iface, [cert]).label).toBe('Running');
  });

  it('lists what needs attention', () => {
    const items = attentionItems(status([up('web', { healthy: false })]), { services: [web] }, [
      { name: 'c', source: 'automatic', domains: ['a.example.com'], not_after: 0, days_left: 5, issuer: 'LE' },
    ]);
    expect(items.map((i) => i.title)).toEqual(['web is down', 'a.example.com certificate']);
  });
});

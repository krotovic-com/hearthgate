import { describe, expect, it } from 'vitest';
import { changeLines, latestFor, scopeLabel, severityCounts } from './ai';
import type { AiReview } from './api';

describe('ai helpers', () => {
  it('counts findings by severity', () => {
    const f = (severity: 'high' | 'medium' | 'low') => ({ severity, area: '', title: '', detail: '' });
    expect(severityCounts([f('high'), f('low'), f('high')])).toBe('2 high · 1 low');
    expect(severityCounts([])).toBe('all clear');
  });

  it('labels scopes', () => {
    expect(scopeLabel({ kind: 'service', service: 'photos' })).toBe('Service photos');
  });

  it('finds the newest review for a scope', () => {
    const r = (id: string, kind: 'all' | 'firewall') => ({ id, scope: { kind } }) as AiReview;
    expect(latestFor([r('2', 'firewall'), r('1', 'all')], { kind: 'all' })?.id).toBe('1');
    expect(latestFor([r('1', 'all')], { kind: 'firewall' })).toBeNull();
  });

  it('shows changes as lines', () => {
    expect(
      changeLines([
        { op: 'replace', path: '/services/0/ports/http', value: 8081 },
        { op: 'remove', path: '/firewall/rules/2' },
        { op: 'add', path: '/firewall/rules/-', value: { name: 'x' } },
      ]),
    ).toEqual([
      { sign: '~', text: 'services › 0 › ports › http → 8081' },
      { sign: '-', text: 'remove firewall › rules › 2' },
      { sign: '+', text: 'firewall › rules › -: {"name":"x"}' },
    ]);
  });
});

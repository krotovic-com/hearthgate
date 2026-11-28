import { describe, expect, it } from 'vitest';
import { planProgress, proposalItem, proposalTarget, starters, turnText } from './askai';
import type { DesiredState, Proposal } from './api';

const proposal = (p: Partial<Proposal>): Proposal => ({
  kind: 'service',
  title: 'Add photos',
  summary: '',
  step: 1,
  changes: [],
  ...p,
});

const state: DesiredState = {
  services: [
    { name: 'nas', kind: 'external', host: '192.168.1.5', ports: { http: 80 } },
    { name: 'photos', kind: 'docker', ports: { http: 2342 } },
  ],
  routes: [{ name: 'photos', interface: 'web', targets: [{ service: 'photos', port: 'http' }] }],
} as unknown as DesiredState;

describe('turnText', () => {
  it('keeps plain turns as they are', () => {
    expect(turnText({ role: 'user', text: 'hi' })).toBe('hi');
  });
  it('adds what the user did with each proposal', () => {
    const text = turnText({
      role: 'assistant',
      text: 'First the service.',
      proposals: [
        { ...proposal({}), state: 'applied', revision: 7 },
        { ...proposal({ title: 'Add route', step: 2 }), state: 'skipped' },
      ],
    });
    expect(text).toContain('Add photos — applied as revision 7');
    expect(text).toContain('(step 2): Add route — skipped');
  });
});

describe('proposalTarget', () => {
  it('names an added item from its value', () => {
    const p = proposal({ changes: [{ op: 'add', path: '/services/-', value: { name: 'photos' } }] });
    expect(proposalItem(p, state)).toBe('photos');
    expect(proposalTarget(p, state)).toBe('/services?edit=photos');
  });
  it('names a changed item by its index', () => {
    const p = proposal({ changes: [{ op: 'replace', path: '/services/0/host', value: '10.0.0.2' }] });
    expect(proposalTarget(p, state)).toBe('/services?edit=nas');
  });
  it('opens routes by name and stacks by their own parameter', () => {
    expect(proposalTarget(proposal({ kind: 'route', changes: [{ op: 'replace', path: '/routes/0/strategy', value: 'least-connections' }] }), state)).toBe('/routes/photos');
    expect(proposalTarget(proposal({ kind: 'stack', changes: [{ op: 'add', path: '/stacks/-', value: { name: 'media stack' } }] }), state)).toBe('/containers?edit-stack=media%20stack');
  });
  it('falls back to the screen for settings and removals', () => {
    expect(proposalTarget(proposal({ kind: 'firewall', changes: [{ op: 'add', path: '/firewall/rules/-', value: {} }] }), state)).toBe('/firewall');
    expect(proposalTarget(proposal({ changes: [{ op: 'remove', path: '/services/1' }] }), state)).toBe('/services');
  });
});

describe('starters', () => {
  it('fits the screen and the selection', () => {
    expect(starters('/services/photos', 'photos')[0]).toContain('photos');
    expect(starters('/firewall', null)).toContain('Why is this traffic blocked?');
    expect(starters('/nowhere', null).length).toBe(3);
  });
});

describe('planProgress', () => {
  it('counts done steps and finds the current one', () => {
    const plan = { title: 'Set up', steps: [{ title: 'a', detail: '', done: true }, { title: 'b', detail: '', done: false }, { title: 'c', detail: '', done: false }] };
    expect(planProgress(plan)).toEqual({ done: 1, total: 3, current: 1 });
    expect(planProgress({ title: 'x', steps: [{ title: 'a', detail: '', done: true }] }).current).toBeNull();
  });
});

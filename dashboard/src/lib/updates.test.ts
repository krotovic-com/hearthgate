import { describe, expect, it } from 'vitest';
import type { AvailableUpdate, UpdateInfo, UpdateJob } from './api';
import { bumpSentence, jobRunning, noteCounts, stepPosition, waitsReason, when, windowOptions } from './updates';

const available = (bump: AvailableUpdate['bump'], security = 0): AvailableUpdate => ({
  version: '1.3.0',
  published_at: '2026-11-26T10:00:00Z',
  bump,
  notes: [
    { kind: 'feature', text: 'a' },
    { kind: 'fix', text: 'b' },
    { kind: 'fix', text: 'c' },
    ...Array.from({ length: security }, () => ({ kind: 'security' as const, text: 's' })),
  ],
  page: 'https://github.com/krotovic-com/hearthgate/releases/tag/v1.3.0',
  asset: 'hearthgate_1.3.0_amd64.deb',
});

const info = (auto: 'off' | 'patch' | 'minor', extra: Partial<UpdateInfo> = {}): UpdateInfo => ({
  current: '1.2.1',
  method: 'deb',
  method_text: 'Debian package · apt',
  can_install: true,
  cannot_install: null,
  settings: { check: true, auto_install: auto, window: '03:00-05:00' },
  checked_unix: null,
  error: null,
  available: null,
  skipped: [],
  scheduled: null,
  job: null,
  rollback_to: null,
  ...extra,
});

describe('update helpers', () => {
  it('counts changes', () => {
    expect(noteCounts(available('minor', 1).notes)).toBe('1 feature, 2 fixes, 1 security fix');
    expect(noteCounts([])).toBe('see the release notes');
  });

  it('explains the kind of release', () => {
    expect(bumpSentence('1.2.1', available('minor'))).toContain('configuration stays as it is');
    expect(bumpSentence('1.2.1', available('major'))).toContain('breaking changes');
  });

  it('says why a release waits', () => {
    expect(waitsReason(info('patch'), available('minor', 1))).toBe(
      'Includes 1 security fix. Automatic install is set to patch releases only, so this one waits for you.',
    );
    expect(waitsReason(info('off'), available('patch'))).toBe('Automatic installs are off, so it waits for you.');
    expect(waitsReason(info('minor'), available('major'))).toBe('Major releases always wait for you.');
    expect(waitsReason(info('minor'), available('minor'))).toContain('by itself');
    expect(waitsReason(info('off', { scheduled: '1.3.0' }), available('minor'))).toBe('It installs at 03:00.');
  });

  it('keeps a custom window', () => {
    expect(windowOptions('03:00-05:00')).toHaveLength(6);
    expect(windowOptions('22:15-23:00')[0]).toEqual({ value: '22:15-23:00', label: 'Every day · 22:15–23:00' });
  });

  it('follows a job', () => {
    const job: UpdateJob = {
      action: 'install',
      from: '1.2.1',
      to: '1.3.0',
      steps: [
        { title: 'a', detail: '', state: 'done' },
        { title: 'b', detail: '', state: 'running' },
        { title: 'c', detail: '', state: 'pending' },
      ],
      outcome: 'running',
      detail: null,
      started_unix: 1000,
      finished_unix: null,
      log: [],
    };
    expect(stepPosition(job)).toBe('Step 2 of 3');
    expect(jobRunning(job, 1100)).toBe(true);
    expect(jobRunning(job, 1000 + 3600)).toBe(false);
    expect(jobRunning({ ...job, outcome: 'installed' }, 1100)).toBe(false);
  });

  it('names days', () => {
    const now = new Date(2026, 10, 28, 12, 0);
    expect(when(new Date(2026, 10, 28, 9, 12).getTime() / 1000, now)).toBe('Today at 09:12');
    expect(when(new Date(2026, 10, 27, 3, 4).getTime() / 1000, now)).toBe('Yesterday at 03:04');
    expect(when(new Date(2026, 10, 21, 3, 4).getTime() / 1000, now)).toBe('21 Nov at 03:04');
  });
});

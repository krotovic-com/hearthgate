import { describe, expect, it } from 'vitest';
import { ago, duration, plural, portLabel, rate } from './format';

describe('format', () => {
  it('pluralizes', () => {
    expect(plural(1, 'route')).toBe('1 route');
    expect(plural(2, 'route')).toBe('2 routes');
  });
  it('formats durations', () => {
    expect(duration(30)).toBe('30 seconds');
    expect(duration(3 * 86400 + 5)).toBe('3 days');
    expect(duration(3600)).toBe('1 hour');
  });
  it('formats times relative to now', () => {
    const now = new Date('2026-11-20T12:00:00');
    expect(ago(new Date('2026-11-20T11:59:50'), now)).toBe('just now');
    expect(ago(new Date('2026-11-20T11:30:00'), now)).toBe('30 min ago');
  });
  it('labels ports and rates', () => {
    expect(portLabel(443, undefined)).toBe('443/tcp');
    expect(portLabel(27015, 'both')).toBe('27015/tcp+udp');
    expect(rate(0)).toBe('0/s');
    expect(rate(2.25)).toBe('2.3/s');
    expect(rate(42.4)).toBe('42/s');
  });
});

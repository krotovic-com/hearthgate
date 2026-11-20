import { describe, expect, it } from 'vitest';
import { canonical, compact, defaultFirewall, parseNet, parsePort, ruleRows, showNet, withDefaults } from './firewall';

describe('firewall helpers', () => {
  it('parses addresses and networks', () => {
    expect(parseNet('203.0.113.5')).toBe('203.0.113.5/32');
    expect(parseNet(' 203.0.113.0/24 ')).toBe('203.0.113.0/24');
    expect(parseNet('2001:DB8::/32')).toBe('2001:db8::/32');
    expect(parseNet('2001:db8::1')).toBe('2001:db8::1/128');
    expect(parseNet('256.1.1.1')).toBeNull();
    expect(parseNet('10.0.0.0/33')).toBeNull();
    expect(parseNet('1::2::3')).toBeNull();
    expect(parseNet('example.com')).toBeNull();
    expect(showNet('203.0.113.5/32')).toBe('203.0.113.5');
    expect(showNet('2001:db8::1/128')).toBe('2001:db8::1');
    expect(showNet('2001:db8::/32')).toBe('2001:db8::/32');
  });

  it('parses ports and ranges', () => {
    expect(parsePort('22')).toBe(22);
    expect(parsePort('8000-8100')).toBe('8000-8100');
    expect(parsePort('80-80')).toBe(80);
    expect(parsePort('0')).toBeNull();
    expect(parsePort('90-80')).toBeNull();
    expect(parsePort('70000')).toBeNull();
  });

  it('stores defaults the way the server does', () => {
    expect(compact(withDefaults(undefined))).toBeUndefined();
    const fw = withDefaults({ ...defaultFirewall(), enabled: true });
    fw.rules.push({ name: 'a', action: 'deny', sources: [], ports: [], comment: '' });
    expect(compact(fw)).toEqual({ ...defaultFirewall(), enabled: true, rules: [{ name: 'a', action: 'deny', protocol: 'tcp' }] });
    expect(canonical({ b: 1, a: [{ d: 1, c: undefined }] })).toBe('{"a":[{"d":1}],"b":1}');
  });

  it('lists the checks in order', () => {
    const fw = withDefaults({
      ...defaultFirewall(),
      enabled: true,
      rules: [{ name: 'ssh', action: 'allow', sources: ['192.0.2.0/24'], ports: [22] }],
      blocklists: [{ name: 'firehol', url: 'https://example.com/l' }],
    });
    const rows = ruleRows(fw, [
      { name: 'web', listeners: [{ port: 443, mode: 'https' }], rate_limit: 20 },
    ]);
    expect(rows.map((r) => r.key)).toEqual(['lists', 'bans', 'lan', 'rule-ssh', 'limit-web', 'open']);
    expect(rows[3]!.detail).toBe('From 192.0.2.0/24 to 22/tcp');
    expect(rows.at(-1)!.detail).toBe('Open 443/tcp');
  });
});

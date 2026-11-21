import { describe, expect, it } from 'vitest';
import { accessLabel, bytes, compactDns, compactVpn, defaultDns, freeAddress, peerName, serverAddress, subnetProblem, vpnWithDefaults } from './vpn';

describe('vpn helpers', () => {
  it('compacts to what the server stores', () => {
    expect(compactVpn(vpnWithDefaults(undefined))).toBeUndefined();
    expect(compactDns(defaultDns())).toBeUndefined();
    const v = vpnWithDefaults({ enabled: true, endpoint: 'VPN.example.com', peers: [
      { name: 'z', public_key: 'k1', address: '10.88.0.3', access: ['b', 'a'] },
      { name: 'a', public_key: 'k2', address: '10.88.0.2' },
    ] });
    expect(compactVpn(v)).toEqual({ enabled: true, port: 51820, subnet: '10.88.0.0/24', endpoint: 'vpn.example.com', peers: [
      { name: 'a', public_key: 'k2', address: '10.88.0.2', access: 'all' },
      { name: 'z', public_key: 'k1', address: '10.88.0.3', access: ['a', 'b'] },
    ] });
  });

  it('picks addresses like the server', () => {
    const v = vpnWithDefaults({ peers: [{ name: 'a', public_key: 'k', address: '10.88.0.2' }] });
    expect(serverAddress(v.subnet)).toBe('10.88.0.1');
    expect(freeAddress(v)).toBe('10.88.0.3');
    expect(freeAddress(vpnWithDefaults({ subnet: '10.9.0.0/30', peers: [{ name: 'a', public_key: 'k', address: '10.9.0.2' }] }))).toBeNull();
  });

  it('checks subnets and names', () => {
    expect(subnetProblem('10.88.0.0/24')).toBeNull();
    expect(subnetProblem('8.8.8.0/24')).toMatch(/private/);
    expect(subnetProblem('10.0.0.0/8')).toMatch(/\/16/);
    expect(subnetProblem('nope')).toMatch(/like/);
    expect(peerName('Lukáš’s phone')).toBe('lukas-s-phone');
    expect(accessLabel(['ssh', 'vault'])).toBe('Only ssh and vault');
    expect(bytes(1_234_000_000)).toBe('1.2 GB');
    expect(bytes(80_000_000)).toBe('80 MB');
  });
});

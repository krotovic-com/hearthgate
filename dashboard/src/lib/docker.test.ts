import { describe, expect, it } from 'vitest';
import { deviceProblem, groupProblem, parseEnv, publishProblem, shortImage, showCommand, splitCommand, volumeProblem } from './docker';

describe('docker form helpers', () => {
  it('parses env lines', () => {
    expect(parseEnv('TZ=Europe/Prague\n# comment\n\nA=b=c')).toEqual({ env: { TZ: 'Europe/Prague', A: 'b=c' }, problem: null });
    expect(parseEnv('1A=x').problem).toMatch(/KEY=value/);
  });
  it('checks volumes and ports like the server', () => {
    expect(volumeProblem('/srv/a:/data:ro')).toBeNull();
    expect(volumeProblem('db:/var/lib/db')).toBeNull();
    expect(volumeProblem('./a:/data')).not.toBeNull();
    expect(publishProblem('127.0.0.1:8080:80')).toBeNull();
    expect(publishProblem('53:53/udp')).toBeNull();
    expect(publishProblem('80')).not.toBeNull();
    expect(publishProblem('0:80')).not.toBeNull();
  });
  it('checks devices and groups like the server', () => {
    expect(deviceProblem('/dev/dri')).toBeNull();
    expect(deviceProblem('/dev/ttyUSB0:/dev/zigbee:rw')).toBeNull();
    expect(deviceProblem('/etc/shadow')).not.toBeNull();
    expect(deviceProblem('/dev/../etc')).not.toBeNull();
    expect(deviceProblem('/dev/dri:/dev/dri:rx')).not.toBeNull();
    expect(deviceProblem('/dev/dri:/dev/dri:rr')).not.toBeNull();
    expect(groupProblem('render')).toBeNull();
    expect(groupProblem('993')).toBeNull();
    expect(groupProblem('Render')).not.toBeNull();
    expect(groupProblem('9x')).not.toBeNull();
  });
  it('splits and shows commands', () => {
    expect(splitCommand(`sh -c "echo 'hi there'"`)).toEqual(['sh', '-c', "echo 'hi there'"]);
    expect(showCommand(['sh', '-c', 'echo hi'])).toBe('sh -c "echo hi"');
    expect(splitCommand(showCommand(['a b', 'c"d']))).toEqual(['a b', 'c"d']);
    expect(shortImage('docker.io/library/nginx:1.27')).toBe('nginx:1.27');
  });
});

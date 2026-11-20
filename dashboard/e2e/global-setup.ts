// Builds Hearthgate (with the freshly built dashboard), starts it with an
// empty data directory, and starts a small upstream web server to publish.
import { execFileSync, spawn } from 'node:child_process';
import { mkdtempSync, writeFileSync, existsSync } from 'node:fs';
import { createServer, Socket } from 'node:net';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { STATE_FILE, type Server } from './server';

const root = resolve(import.meta.dirname, '..', '..');

function freePort(): Promise<number> {
  return new Promise((ok, fail) => {
    const s = createServer();
    s.once('error', fail);
    s.listen(0, '127.0.0.1', () => {
      const port = (s.address() as { port: number }).port;
      s.close(() => ok(port));
    });
  });
}

async function waitFor(port: number): Promise<void> {
  for (let i = 0; i < 100; i++) {
    const open = await new Promise<boolean>((ok) => {
      const sock = new Socket();
      sock.once('connect', () => (sock.destroy(), ok(true)));
      sock.once('error', () => ok(false));
      sock.connect(port, '127.0.0.1');
    });
    if (open) return;
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`nothing listens on port ${port}`);
}

export default async function globalSetup(): Promise<void> {
  if (!existsSync(join(root, 'dashboard', 'dist', 'index.html'))) {
    throw new Error('build the dashboard first: npm run build');
  }
  const bin = process.env.HEARTHGATE_BIN ?? join(root, 'target', 'debug', 'hearthgate');
  if (!process.env.HEARTHGATE_BIN) {
    execFileSync('cargo', ['build', '-p', 'hearthgate'], { cwd: root, stdio: 'inherit' });
  }
  const dir = mkdtempSync(join(tmpdir(), 'hearthgate-e2e-'));
  const [adminPort, publicPort, upstreamPort] = [await freePort(), await freePort(), await freePort()];
  const config = join(dir, 'hearthgate.toml');
  writeFileSync(
    config,
    `data_dir = "${join(dir, 'data')}"\nlog = "info"\n[admin]\nlisten = ["127.0.0.1:${adminPort}"]\n[docker]\nenabled = false\n`,
  );
  const daemon = spawn(bin, ['-c', config, 'run'], { stdio: ['ignore', 'ignore', 'inherit'], detached: false });
  const upstream = spawn(
    process.execPath,
    [
      '-e',
      `require('http').createServer((q, r) => r.end('hello from upstream ' + q.url)).listen(${upstreamPort}, '127.0.0.1')`,
    ],
    { stdio: 'ignore' },
  );
  await waitFor(adminPort);
  await waitFor(upstreamPort);
  const state: Server = {
    baseURL: `https://127.0.0.1:${adminPort}`,
    bin,
    config,
    publicPort,
    upstreamPort,
    pids: [daemon.pid!, upstream.pid!],
  };
  writeFileSync(STATE_FILE, JSON.stringify(state));
}

// Shared between the global setup and the tests: where the Hearthgate
// under test runs.
import { readFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';

export interface Server {
  baseURL: string;
  bin: string;
  config: string;
  publicPort: number;
  upstreamPort: number;
  pids: number[];
}

export const STATE_FILE = join(tmpdir(), 'hearthgate-e2e.json');

export function server(): Server {
  return JSON.parse(readFileSync(STATE_FILE, 'utf8')) as Server;
}

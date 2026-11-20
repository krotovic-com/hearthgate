// Live data from the gateway: status every two seconds and new log lines,
// over one Server-Sent Events connection. Falls back to polling when the
// stream is unavailable.

import { api, type LogLine, type StatusResponse } from './api';
import { loadSession } from './session.svelte';

const MAX_LINES = 500;

export const live = $state<{
  status: StatusResponse | null;
  logs: LogLine[];
  connected: boolean;
}>({ status: null, logs: [], connected: false });

let source: EventSource | null = null;
let poll: ReturnType<typeof setInterval> | null = null;

function addLines(lines: LogLine[]): void {
  const last = live.logs.at(-1)?.id ?? 0;
  const fresh = lines.filter((l) => l.id > last);
  if (fresh.length === 0) return;
  const all = live.logs.concat(fresh);
  live.logs = all.length > MAX_LINES ? all.slice(all.length - MAX_LINES) : all;
}

async function refreshLogs(): Promise<void> {
  try {
    addLines(await api.logs(MAX_LINES));
  } catch {
    // Shown as disconnected; the next event retries.
  }
}

async function refreshStatus(): Promise<void> {
  try {
    live.status = await api.status();
  } catch {
    // A 401 logs out through the API client.
  }
}

/** Starts the stream. Returns a function that stops it. */
export function connectLive(): () => void {
  refreshStatus();
  refreshLogs();
  if (typeof EventSource === 'undefined') {
    poll = setInterval(refreshStatus, 5000);
  } else {
    source = new EventSource('/api/v1/events');
    source.addEventListener('open', () => {
      live.connected = true;
    });
    source.addEventListener('status', (e) => {
      live.status = JSON.parse((e as MessageEvent).data) as StatusResponse;
    });
    source.addEventListener('log', (e) => {
      addLines([JSON.parse((e as MessageEvent).data) as LogLine]);
    });
    source.addEventListener('log-reset', () => {
      refreshLogs();
    });
    source.addEventListener('error', () => {
      live.connected = false;
      // The browser reconnects by itself; check that we are still logged in.
      loadSession();
    });
  }
  return () => {
    source?.close();
    source = null;
    if (poll) clearInterval(poll);
    poll = null;
    live.connected = false;
  };
}

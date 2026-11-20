// Formatting helpers shared by the pages.

export function plural(n: number, one: string, many = `${one}s`): string {
  return `${n} ${n === 1 ? one : many}`;
}

/** "3 days", "5 hours", "12 minutes". */
export function duration(seconds: number): string {
  if (seconds >= 86400) return plural(Math.floor(seconds / 86400), 'day');
  if (seconds >= 3600) return plural(Math.floor(seconds / 3600), 'hour');
  if (seconds >= 60) return plural(Math.floor(seconds / 60), 'minute');
  return plural(Math.max(0, Math.floor(seconds)), 'second');
}

/** "4 min ago", "yesterday 13:20", a date for older times. */
export function ago(date: Date, now = new Date()): string {
  const s = (now.getTime() - date.getTime()) / 1000;
  if (s < 45) return 'just now';
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400 && date.getDate() === now.getDate()) return `today ${clock(date)}`;
  if (s < 2 * 86400) return `yesterday ${clock(date)}`;
  return date.toLocaleDateString(undefined, { day: 'numeric', month: 'short', year: 'numeric' });
}

/** "13:21". */
export function clock(date: Date): string {
  return date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false });
}

/** "12.5/s", "0.3/s", "–". */
export function rate(perSecond: number): string {
  if (perSecond <= 0) return '0/s';
  if (perSecond < 10) return `${perSecond.toFixed(1)}/s`;
  return `${Math.round(perSecond)}/s`;
}

export function number(n: number): string {
  return n.toLocaleString('en-US');
}

/** "443/tcp", "51820/udp", "27015/tcp+udp". */
export function portLabel(port: number, protocol: string | undefined): string {
  const p = protocol ?? 'tcp';
  return `${port}/${p === 'both' ? 'tcp+udp' : p}`;
}

export const modeLabels: Record<string, string> = {
  https: 'HTTPS',
  http: 'HTTP',
  'tls-passthrough': 'TLS passthrough',
  raw: 'Raw TCP/UDP',
};

export const strategyLabels: Record<string, string> = {
  'round-robin': 'Round robin',
  'least-connections': 'Least connections',
  'source-hash': 'Same client, same target',
};

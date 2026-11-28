// Parsing and checking the container and stack forms, the same way the
// server validates them.

/** "KEY=value" lines into a map, with the first bad line. */
export function parseEnv(text: string): { env: Record<string, string>; problem: string | null } {
  const env: Record<string, string> = {};
  for (const raw of text.split('\n')) {
    const line = raw.trim();
    if (!line || line.startsWith('#')) continue;
    const at = line.indexOf('=');
    const key = at < 0 ? line : line.slice(0, at).trim();
    if (at < 0 || !/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
      return { env, problem: `"${line}" is not KEY=value.` };
    }
    env[key] = line.slice(at + 1);
  }
  return { env, problem: null };
}

export const showEnv = (env: Record<string, string> | undefined): string =>
  Object.entries(env ?? {})
    .map(([k, v]) => `${k}=${v}`)
    .join('\n');

export const lines = (text: string): string[] =>
  text
    .split('\n')
    .map((l) => l.trim())
    .filter(Boolean);

export function volumeProblem(v: string): string | null {
  const parts = v.split(':');
  const ok =
    (parts.length === 2 || (parts.length === 3 && (parts[2] === 'ro' || parts[2] === 'rw'))) &&
    (parts[0]!.startsWith('/') || /^[A-Za-z0-9][A-Za-z0-9_.-]*$/.test(parts[0]!)) &&
    parts[1]!.startsWith('/');
  return ok ? null : `"${v}" is not /host/path:/path[:ro] or name:/path[:ro].`;
}

export function publishProblem(p: string): string | null {
  const m = /^(?:(?:\[[0-9a-fA-F:]+\]|[0-9.]+):)?(\d{1,5}):(\d{1,5})(?:\/(tcp|udp))?$/.exec(p);
  const ok = m && [m[1], m[2]].every((n) => Number(n) >= 1 && Number(n) <= 65535);
  return ok ? null : `"${p}" is not [address:]host:container[/udp].`;
}

export function deviceProblem(d: string): string | null {
  const parts = d.split(':');
  const [host, container = host, perms = 'rwm'] = parts;
  const devPath = (p: string) => /^\/dev\/\S+$/.test(p) && !p.split('/').includes('..');
  const ok =
    parts.length <= 3 &&
    devPath(host!) &&
    container!.startsWith('/') &&
    !container!.includes('..') &&
    /^[rwm]{1,3}$/.test(perms!) &&
    new Set(perms).size === perms!.length;
  return ok ? null : `"${d}" is not /dev/host[:/dev/container[:rwm]].`;
}

export function groupProblem(g: string): string | null {
  const ok = /^\d+$/.test(g) ? Number(g) <= 0xffffffff : /^[a-z_][a-z0-9_-]{0,31}$/.test(g);
  return ok ? null : `"${g}" is not a group name or number.`;
}

/** Splits a command like a shell would, for simple quoting. */
export function splitCommand(text: string): string[] {
  const out: string[] = [];
  const re = /"((?:[^"\\]|\\.)*)"|'([^']*)'|(\S+)/g;
  for (const m of text.matchAll(re)) out.push(m[1]?.replace(/\\(.)/g, '$1') ?? m[2] ?? m[3]!);
  return out;
}

export function showCommand(cmd: string[] | undefined): string {
  return (cmd ?? []).map((a) => (/^[\w@%+=:,./-]+$/.test(a) ? a : `"${a.replace(/(["\\])/g, '\\$1')}"`)).join(' ');
}

/** The image without registry and digest, for display: "nginx:1.27". */
export function shortImage(image: string): string {
  return image.replace(/@sha256:[0-9a-f]+$/, '').replace(/^docker\.io\/(library\/)?/, '');
}

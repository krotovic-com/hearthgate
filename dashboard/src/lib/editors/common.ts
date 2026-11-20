// Small helpers for the edit forms.

export const NAME_PATTERN = '[a-z0-9]([a-z0-9-]*[a-z0-9])?';
const nameRe = new RegExp(`^${NAME_PATTERN}$`);

export function nameProblem(name: string, taken: string[], what: string): string | null {
  if (!name) return `Give the ${what} a name.`;
  if (!nameRe.test(name)) return 'Use lowercase letters, digits and dashes.';
  if (taken.includes(name)) return `Another ${what} already has this name.`;
  return null;
}

/** "cloud.example.com" → "cloud-example-com". */
export function slug(text: string): string {
  return text
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 63);
}

/** Splits "a, b c" into ["a", "b", "c"]. */
export function list(text: string): string[] {
  return text
    .split(/[\s,]+/)
    .map((s) => s.trim())
    .filter(Boolean);
}

export function portProblem(text: string | number): string | null {
  const n = Number(text);
  if (!Number.isInteger(n) || n < 1 || n > 65535) return 'Use a port from 1 to 65535.';
  return null;
}

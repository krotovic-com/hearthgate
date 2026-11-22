// Pure helpers for the AI assistant's screens.

import type { AiFinding, AiProvider, AiReview, AiScope, AiSeverity, PatchOp } from './api';
import type { BadgeStatus } from './ui/Badge.svelte';

export const severityBadge: Record<AiSeverity, { label: string; status: BadgeStatus }> = {
  high: { label: 'High', status: 'critical' },
  medium: { label: 'Medium', status: 'warning' },
  low: { label: 'Low', status: 'neutral' },
};

export const providerLabel: Record<AiProvider, string> = {
  anthropic: 'Claude · Anthropic API',
  openai: 'OpenAI',
  gemini: 'Google Gemini',
  'openai-compatible': 'OpenAI-compatible (local)',
};

export function scopeLabel(scope: AiScope): string {
  switch (scope.kind) {
    case 'all':
      return 'Whole setup';
    case 'firewall':
      return 'Firewall rules';
    case 'service':
      return `Service ${scope.service}`;
  }
}

/** "2 high · 3 medium · 1 low", leaving out zeros. */
export function severityCounts(findings: AiFinding[]): string {
  const parts = (['high', 'medium', 'low'] as const)
    .map((s) => [s, findings.filter((f) => f.severity === s).length] as const)
    .filter(([, n]) => n > 0)
    .map(([s, n]) => `${n} ${s}`);
  return parts.length ? parts.join(' · ') : 'all clear';
}

/** The newest review for `scope`, if any. */
export function latestFor(reviews: AiReview[], scope: AiScope): AiReview | null {
  const key = JSON.stringify(scope);
  return reviews.find((r) => JSON.stringify(r.scope) === key) ?? null;
}

function show(value: unknown): string {
  return typeof value === 'string' ? value : JSON.stringify(value);
}

/** A fix's changes as diff-like lines, such as `+ /firewall/rules/- {…}`. */
export function changeLines(changes: PatchOp[]): { sign: '+' | '-' | '~'; text: string }[] {
  return changes.map((c) => {
    const path = c.path.replace(/^\//, '').replaceAll('/', ' › ') || 'everything';
    if (c.op === 'remove') return { sign: '-', text: `remove ${path}` };
    if (c.op === 'add') return { sign: '+', text: `${path}: ${show(c.value)}` };
    return { sign: '~', text: `${path} → ${show(c.value)}` };
  });
}

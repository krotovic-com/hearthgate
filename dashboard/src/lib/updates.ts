// Pure helpers for the update screens.

import type { AutoInstall, AvailableUpdate, NoteKind, ReleaseNote, UpdateInfo, UpdateJob } from './api';
import type { BadgeStatus } from './ui/Badge.svelte';
import { plural } from './format';

export const noteBadge: Record<NoteKind, { label: string; status: BadgeStatus }> = {
  breaking: { label: 'Breaking', status: 'warning' },
  security: { label: 'Security', status: 'critical' },
  feature: { label: 'Feature', status: 'public' },
  fix: { label: 'Fix', status: 'healthy' },
};

export const autoInstallLabel: Record<AutoInstall, string> = {
  off: 'Never, ask me first',
  patch: 'Patch releases only',
  minor: 'Patch and minor releases',
};

/** "4 features, 6 fixes, 1 security fix", leaving out zeros. */
export function noteCounts(notes: ReleaseNote[]): string {
  const n = (k: NoteKind) => notes.filter((x) => x.kind === k).length;
  const parts = [
    n('breaking') && plural(n('breaking'), 'breaking change'),
    n('feature') && plural(n('feature'), 'feature'),
    n('fix') && plural(n('fix'), 'fix', 'fixes'),
    n('security') && plural(n('security'), 'security fix', 'security fixes'),
  ].filter(Boolean);
  return parts.length ? parts.join(', ') : 'see the release notes';
}

/** "03:00-05:00" as "03:00–05:00". */
export function windowLabel(window: string): string {
  return window.replace('-', '–');
}

/** The start of the window, "03:00". */
export function windowStart(window: string): string {
  return window.split('-')[0] ?? window;
}

/** Window choices, keeping a custom one from the configuration file. */
export function windowOptions(current: string): { value: string; label: string }[] {
  const presets = ['00:00-02:00', '01:00-03:00', '02:00-04:00', '03:00-05:00', '04:00-06:00', '23:00-01:00'];
  const all = presets.includes(current) ? presets : [current, ...presets];
  return all.map((w) => ({ value: w, label: `Every day · ${windowLabel(w)}` }));
}

/** What the person gets with this release, in one sentence. */
export function bumpSentence(current: string, a: AvailableUpdate): string {
  switch (a.bump) {
    case 'patch':
      return `You have ${current}. This is a patch release with fixes only.`;
    case 'minor':
      return `You have ${current}. This is a minor release, so your configuration stays as it is.`;
    case 'major':
      return `You have ${current}. This is a major release: read the breaking changes before you install it.`;
  }
}

/** Why an available release waits, for the Overview notice. */
export function waitsReason(info: UpdateInfo, a: AvailableUpdate): string {
  const security = a.notes.filter((n) => n.kind === 'security').length;
  const lead = security ? `Includes ${plural(security, 'security fix', 'security fixes')}. ` : '';
  if (!info.can_install) return `${lead}${info.cannot_install ?? ''}`.trim();
  if (info.scheduled === a.version) return `${lead}It installs at ${windowStart(info.settings.window)}.`;
  const auto = info.settings.auto_install;
  if (auto === 'off') return `${lead}Automatic installs are off, so it waits for you.`;
  if (a.bump === 'major') return `${lead}Major releases always wait for you.`;
  if (auto === 'patch' && a.bump === 'minor')
    return `${lead}Automatic install is set to patch releases only, so this one waits for you.`;
  return `${lead}It installs by itself in the install window, ${windowLabel(info.settings.window)}.`;
}

/** Whether the helper is still working on this job. */
export function jobRunning(job: UpdateJob | null, now = Date.now() / 1000): boolean {
  return !!job && job.outcome === 'running' && now - job.started_unix < 30 * 60;
}

/** "Step 4 of 5". */
export function stepPosition(job: UpdateJob): string {
  const i = job.steps.findIndex((s) => s.state === 'running' || s.state === 'pending');
  const at = i < 0 ? job.steps.length : i + 1;
  return `Step ${at} of ${job.steps.length}`;
}

/** "Today at 09:12", "Yesterday at 03:04" or "26 Nov at 03:04". */
export function when(unix: number, now = new Date()): string {
  const d = new Date(unix * 1000);
  const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', hour12: false });
  const day = (x: Date) => new Date(x.getFullYear(), x.getMonth(), x.getDate()).getTime();
  const diff = Math.round((day(now) - day(d)) / 86_400_000);
  if (diff === 0) return `Today at ${time}`;
  if (diff === 1) return `Yesterday at ${time}`;
  return `${d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' })} at ${time}`;
}

/** "26 Nov" from an RFC 3339 date. */
export function shortDate(rfc3339: string): string {
  const d = new Date(rfc3339);
  return Number.isNaN(d.getTime()) ? '' : d.toLocaleDateString('en-GB', { day: 'numeric', month: 'short' });
}

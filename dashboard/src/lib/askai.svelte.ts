// Ask AI: the chat side panel. The conversation lives here, outside the
// pages, so it stays as it is while the user moves between screens, and
// it is kept in sessionStorage so a reload doesn't lose it. The context
// (screen, selected item, open dialog) follows the user; each change is
// noted in the conversation so the assistant knows what happened.
//
// Proposed changes are never applied by the assistant. The user applies
// one (it becomes a revision like any other change, and can be undone),
// puts it in the draft to edit it in the normal form first, or skips it.

import { api, type ChatContext, type ChatTurn, type Plan, type Proposal } from './api';
import { gw, isDirty, replaceDraft, applyDraft, rollbackTo } from './gateway.svelte';
import { navigate } from './router.svelte';
import { overlays, fieldsOf } from './overlay.svelte';
import { proposalTarget, turnText } from './askai';

export type ProposalState = 'pending' | 'applied' | 'draft' | 'skipped' | 'undone';

export interface UiProposal extends Proposal {
  state: ProposalState;
  /** The revision it was applied as. */
  revision?: number;
  /** The revision before it, to undo to. */
  previous?: number | null;
}

export interface ChatItem {
  id: number;
  role: ChatTurn['role'];
  text: string;
  proposals?: UiProposal[];
  plan?: Plan | null;
  suggestions?: string[];
  /** For context notes: what kind of change it was. */
  icon?: 'screen' | 'dialog' | 'selection' | 'change';
}

interface Screen {
  title: string;
  path: string;
  selection: string | null;
}

const STORAGE = 'hg-askai';
const nextId = (items: ChatItem[]) => (items.length ? items[items.length - 1]!.id + 1 : 1);

function restore(): { open: boolean; items: ChatItem[] } {
  try {
    const raw = sessionStorage.getItem(STORAGE);
    if (raw) {
      const parsed = JSON.parse(raw) as { open?: boolean; items?: ChatItem[] };
      return { open: !!parsed.open, items: Array.isArray(parsed.items) ? parsed.items : [] };
    }
  } catch {
    // Private mode or a broken entry: start fresh.
  }
  return { open: false, items: [] };
}

const saved = typeof sessionStorage === 'undefined' ? { open: false, items: [] } : restore();

export const askai = $state<{
  open: boolean;
  items: ChatItem[];
  busy: boolean;
  error: string | null;
  screen: Screen;
  /** The title of the open dialog, if any. */
  dialog: string | null;
}>({
  open: saved.open,
  items: saved.items,
  busy: false,
  error: null,
  screen: { title: '', path: '/', selection: null },
  dialog: null,
});

function persist(): void {
  try {
    sessionStorage.setItem(STORAGE, JSON.stringify({ open: askai.open, items: $state.snapshot(askai.items) }));
  } catch {
    // Storage full or blocked: the chat still works for this page.
  }
}

export function toggleAskAi(open = !askai.open): void {
  askai.open = open;
  persist();
}

export function newChat(): void {
  askai.items = [];
  askai.error = null;
  persist();
}

/** The latest plan in the conversation. Replies that don't touch the
 * plan leave it as it was. */
export function currentPlan(): Plan | null {
  for (let i = askai.items.length - 1; i >= 0; i--) {
    const plan = askai.items[i]!.plan;
    if (plan?.steps.length) return plan;
  }
  return null;
}

function push(item: Omit<ChatItem, 'id'>): void {
  askai.items = [...askai.items, { ...item, id: nextId(askai.items) }];
  persist();
}

/** Notes a change of context in the conversation. Repeated screen and
 * dialog notes collapse into the latest, so wandering around doesn't fill
 * the chat. */
function note(text: string, icon: ChatItem['icon']): void {
  if (!askai.items.some((i) => i.role !== 'context')) return;
  const last = askai.items[askai.items.length - 1];
  if (last?.role === 'context' && last.icon !== 'change' && icon !== 'change') {
    askai.items = [...askai.items.slice(0, -1), { ...last, text, icon }];
    persist();
    return;
  }
  push({ role: 'context', text, icon });
}

/** Called by App whenever the screen or the selected item changes. */
export function setScreen(screen: Screen): void {
  const before = askai.screen;
  askai.screen = screen;
  if (!before.title) return;
  if (before.title !== screen.title) {
    note(
      screen.selection ? `You opened ${screen.title} · ${screen.selection}` : `You opened ${screen.title}`,
      'screen',
    );
  } else if (before.selection !== screen.selection && screen.selection) {
    note(`You selected ${screen.selection} on ${screen.title}`, 'selection');
  }
}

/** Called by App when the topmost dialog changes. */
export function setDialog(title: string | null): void {
  const before = askai.dialog;
  askai.dialog = title;
  if (before === title) return;
  if (title) note(`You opened the dialog “${title}”`, 'dialog');
  else if (before) note(`You closed the dialog “${before}”`, 'dialog');
}

/** What the assistant can see right now. */
export function context(): ChatContext {
  const top = overlays.top;
  return {
    screen: askai.screen.title,
    path: askai.screen.path,
    selection: askai.screen.selection,
    dialog: top ? { title: top.title, fields: fieldsOf(top.el) } : null,
  };
}

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export async function send(text: string): Promise<void> {
  const question = text.trim();
  if (!question || askai.busy) return;
  askai.error = null;
  push({ role: 'user', text: question });
  askai.busy = true;
  try {
    const turns: ChatTurn[] = askai.items.map((i) => ({ role: i.role, text: turnText(i) }));
    const draft = gw.loaded && isDirty() ? $state.snapshot(gw.draft) : undefined;
    const reply = await api.aiChat(turns, context(), draft);
    push({
      role: 'assistant',
      text: reply.reply,
      proposals: reply.proposals.map((p) => ({ ...p, state: 'pending' as const })),
      plan: reply.plan ?? null,
      suggestions: reply.suggestions,
    });
  } catch (e) {
    askai.error = message(e);
  } finally {
    askai.busy = false;
  }
}

function update(itemId: number, index: number, change: Partial<UiProposal>): void {
  askai.items = askai.items.map((i) =>
    i.id === itemId && i.proposals
      ? { ...i, proposals: i.proposals.map((p, n) => (n === index ? { ...p, ...change } : p)) }
      : i,
  );
  persist();
}

function proposalOf(itemId: number, index: number): UiProposal {
  const p = askai.items.find((i) => i.id === itemId)?.proposals?.[index];
  if (!p) throw new Error('That proposal is gone.');
  return p;
}

/** Applies a proposal as a new revision. Refused while the draft has other
 * unapplied changes, so they aren't applied along with it by surprise. */
export async function applyProposal(itemId: number, index: number): Promise<void> {
  const p = proposalOf(itemId, index);
  if (isDirty()) {
    throw new Error('Your draft has other changes that are not applied. Apply or discard them first, or use Open in form.');
  }
  const previous = gw.revision?.id ?? null;
  const next = await api.aiProposalDraft($state.snapshot(gw.current), $state.snapshot(p.changes));
  replaceDraft(next.state);
  let revision;
  try {
    revision = await applyDraft(`Ask AI: ${p.title}`);
  } catch (e) {
    // The change stays in the draft, where the draft bar shows it.
    update(itemId, index, { state: 'draft' });
    throw new Error(`${message(e)} The change is in your draft, so you can fix it in the form.`);
  }
  update(itemId, index, { state: 'applied', revision: revision.id, previous });
  note(`Applied “${p.title}” as revision ${revision.id}.`, 'change');
}

/** Puts a proposal in the draft and opens the form it belongs to. */
export async function openProposal(itemId: number, index: number): Promise<void> {
  const p = proposalOf(itemId, index);
  const next = await api.aiProposalDraft($state.snapshot(gw.draft), $state.snapshot(p.changes));
  replaceDraft(next.state);
  update(itemId, index, { state: 'draft' });
  note(`Put “${p.title}” in the draft to edit it in the form.`, 'change');
  navigate(proposalTarget(p, next.state));
}

export function skipProposal(itemId: number, index: number): void {
  const p = proposalOf(itemId, index);
  update(itemId, index, { state: 'skipped' });
  note(`Skipped “${p.title}”.`, 'change');
}

export function restoreProposal(itemId: number, index: number): void {
  update(itemId, index, { state: 'pending' });
}

/** Rolls back to the revision before the proposal, if nothing was applied
 * since. */
export async function undoProposal(itemId: number, index: number): Promise<void> {
  const p = proposalOf(itemId, index);
  if (p.previous === undefined || p.previous === null) throw new Error('There is no earlier revision to go back to.');
  if (gw.revision?.id !== p.revision) {
    throw new Error('Other changes were applied since. Roll back in History instead.');
  }
  const revision = await rollbackTo(p.previous);
  update(itemId, index, { state: 'undone' });
  note(`Undid “${p.title}” (rolled back to revision ${p.previous}, now revision ${revision.id}).`, 'change');
}

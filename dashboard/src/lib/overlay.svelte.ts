// Open dialogs, topmost last. Dialogs open without the browser's modal
// mode so the Ask AI panel stays usable next to them; instead, the page
// behind them is made inert here and in App, and only the topmost dialog
// takes input.

export interface Overlay {
  id: number;
  title: string;
  el: HTMLElement;
}

let stack = $state.raw<Overlay[]>([]);
let next = 0;

export const overlays = {
  get stack(): Overlay[] {
    return stack;
  },
  get top(): Overlay | null {
    return stack[stack.length - 1] ?? null;
  },
};

function refresh(): void {
  stack.forEach((o, i) => {
    o.el.inert = i < stack.length - 1;
  });
}

export function pushOverlay(title: string, el: HTMLElement): number {
  const id = ++next;
  stack = [...stack, { id, title, el }];
  refresh();
  return id;
}

export function popOverlay(id: number): void {
  stack = stack.filter((o) => o.id !== id);
  refresh();
}

/** The fields of a dialog as the user sees them, for Ask AI's context. */
export function fieldsOf(el: HTMLElement): { label: string; value: string }[] {
  const out: { label: string; value: string }[] = [];
  const fields = el.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>(
    'input, select, textarea',
  );
  for (const f of fields) {
    if (f instanceof HTMLInputElement && ['password', 'hidden', 'file', 'submit', 'button'].includes(f.type)) {
      continue;
    }
    const label = (
      f.labels?.[0]?.textContent ??
      f.getAttribute('aria-label') ??
      f.getAttribute('name') ??
      ''
    )
      .replace(/\s+/g, ' ')
      .trim();
    let value: string;
    if (f instanceof HTMLInputElement && (f.type === 'checkbox' || f.type === 'radio')) {
      if (f.type === 'radio' && !f.checked) continue;
      value = f.type === 'radio' ? f.value || 'selected' : f.checked ? 'on' : 'off';
    } else if (f instanceof HTMLSelectElement) {
      value = f.selectedOptions[0]?.textContent?.trim() ?? f.value;
    } else {
      value = f.value;
    }
    if (!label && !value) continue;
    out.push({ label: label.slice(0, 120), value: value.slice(0, 500) });
    if (out.length >= 40) break;
  }
  return out;
}

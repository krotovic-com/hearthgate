import { render, screen } from '@testing-library/svelte';
import { describe, expect, it } from 'vitest';
import DiffView from './DiffView.svelte';

describe('DiffView', () => {
  it('marks added and removed lines for screen readers', () => {
    const { container } = render(DiffView, { diff: '@@ -1 +1 @@\n-port = 80\n+port = 8080\n' });
    expect(container.querySelector('.add')?.textContent).toContain('added: +port = 8080');
    expect(container.querySelector('.del')?.textContent).toContain('removed: -port = 80');
  });
  it('says when nothing changed', () => {
    render(DiffView, { diff: '' });
    expect(screen.getByText('No changes.')).toBeInTheDocument();
  });
});

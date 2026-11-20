// A small router on the History API: the server answers every app path
// with index.html, so links are plain paths like /services.

const here = typeof location === 'undefined' ? { pathname: '/', search: '' } : location;

export const route = $state({ path: here.pathname, search: here.search });

/** A query parameter of the current address. */
export function param(name: string): string | null {
  return new URLSearchParams(route.search).get(name);
}

export function navigate(to: string, replace = false): void {
  const url = new URL(to, location.href);
  if (url.pathname === route.path && url.search === route.search) return;
  if (replace) history.replaceState(null, '', url.pathname + url.search);
  else history.pushState(null, '', url.pathname + url.search);
  const sameSection = url.pathname.split('/')[1] === route.path.split('/')[1];
  route.path = url.pathname;
  route.search = url.search;
  if (!sameSection) window.scrollTo(0, 0);
}

if (typeof window !== 'undefined') {
  window.addEventListener('popstate', () => {
    route.path = location.pathname;
    route.search = location.search;
  });
  // Handle clicks on same-origin links without a full page load.
  document.addEventListener('click', (event) => {
    if (event.defaultPrevented || event.button !== 0) return;
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey) return;
    const anchor = (event.target as Element | null)?.closest?.('a');
    if (!anchor || anchor.target || anchor.hasAttribute('download')) return;
    const url = new URL(anchor.href, location.href);
    if (url.origin !== location.origin || url.pathname.startsWith('/api/')) return;
    event.preventDefault();
    navigate(url.pathname + url.search);
  });
}

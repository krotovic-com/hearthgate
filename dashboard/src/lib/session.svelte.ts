import { api, setCsrf, setUnauthorizedHandler, type SessionInfo } from './api';

/** Who is logged in. `null` until the first check finishes. */
export const session = $state<{ info: SessionInfo | null; error: string | null }>({
  info: null,
  error: null,
});

export function setSession(info: SessionInfo): void {
  session.info = info;
  session.error = null;
  setCsrf(info.csrf);
}

export async function loadSession(): Promise<void> {
  try {
    setSession(await api.session());
  } catch (e) {
    session.error = e instanceof Error ? e.message : String(e);
  }
}

export async function logout(): Promise<void> {
  try {
    await api.logout();
  } finally {
    setSession({ setup_required: false, user: null, csrf: null });
  }
}

setUnauthorizedHandler(() => {
  if (session.info?.user) {
    setSession({ setup_required: false, user: null, csrf: null });
  }
});

// Updates of Hearthgate itself: what is installed, what is available, and
// the install or rollback going on. During an install the gateway restarts,
// so requests fail for a few seconds; polling keeps going until it answers.

import { api, type UpdateInfo } from './api';
import { jobRunning } from './updates';

export const updates = $state<{
  info: UpdateInfo | null;
  error: string | null;
  checking: boolean;
  busy: boolean;
  /** The gateway is restarting and doesn't answer yet. */
  reconnecting: boolean;
}>({ info: null, error: null, checking: false, busy: false, reconnecting: false });

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));
let timer: ReturnType<typeof setTimeout> | null = null;
let waiting = false;
/** Jobs that started before the request are old ones. */
let startedAfter = 0;

export async function loadUpdates(): Promise<void> {
  try {
    updates.info = await api.updates();
    updates.reconnecting = false;
  } catch (e) {
    if (waiting) updates.reconnecting = true;
    else updates.error = message(e);
  }
  follow();
}

/** Polls every two seconds while a job runs or the gateway is restarting. */
function follow(): void {
  const active = waiting || updates.reconnecting || jobRunning(updates.info?.job ?? null);
  if (timer) clearTimeout(timer);
  timer = null;
  if (!active) return;
  timer = setTimeout(() => {
    if (updates.info?.job && !jobRunning(updates.info.job) && updates.info.job.started_unix >= startedAfter) {
      waiting = false;
    }
    void loadUpdates();
  }, 2000);
}
async function act(f: () => Promise<UpdateInfo>, wait = false): Promise<boolean> {
  updates.busy = true;
  updates.error = null;
  try {
    if (wait) {
      startedAfter = Math.floor(Date.now() / 1000) - 5;
      waiting = true;
    }
    updates.info = await f();
    follow();
    return true;
  } catch (e) {
    waiting = false;
    updates.error = message(e);
    return false;
  } finally {
    updates.busy = false;
  }
}

export async function checkNow(): Promise<void> {
  updates.checking = true;
  await act(api.checkUpdates);
  updates.checking = false;
}

export const installNow = (version: string) => act(() => api.installUpdate(version, 'now'), true);
export const installInWindow = (version: string) => act(() => api.installUpdate(version, 'window'));
export const unschedule = () => act(api.unscheduleUpdate);
export const rollBack = () => act(api.rollbackUpdate, true);
export const skip = (version: string) => act(() => api.skipUpdate(version));
export const unskip = (version: string) => act(() => api.unskipUpdate(version));

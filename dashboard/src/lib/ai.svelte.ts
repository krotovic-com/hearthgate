// The AI assistant's state: settings in effect, past reviews, and the
// review running now. Nothing here sends anything unless the user asks
// for a review and AI is on (the server refuses otherwise).

import { api, type AiInfo, type AiReview, type AiScope } from './api';
import { replaceDraft } from './gateway.svelte';

export const ai = $state<{
  info: AiInfo | null;
  reviews: AiReview[];
  loaded: boolean;
  running: string | null;
  error: string | null;
}>({ info: null, reviews: [], loaded: false, running: null, error: null });

const message = (e: unknown) => (e instanceof Error ? e.message : String(e));

export async function loadAi(): Promise<void> {
  try {
    const [info, reviews] = await Promise.all([api.ai(), api.aiReviews()]);
    ai.info = info;
    ai.reviews = reviews;
    ai.loaded = true;
  } catch (e) {
    ai.error = message(e);
  }
}

/** Runs a review and adds it to the list. */
export async function runReview(scope: AiScope): Promise<AiReview | null> {
  ai.running = JSON.stringify(scope);
  ai.error = null;
  try {
    const review = await api.aiReview(scope);
    ai.reviews = [review, ...ai.reviews.filter((r) => r.id !== review.id)];
    return review;
  } catch (e) {
    ai.error = message(e);
    return null;
  } finally {
    ai.running = null;
  }
}

export function isRunning(scope: AiScope): boolean {
  return ai.running === JSON.stringify(scope);
}

/**
 * Puts a finding's fix into the draft, on top of the current
 * configuration. The draft bar then shows the diff to apply, and History
 * can roll it back like any change.
 */
export async function useFix(review: AiReview, index: number): Promise<string> {
  const fix = await api.aiFixDraft(review.id, index);
  replaceDraft(fix.state);
  return fix.summary;
}

// Pure decisions behind the store's mutation queue. No imports: `queue.check.ts` runs it under plain node.

export interface AtLike { index: number; name: string }
export interface StateLike { active: number; playsets: { name: string }[] }

/** The playset a queued action targets. Read when the action RUNS, from the latest applied state, so it follows an earlier switch. */
export function atFor(s: StateLike | null | undefined): AtLike | null {
  const p = s?.playsets[s.active];
  return s && p ? { index: s.active, name: p.name } : null;
}

/**
 * Whether a finished mutation's answer replaces the shown state now. Optimistic edits wait for the last queued
 * answer (later edits are already drawn on top); anything else changes what later actions read, so it shows at once.
 */
export const showResultNow = (optimistic: boolean, pending: number) => !optimistic || pending <= 1;

/** A load/summary answer is only valid if no mutation was enqueued since it started. */
export const responseIsCurrent = (startEpoch: number, epoch: number) => startEpoch === epoch;

/** Launch goes ahead only if the arguments save it waited for (if any) succeeded. */
export const mayLaunch = (argsSaved: boolean | null) => argsSaved !== false;

/** Which save-confirm token to send: the one on the dialog being confirmed, never a derived string. */
export const confirmExpect = (c: { token: string } | null) => c?.token;

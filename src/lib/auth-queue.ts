import type { AuthRequest } from "../types";

/**
 * The credential requests waiting on the user, oldest first.
 *
 * Two sessions opening at once can both ask, and each backend waiter stays
 * open until its own answer arrives or its timeout fires. A single slot that
 * showed whichever request came last left the earlier waiter with no dialog
 * that could ever answer it. So requests queue, the front one is shown, and
 * every answer travels under the id of the request it belongs to — the
 * backend routes on that id, so nothing here has to know which session is
 * which.
 */
export function enqueueRequest(queue: AuthRequest[], request: AuthRequest): AuthRequest[] {
  // A request announced twice takes its own place rather than a second one.
  if (queue.some(item => item.request === request.request)) {
    return queue.map(item => item.request === request.request ? request : item);
  }
  return [...queue, request];
}

export function dequeueRequest(queue: AuthRequest[], id: string): AuthRequest[] {
  return queue.filter(item => item.request !== id);
}

/** One empty answer per prompt, which is what every request starts with. */
export function blankAnswers(request: AuthRequest | undefined): string[] {
  return request ? request.prompts.map(() => "") : [];
}

/**
 * What has been typed, and which request it was typed into.
 *
 * Keyed to the request rather than kept as a bare array so a secret half
 * typed for one prompt can never show in the next one's fields, and so it can
 * be dropped the moment its request is over — submitted, dismissed, or
 * resolved by the backend — rather than sitting in state until the next
 * prompt happened to overwrite it. Hygiene, not a guarantee: JavaScript keeps
 * no promise about copies it has already made.
 */
export interface Typed {
  id: string;
  values: string[];
}

/** The fields' contents: the typing if it belongs to this request, blanks if not. */
export function answersFor(typed: Typed | null, request: AuthRequest | undefined): string[] {
  if (!request) return [];
  return typed && typed.id === request.request ? typed.values : blankAnswers(request);
}

/** `typed`, unless it belonged to the request that just ended. */
export function forget(typed: Typed | null, id: string): Typed | null {
  return typed && typed.id === id ? null : typed;
}

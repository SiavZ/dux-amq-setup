/**
 * Telling yaran's own replayed DOM events apart from the visitor's real ones.
 *
 * The OSC 8 link probe (`lib/termlink.ts`) and the touch-gesture forwarder
 * (`lib/termmouse.ts`) dispatch mouse events at xterm on purpose. Both must pass
 * through the pane's capture-phase listeners rather than be judged by them, since
 * the link intercept decides what a human press means, and a replay dispatched at
 * a descendant still travels the capture path from the container down.
 *
 * `isTrusted` is not the discriminator: jsdom dispatches are never trusted, so
 * such a guard would leave every component test of the intercept exercising
 * nothing, and a click synthesized by an assistive technology is untrusted too
 * while being real user intent. So yaran tags what yaran dispatched, and nothing
 * else. A `WeakSet` keeps the tag off the event object.
 */

const yaranReplays = new WeakSet<Event>()

/** Marks an event as one yaran dispatched itself. Returns it for chaining. */
export function markYaranReplay<T extends Event>(event: T): T {
  yaranReplays.add(event)
  return event
}

/** Whether this event came out of one of yaran's own replays. */
export function isYaranReplay(event: Event): boolean {
  return yaranReplays.has(event)
}

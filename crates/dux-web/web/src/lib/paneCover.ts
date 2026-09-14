import { useSyncExternalStore } from "react"

// Whether a full-pane cover owns one pane, keyed by pty id, for the chrome that
// sits outside the pane. Only the pane knows (it is the one that resolves the
// cover), and the phone's header has to know, so the pane publishes and the
// shell reads.
//
// Keyed rather than a single slot, on the `paneInputGroup` precedent: several
// panes can be mounted at once and a shell must read the one it mounted, never
// whichever published last.

/// Boxed so each registration has an identity of its own: two panes publishing
/// the same verdict must still be told apart when the outgoing one retires.
interface CoverEntry {
  ownsPane: boolean
}

const covered = new Map<string, CoverEntry>()
const listeners = new Set<() => void>()
// A monotonic counter IS the snapshot: `useSyncExternalStore` compares by value.
let version = 0

function publish(): void {
  version++
  for (const listener of listeners) listener()
}

/**
 * Publish this pane's cover verdict and return its retirement. The retirement
 * clears the entry only while it is still this pane's, so a replacement pane
 * survives the outgoing pane's late cleanup.
 */
export function registerPaneCover(ptyId: string, ownsPane: boolean): () => void {
  const entry: CoverEntry = { ownsPane }
  covered.set(ptyId, entry)
  publish()
  return () => {
    if (covered.get(ptyId) !== entry) return
    covered.delete(ptyId)
    publish()
  }
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener)
  return () => void listeners.delete(listener)
}

function snapshot(): number {
  return version
}

/**
 * Does a full-pane cover own this pty's pane? An id nobody has published for
 * reads as uncovered, which is what an unmounted or dormant pane is.
 */
export function usePaneCoverOwned(ptyId: string | null): boolean {
  useSyncExternalStore(subscribe, snapshot, snapshot)
  return paneCoverOwnedFor(ptyId)
}

/**
 * Has the pane behind this id published a verdict at all? "Uncovered" and "no
 * pane has answered yet" read the same through `usePaneCoverOwned`, and chrome
 * that animates on a change must tell them apart: a pane mounting late turns
 * its first publish into a change the user never saw.
 */
export function usePaneCoverKnown(ptyId: string | null): boolean {
  useSyncExternalStore(subscribe, snapshot, snapshot)
  return ptyId !== null && covered.has(ptyId)
}

/// The same verdict without the subscription, for a caller that is not a
/// component and so needs no re-render.
export function paneCoverOwnedFor(ptyId: string | null): boolean {
  return ptyId === null ? false : (covered.get(ptyId)?.ownsPane ?? false)
}

/** Test-only: forget every registration between cases. */
export function resetPaneCovers(): void {
  covered.clear()
  publish()
}

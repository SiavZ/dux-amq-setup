import { useEffect, useMemo, useRef, useState } from "react"
import type { RefObject } from "react"

import type { ChangesSliceView } from "@/lib/editorBuffers"
import { changedPathsFrom, dirsToRefetch } from "@/lib/fileTreeFreshness"
import { useRevisit } from "@/hooks/use-revisit"

// A batch of directories the tree should refetch in place, identified by a
// nonce so a repeat of the same set still lands.
export interface FileTreeRefresh {
  dirs: readonly string[]
  nonce: number
}

interface UseFileTreeFreshnessOptions {
  // This editor's changed-files slice, or null when it is not this root's.
  slice: ChangesSliceView | null
  // The tree's loaded directories, as the tree last reported them.
  loadedDirsRef: RefObject<readonly string[]>
  // The active editor tab's identity, "" when there is none. Switching tabs is
  // the third trigger the open buffers ride, and the tree rides it too.
  activeTabKey: string
  // Fires once per batch the tree is asked to refetch, whatever asked for it.
  // The flat search index answers the same question about the same worktree
  // off a different endpoint, so it has to be re-walked on the same triggers.
  onRefreshRequested: () => void
}

interface FileTreeFreshness {
  refresh: FileTreeRefresh | null
  onRefreshSettled: (nonce: number) => void
  // The explorer menu's Refresh files: every loaded directory, on demand.
  refreshAllLoadedDirs: () => void
}

// Keeps the editor's file tree in step with files nobody in this browser wrote,
// on events rather than a poll: the changed-files broadcast tells it which
// directories git saw an entry appear in or leave, and a revisit refetches
// every loaded one, which is how a file git never reports (an ignored one)
// still shows up.
export function useFileTreeFreshness({
  slice,
  loadedDirsRef,
  activeTabKey,
  onRefreshRequested,
}: UseFileTreeFreshnessOptions): FileTreeFreshness {
  const [refresh, setRefresh] = useState<FileTreeRefresh | null>(null)
  const nonceRef = useRef(0)
  // The nonce of the revisit refresh still in flight, so a second revisit
  // arriving behind it asks for nothing. Released on the settle, on a newer
  // request replacing it, and on the tree unmounting with it outstanding.
  const revisitNonceRef = useRef<number | null>(null)
  // The changed paths as of the last slice seen. Null until one arrives: the
  // first slice is the baseline the tree was already fetched against.
  const seenPathsRef = useRef<readonly string[] | null>(null)

  // Read through a ref so an inline arrow from the caller cannot make the
  // request path depend on render identity.
  const onRefreshRequestedRef = useRef(onRefreshRequested)
  useEffect(() => {
    onRefreshRequestedRef.current = onRefreshRequested
  })

  function request(dirs: readonly string[]): number {
    nonceRef.current += 1
    const nonce = nonceRef.current
    // A newer batch replaces the one the gate is holding, which the tree may
    // never have seen at all: the settle that eventually arrives names this
    // nonce, so holding the older one would arm the gate for good.
    revisitNonceRef.current = null
    setRefresh({ dirs: [...dirs], nonce })
    onRefreshRequestedRef.current()
    return nonce
  }

  function refreshLoadedDirs(gated: boolean): void {
    if (gated && revisitNonceRef.current !== null) return
    const dirs = loadedDirsRef.current ?? []
    if (dirs.length === 0) return
    const nonce = request(dirs)
    if (gated) revisitNonceRef.current = nonce
  }

  useRevisit(() => refreshLoadedDirs(true))

  // Tab activation, the buffers' third trigger. The first render is skipped:
  // the tree is fetching its directories for the first time anyway.
  const seenTabKeyRef = useRef<string | null>(null)
  useEffect(() => {
    const previous = seenTabKeyRef.current
    seenTabKeyRef.current = activeTabKey
    if (previous !== null) refreshLoadedDirs(true)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [activeTabKey])

  const changedPathsKey = useMemo(
    () => JSON.stringify(changedPathsFrom(slice)),
    [slice],
  )

  useEffect(() => {
    if (!slice || slice.phase !== "loaded") return
    const next = JSON.parse(changedPathsKey) as string[]
    const previous = seenPathsRef.current
    seenPathsRef.current = next
    if (previous === null) return
    const dirs = dirsToRefetch(
      previous,
      next,
      new Set(loadedDirsRef.current ?? []),
    )
    if (dirs.length === 0) return
    request(dirs)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [changedPathsKey, slice?.phase])

  return {
    refresh,
    onRefreshSettled: (nonce: number) => {
      if (revisitNonceRef.current === nonce) revisitNonceRef.current = null
    },
    refreshAllLoadedDirs: () => refreshLoadedDirs(false),
  }
}

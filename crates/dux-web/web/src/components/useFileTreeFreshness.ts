import { useEffect, useMemo, useRef, useState } from "react"
import type { RefObject } from "react"

import type { ChangesSliceView } from "@/lib/editorBuffers"
import { changedPathsFrom, dirsToRefetch } from "@/lib/fileTreeFreshness"

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
}

interface FileTreeFreshness {
  refresh: FileTreeRefresh | null
  onRefreshSettled: (nonce: number) => void
  // The explorer menu's Refresh files: every loaded directory, on demand.
  refreshAllLoadedDirs: () => void
}

// The two revisit triggers the open buffers already ride: the window regaining
// focus, and this tab becoming the visible one. Both funnel into one handler,
// because coming back to a backgrounded tab commonly fires both.
function useRevisit(onRevisit: () => void): void {
  const handlerRef = useRef(onRevisit)
  useEffect(() => {
    handlerRef.current = onRevisit
  })
  useEffect(() => {
    const fire = () => handlerRef.current()
    const onVisibility = () => {
      if (document.visibilityState === "visible") fire()
    }
    window.addEventListener("focus", fire)
    document.addEventListener("visibilitychange", onVisibility)
    return () => {
      window.removeEventListener("focus", fire)
      document.removeEventListener("visibilitychange", onVisibility)
    }
  }, [])
}

// Keeps the editor's file tree in step with files nobody in this browser wrote,
// on events rather than a poll: the changed-files broadcast tells it which
// directories git saw an entry appear in or leave, and a revisit refetches
// every loaded one, which is how a file git never reports (an ignored one)
// still shows up.
export function useFileTreeFreshness({
  slice,
  loadedDirsRef,
}: UseFileTreeFreshnessOptions): FileTreeFreshness {
  const [refresh, setRefresh] = useState<FileTreeRefresh | null>(null)
  const nonceRef = useRef(0)
  // The nonce of the revisit refresh still in flight, so a second revisit
  // arriving behind it asks for nothing.
  const revisitNonceRef = useRef<number | null>(null)
  // The changed paths as of the last slice seen. Null until one arrives: the
  // first slice is the baseline the tree was already fetched against.
  const seenPathsRef = useRef<readonly string[] | null>(null)

  function request(dirs: readonly string[]): number {
    nonceRef.current += 1
    const nonce = nonceRef.current
    setRefresh({ dirs: [...dirs], nonce })
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

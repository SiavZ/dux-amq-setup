// Pure planner for keeping the editor's file tree fresh off the changed-files
// broadcast, the same signal the open buffers are questioned by. A path that
// entered or left the slice may have changed a directory LISTING; a path that
// merely changed content did not.

import type { ChangesSliceView } from "@/lib/editorBuffers"
import type { DirEntry } from "@/lib/fileTree"

// Every path git reports for the worktree, staged and unstaged alike, sorted so
// two reads of the same slice compare equal as strings. A rename contributes
// both of its ends: the listing it left is as stale as the one it arrived in.
export function changedPathsFrom(slice: ChangesSliceView | null): string[] {
  if (!slice) return []
  const paths = new Set<string>()
  for (const f of [...slice.unstaged, ...slice.staged]) {
    paths.add(f.path)
    if (f.renamed_from) paths.add(f.renamed_from)
  }
  return [...paths].sort()
}

// The directory a changed path lives in, "" for the worktree root. Stripping a
// trailing slash is defensive: dux reads git status with `--untracked-files=all`,
// which reports a new folder as its files rather than as "newdir/".
function parentDirOf(path: string): string {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path
  const cut = trimmed.lastIndexOf("/")
  return cut === -1 ? "" : trimmed.slice(0, cut)
}

// The deepest loaded directory at or above `dir`, or null when not even the
// root is loaded. A file in a folder git has only just seen has no loaded
// parent of its own, and the listing that gained an entry is its nearest
// loaded ancestor's.
function nearestLoadedDir(
  dir: string,
  loadedDirs: ReadonlySet<string>,
): string | null {
  let current = dir
  for (;;) {
    if (loadedDirs.has(current)) return current
    if (current === "") return null
    const cut = current.lastIndexOf("/")
    current = cut === -1 ? "" : current.slice(0, cut)
  }
}

// Which loaded directories may list something new or missing, given the changed
// paths before and after the slice moved. Only paths that ENTERED or LEFT count:
// a modified file's content change leaves every listing exactly as it was.
// Each such path is charged to the nearest loaded directory at or above it.
//
// `previousPaths` is null before any slice has been seen, which is a baseline
// rather than a change: the tree was just fetched.
export function dirsToRefetch(
  previousPaths: readonly string[] | null,
  nextPaths: readonly string[],
  loadedDirs: ReadonlySet<string>,
): string[] {
  if (previousPaths === null) return []
  const before = new Set(previousPaths)
  const after = new Set(nextPaths)
  const dirs = new Set<string>()
  for (const path of [...previousPaths, ...nextPaths]) {
    if (before.has(path) && after.has(path)) continue
    const dir = nearestLoadedDir(parentDirOf(path), loadedDirs)
    if (dir !== null) dirs.add(dir)
  }
  return [...dirs].sort()
}

// The subdirectories a refetched listing no longer has. Their cached listings
// are dropped with them, so a directory deleted and later recreated under the
// same name does not come back holding the old one's children.
export function vanishedDirPaths(
  before: readonly DirEntry[],
  after: readonly DirEntry[],
): string[] {
  const kept = new Set(after.filter((e) => e.is_dir).map((e) => e.path))
  return before.filter((e) => e.is_dir && !kept.has(e.path)).map((e) => e.path)
}

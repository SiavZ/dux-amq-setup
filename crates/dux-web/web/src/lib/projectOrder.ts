// The recency ordering of the project lists, a hand mirror of the core-owned
// `dux_core::project_order::order_projects_by_recency`. The rule lives there;
// the same cases are pinned here in `projectOrder.test.ts`, so an unmirrored
// change fails a test.
//
// A project's instant is the later of when it was added and when its newest
// agent was created; a standalone agent belongs to no project and counts for
// none.

import { workspaceProjectId } from "@/lib/agentWorkspace"
import type { ProjectView, SessionView } from "@/lib/types"

// Milliseconds since the epoch, or null when the timestamp is absent or
// unparseable (an empty `created_at` is a project with no store row yet).
function instant(timestamp: string | undefined): number | null {
  if (!timestamp) return null
  const ms = Date.parse(timestamp)
  return Number.isNaN(ms) ? null : ms
}

// The newest agent creation instant per project id, standalone agents excluded.
function newestAgentPerProject(sessions: SessionView[]): Map<string, number> {
  const newest = new Map<string, number>()
  for (const session of sessions) {
    const projectId = workspaceProjectId(session.workspace)
    if (!projectId) continue
    const ms = instant(session.created_at)
    if (ms === null) continue
    const current = newest.get(projectId)
    if (current === undefined || ms > current) newest.set(projectId, ms)
  }
  return newest
}

/** The projects ordered newest-touched first. A project with no instant at all
 * (no added date and no agents) goes last, keeping incoming order; the sort is
 * stable, so ties keep the stored order. */
export function orderProjectsByRecency(
  projects: ProjectView[],
  sessions: SessionView[],
): ProjectView[] {
  const newest = newestAgentPerProject(sessions)
  const lastTouched = new Map<string, number | null>()
  for (const project of projects) {
    const added = instant(project.created_at)
    const agent = newest.get(project.id) ?? null
    if (added === null && agent === null) lastTouched.set(project.id, null)
    else lastTouched.set(project.id, Math.max(added ?? -Infinity, agent ?? -Infinity))
  }
  return [...projects].sort((a, b) => {
    const left = lastTouched.get(a.id) ?? null
    const right = lastTouched.get(b.id) ?? null
    if (left === right) return 0
    if (left === null) return 1
    if (right === null) return -1
    return right - left
  })
}

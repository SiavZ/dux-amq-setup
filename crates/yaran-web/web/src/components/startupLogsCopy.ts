import { chip, type Prose } from "@/lib/prose"
import type { StartupLogsScope } from "@/lib/store"

// One dialog serves both scopes of `StartupCommandLogScope`, and only the
// naming differs: the title says which entity and the subtitle says how wide
// the list is, or an agent's runs and a project's would look identical. The
// entity's name is a chip; the generic noun standing in for a missing one is
// prose.
export function startupLogsCopy(
  scope: StartupLogsScope,
  projectName: string | undefined,
  agentName: string | undefined,
): { title: Prose; description: string; emptyMessage: string } {
  if (scope === "project") {
    return {
      title: projectName
        ? ["Startup command logs: ", chip(projectName), " (all agents)"]
        : ["Startup command logs: project (all agents)"],
      description:
        "Output from each run of the project startup command across every agent in this project, newest first.",
      emptyMessage:
        "No startup command logs yet. Run the startup command for an agent in this project to generate one.",
    }
  }
  return {
    title:
      agentName !== undefined
        ? ["Startup command logs: ", chip(agentName)]
        : ["Startup command logs: agent"],
    description:
      "Output from each run of the project startup command in this agent's worktree, newest first.",
    emptyMessage:
      "No startup command logs yet. Run the startup command for this agent to generate one.",
  }
}

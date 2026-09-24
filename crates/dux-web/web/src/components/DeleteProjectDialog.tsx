import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
import { closeDeleteProject, deleteProject, useDux } from "@/lib/store"
import { workspaceProjectId } from "@/lib/agentWorkspace"
import { deleteProjectProse } from "@/lib/projectConfirm"
import { renderProse } from "@/lib/prose"

// The destructive cascade counterpart to `RemoveProjectDialog`: it also deletes
// every agent's worktree from disk, so the copy spells that out. Offered only for
// a real project, so its open state goes through the vanish guard and the dialog
// closes itself rather than acting on a target the ViewModel no longer has.
export function DeleteProjectDialog() {
  const { deleteProjectTarget, spine } = useDux()

  const project = spine?.projects.find((p) => p.id === deleteProjectTarget)
  const isOpen = useVanishedTargetGuard(
    deleteProjectTarget !== null,
    project !== undefined,
    closeDeleteProject,
  )
  const name = project?.name
  const agentCount =
    spine?.sessions.filter(
      (s) => workspaceProjectId(s.workspace) === deleteProjectTarget,
    ).length ?? 0

  function handleConfirm() {
    if (!deleteProjectTarget) return
    deleteProject(deleteProjectTarget)
    closeDeleteProject()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeDeleteProject()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Delete project?</DialogTitle>
          <DialogDescription>
            {renderProse(deleteProjectProse(name, agentCount))}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeDeleteProject}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Delete
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

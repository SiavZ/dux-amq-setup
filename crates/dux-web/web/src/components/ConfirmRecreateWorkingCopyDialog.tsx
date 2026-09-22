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
import { managedWorkspace } from "@/lib/agentWorkspace"
import {
  recreateConfirmBody,
  recreateRunningProviders,
} from "@/lib/recreateWorkingCopy"
import {
  closeRecreateWorkingCopy,
  recreateWorkingCopy,
  useDux,
} from "@/lib/store"

// The confirmation behind putting back a working copy an agent deleted from
// under itself.
//
// Destructive about what was in that directory rather than about the agent: the
// directory comes back, its contents do not. The body says so, and says what a
// tab still running in the deleted directory makes of the recreated one, which
// is the provider's own answer rather than dux's.
export function ConfirmRecreateWorkingCopyDialog() {
  const { recreateWorkingCopyTarget, spine } = useDux()

  const session = recreateWorkingCopyTarget
    ? spine?.sessions.find((s) => s.id === recreateWorkingCopyTarget)
    : undefined
  const managed = session ? managedWorkspace(session.workspace) : null

  // Closes itself when the agent leaves the live view model, like every other
  // target-keyed dialog. A working copy that came back on its own counts: the
  // menu entry is gone by then and the dialog behind it has nothing to do.
  const isOpen = useVanishedTargetGuard(
    recreateWorkingCopyTarget !== null,
    session !== undefined && managed?.worktree_missing === true,
    closeRecreateWorkingCopy,
  )

  function handleConfirm() {
    if (!recreateWorkingCopyTarget) return
    recreateWorkingCopy(recreateWorkingCopyTarget)
    closeRecreateWorkingCopy()
  }

  function handleOpenChange(next: boolean) {
    if (!next) closeRecreateWorkingCopy()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Recreate working copy?</DialogTitle>
          <DialogDescription className="whitespace-pre-line">
            {managed && session
              ? recreateConfirmBody(
                  managed.worktree_label ?? managed.worktree_path,
                  managed.branch_name,
                  managed.source_branch,
                  managed.conversation_resumes === true,
                  recreateRunningProviders(session),
                )
              : ""}
          </DialogDescription>
        </DialogHeader>
        {/* Misclick-safe spacing between the body and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          <Button variant="outline" autoFocus onClick={closeRecreateWorkingCopy}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Recreate
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

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
import { checkoutDefaultBranchProse } from "@/lib/checkoutDefaultBranch"
import { renderProse } from "@/lib/prose"
import {
  checkoutDefaultBranch,
  closeCheckoutDefaultBranch,
  useDux,
} from "@/lib/store"

// Confirms switching a project's SOURCE checkout back to its default branch, which
// moves HEAD in a shared checkout. The server decides the target branch, so the
// copy describes the action generically.
export function CheckoutDefaultBranchDialog() {
  const { checkoutDefaultBranchTarget, spine } = useDux()

  const project = spine?.projects.find(
    (p) => p.id === checkoutDefaultBranchTarget,
  )
  // Closes the dialog when the project vanishes from the ViewModel: checking
  // out a branch of a project that no longer exists is moot. See the hook.
  const isOpen = useVanishedTargetGuard(
    checkoutDefaultBranchTarget !== null,
    project !== undefined,
    closeCheckoutDefaultBranch,
  )
  const name = project?.name ?? "this project"

  function handleConfirm() {
    if (!checkoutDefaultBranchTarget) return
    checkoutDefaultBranch(checkoutDefaultBranchTarget)
    closeCheckoutDefaultBranch()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeCheckoutDefaultBranch()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Check out default branch?</DialogTitle>
          <DialogDescription>
            {renderProse(
              checkoutDefaultBranchProse(name, project?.leading_branch ?? null),
            )}
          </DialogDescription>
        </DialogHeader>
        <DialogFooter>
          <Button variant="outline" onClick={closeCheckoutDefaultBranch}>
            Cancel
          </Button>
          {/* The terminal UI's confirm button reads the same. */}
          <Button onClick={handleConfirm}>Check out default branch</Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}

// Recreating a working copy an agent deleted from under itself: the pure half.
// What the confirmation says, and whether the action exists at all.
//
// The wording mirrors `dux_core::working_copy::recreate_confirm_body` word for
// word, and each side has a test asserting the exact sentences, so a change on
// one fails on the side that changed.

import { type AgentWorkspaceWire, matchWorkspace } from "./agentWorkspace"

/** Whether the recreate action exists for this agent. Its own helper rather
 * than an inline check, because the menu, the dialog and the test all have to
 * agree: a managed working copy that is gone, and nothing else. dux never
 * creates, moves or removes a standalone agent's folder. */
export function canRecreateWorkingCopy(workspace: AgentWorkspaceWire): boolean {
  return matchWorkspace(workspace, {
    managed: (w) => w.worktree_missing === true,
    folder: () => false,
  })
}

/** The body of the recreate confirmation.
 *
 * All three branch arms are stated because asking the server which one applies
 * would cost a round trip to open a dialog; the status that follows names the
 * arm that actually ran. The path is the server's, home-collapsed there,
 * because the browser is not necessarily on the server's machine. */
export function recreateConfirmBody(
  worktreeLabel: string,
  branchName: string,
  sourceBranch: string,
): string {
  return (
    `Recreate the working copy for this agent at ${worktreeLabel}?\n\n` +
    `If branch "${branchName}" still exists locally, dux checks it out there ` +
    `again. If it is gone locally but still on the remote, dux creates it ` +
    `again from "origin/${branchName}", holding everything that had been ` +
    `pushed. If it is gone everywhere, dux creates it again from ` +
    `"${sourceBranch}", and the commits that branch held are not coming ` +
    `back.\n\n` +
    `Any code changes that were in the old directory are gone either way: this ` +
    `puts the directory back, not its contents. The conversation may resume, ` +
    `because the agent's CLI keys its history by directory path and dux ` +
    `recreates the working copy at the same path.\n\n` +
    `Its tabs are dormant and stay that way, because dux refuses this while ` +
    `the agent is running. A terminal still open in the old directory keeps ` +
    `working in a directory that is gone; close it and open one in the ` +
    `recreated copy.`
  )
}

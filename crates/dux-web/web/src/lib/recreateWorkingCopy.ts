// Recreating a working copy an agent deleted from under itself: the pure half.
// What the confirmation says, and whether the action exists at all.
//
// The wording mirrors `dux_core::working_copy::recreate_confirm_body` word for
// word, and each side has a test asserting the exact sentences, so a change on
// one fails on the side that changed.

import { type AgentWorkspaceWire, matchWorkspace } from "./agentWorkspace"
import { chip, type Prose, proseText, quotedChip } from "./prose"
import type { SessionView } from "./types"

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

/** The providers of the tabs of this agent that are running right now, without
 * repeats, or the agent's own provider when none is.
 *
 * What a recreate does to a process is the running CLI's answer, and the
 * agent's provider is only the slot tab's: a dormant claude slot beside a live
 * codex extra is a codex process, and the mirror would promise the opposite. */
export function recreateRunningProviders(session: SessionView): string[] {
  const live: string[] = []
  for (const tab of session.tabs ?? []) {
    if (!tab.has_live_process) continue
    if (!live.includes(tab.provider)) live.push(tab.provider)
  }
  return live.length > 0 ? live : [session.provider]
}

/** Several provider names as one subject, each marked as a name: "Codex",
 * "Codex or Opencode". */
function providersJoined(names: string[]): Prose {
  return names.flatMap((name, index): Prose => {
    if (index === 0) return [chip(name)]
    return [index === names.length - 1 ? " or " : ", ", chip(name)]
  })
}

/** What the tabs still running in the deleted directory do once the working
 * copy is back, mirroring `dux_core::working_copy::recreate_running_tab_clause`.
 *
 * One non-claude provider among them makes the whole sentence the cautious one,
 * and it names every such tab, because each is one the user has to stop.
 *
 * Measured against the real CLIs. The Claude CLI never notices its directory
 * going, and once one exists at the same path again it keeps working in it with
 * its conversation intact. The Codex CLI answers every turn with "invalid cwd"
 * from the moment the directory goes and stays stuck there after the recreate,
 * until it is quit and resumed in the new folder. OpenCode and Copilot have not
 * been measured, so they get the cautious answer rather than a promise. */
export function recreateRunningTabClause(providers: string[]): string {
  return proseText(recreateRunningTabProse(providers))
}

/** The same clause with each provider marked, for the web to chip. */
function recreateRunningTabProse(providers: string[]): Prose {
  const name = (provider: string) =>
    provider.charAt(0).toUpperCase() + provider.slice(1)
  const cautious = providers
    .filter((p) => p.toLowerCase() !== "claude")
    .map(name)
  if (cautious.length === 0) {
    const only = providers.length > 0 ? name(providers[0]) : "Claude"
    return [
      "A running ",
      chip(only),
      " tab keeps working in the recreated copy by itself.",
    ]
  }
  return [
    "A running ",
    ...providersJoined(cautious),
    " tab cannot follow the folder: " +
      "stop it and start the agent again to continue in the recreated copy.",
  ]
}

/** The body of the recreate confirmation.
 *
 * All three branch arms are stated because asking the server which one applies
 * would cost a round trip to open a dialog; the status that follows names the
 * arm that actually ran. The path is the server's, home-collapsed there,
 * because the browser is not necessarily on the server's machine.
 *
 * `conversationResumes` is the agent's provider's own answer rather than a
 * constant: a provider that keeps no directory-scoped history (copilot ships
 * with no resume arguments) would have this dialog promising something that
 * cannot happen, and the same path buys it nothing.
 *
 * `providers` are the providers of the tabs running right now, or the agent's
 * own when none is, from `recreateRunningProviders`. */
export function recreateConfirmBody(
  worktreeLabel: string,
  branchName: string,
  sourceBranch: string,
  conversationResumes: boolean,
  providers: string[],
): string {
  return proseText(
    recreateConfirmProse(
      worktreeLabel,
      branchName,
      sourceBranch,
      conversationResumes,
      providers,
    ),
  )
}

/** The same body with the path and every branch marked, for the web to chip.
 * Its plain-text spelling is `recreateConfirmBody`. dux-core's
 * `recreate_confirm_prose` builds the same segments for the terminal UI, and
 * both are pinned by `crates/dux-core/tests/fixtures/prose_cross_language.json`. */
export function recreateConfirmProse(
  worktreeLabel: string,
  branchName: string,
  sourceBranch: string,
  conversationResumes: boolean,
  providers: string[],
): Prose {
  const conversation = conversationResumes
    ? `The conversation may resume, because the agent's CLI keys its history ` +
      `by directory path and dux recreates the working copy at the same path.`
    : `The conversation will not resume: this agent's CLI has no way to pick ` +
      `a conversation back up, so it starts fresh wherever it runs.`
  return [
    "Recreate the working copy for this agent at ",
    chip(worktreeLabel),
    "?\n\nIf branch ",
    quotedChip(branchName),
    ` still exists locally, dux checks it out there ` +
      `again. If it is gone locally but still on the remote, dux creates it ` +
      `again from `,
    quotedChip(`origin/${branchName}`),
    `, holding everything that had been ` +
      `pushed. If it is gone everywhere, dux creates it again from `,
    quotedChip(sourceBranch),
    `, and the commits that branch held are not coming ` +
      `back.\n\n` +
      `Any code changes that were in the old directory are gone either way: this ` +
      `puts the directory back, not its contents. ${conversation}\n\n`,
    ...recreateRunningTabProse(providers),
    ` A terminal still open in the old directory keeps ` +
      `working in a directory that is gone; close it and open one in the ` +
      `recreated copy.`,
  ]
}

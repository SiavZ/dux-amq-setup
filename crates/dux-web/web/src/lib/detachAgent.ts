// Detaching an agent: the pure half. What the confirmation says, and how long
// it says dux will wait.
//
// The wait is configurable, so the number is read from the bootstrap document
// at render time rather than written into the copy. The wording is kept here,
// beside that read, so the sentence and the number it quotes cannot come apart.

import {
  DEFAULT_SHUTDOWN_TIMEOUT_SECONDS,
  type Bootstrap,
} from "./bootstrapApi"
import { type Prose, quotedChip } from "./prose"

/** How long dux waits for an agent to exit before forcing it, in seconds.
 * The server clamps and projects it; an older server that omits it falls back
 * to the documented default. */
export function shutdownGraceSeconds(
  bootstrap: Pick<Bootstrap, "shutdown_timeout_seconds"> | null | undefined,
): number {
  const value = bootstrap?.shutdown_timeout_seconds
  return typeof value === "number" && Number.isFinite(value) && value >= 0
    ? value
    : DEFAULT_SHUTDOWN_TIMEOUT_SECONDS
}

/** The body of the detach confirmation, the agent's name marked for the web
 * to chip. dux-core's `detach_confirm_prose` builds the same segments for the
 * terminal UI, and both are pinned by
 * `crates/dux-core/tests/fixtures/prose_cross_language.json`.
 *
 * `liveTabs` is how many of the agent's tabs are running. Past one it earns a
 * sentence: the menu names one agent and the act ends several conversations,
 * which is a scope the user has to be told about before agreeing. */
export function detachConfirmProse(
  label: string,
  graceSeconds: number,
  liveTabs: number,
): Prose {
  const body: Prose = [
    "dux will ask ",
    quotedChip(label),
    ` to shut down and wait up to ${graceSeconds} seconds ` +
      `for it to exit before forcing it. The agent stays in the list as Detached, ` +
      `and you can resume it later. Anything the agent is doing right now is ` +
      `interrupted.`,
  ]
  return liveTabs > 1
    ? [...body, ` All ${liveTabs} running tabs stop together.`]
    : body
}

/** The body of the Task Manager's force-stop confirmation, the immediate
 * sibling of `detachConfirmProse`. Separate copy rather than a parameter on the
 * polite one, because the two promise DIFFERENT things: the Task Manager is the
 * panic surface and ends the processes at once, while the agent menu's Detach
 * agent asks first and waits out the configured grace. No number appears here,
 * because there is no wait to quote. */
export function forceStopConfirmProse(label: string): Prose {
  return [
    "dux will stop ",
    quotedChip(label),
    ` immediately, with no shutdown wait. Anything it ` +
      `is doing right now is lost. The agent stays in the list as Detached, and ` +
      `you can resume it later.`,
  ]
}

/** Whether the agent has anything to detach: a detach asks a PROCESS to go, so
 * with none running there is nothing to ask. The menu item is absent rather
 * than disabled in that case, because a disabled row promises an action that is
 * not waiting on the user.
 *
 * The answer is the SERVER's (`SessionView::detachable`, from the engine's own
 * `live_tab_ids`), read here rather than re-derived from the tab rows: the
 * engine's teardown and the terminal UI's palette gate ask that same oracle, and
 * a third implementation here is a third chance to disagree about one agent. An
 * older server that does not send the field falls back to the tab scan, which is
 * what the field is computed from anyway. */
export function agentIsDetachable(session: {
  detachable?: boolean
  tabs: { has_live_process: boolean }[]
}): boolean {
  return session.detachable ?? session.tabs.some((tab) => tab.has_live_process)
}

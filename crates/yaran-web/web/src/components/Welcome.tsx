import { useState } from "react"
import type { ReactNode } from "react"

import { useYaran } from "@/lib/store"

// The Yaran welcome screen mirrors the TUI's idle agent pane. The branching-Y
// mark comes from the canonical artwork and the wordmark matches the shared
// 39-column, five-line ASCII_LOGO in crates/yaran-tui/src/app/render.rs.
const TEXT_LOGO = [
  "██   ██  █████  ██████   █████  ██   ██",
  " ██ ██  ██   ██ ██   ██ ██   ██ ███  ██",
  "  ███   ███████ ██████  ███████ ██ █ ██",
  "  ███   ██   ██ ██  ██  ██   ██ ██  ███",
  "  ███   ██   ██ ██   ██ ██   ██ ██   ██",
].join("\n")

// Tips come from the server's ViewModel. The single source of truth is crates/yaran-core/src/welcome.rs (WELCOME_TIPS). Add new tips THERE, with both surface variants.

// Render a tip, highlighting `backticked` spans in the foreground accent
// (the backticks themselves are not shown), the same convention as the TUI.
function TipText({ tip }: { tip: string }) {
  // No platform localization pass here: the web has no command palette and no
  // keyboard shortcuts of its own, so web tips name affordances (the cog menu,
  // buttons, the ⋯ menu) rather than keys. See the `web` field's doc in
  // crates/yaran-core/src/welcome.rs.
  const parts = tip.split("`")
  return (
    <>
      {parts.map((part, i) =>
        i % 2 === 1 ? (
          <span key={i} className="font-medium text-foreground">
            {part}
          </span>
        ) : (
          <span key={i}>{part}</span>
        ),
      )}
    </>
  )
}

// `action` is an optional slot under the tip, for the surfaces where the idle
// screen is not merely idle: a dormant tab shows this same screen and needs the
// one act that starts it. A slot rather than a second component, because the
// logo, the wordmark and the tip rotation are the thing that must not be copied.
//
// With an action the pane scrolls instead of clipping: the idle screen alone is
// decoration a short viewport may crop harmlessly, but a button has to be
// reachable, and on a phone in landscape the whole stack is taller than the
// pane. `my-auto` inside a scrolling column gives both behaviours from one
// rule: content that fits is centred, content that does not scrolls.
export function Welcome({ action }: { action?: ReactNode } = {}) {
  const tips = useYaran().bootstrap?.welcome_tips ?? []
  // Pick a stable random fraction once per visit to the welcome screen (the
  // component remounts whenever the center pane returns to the idle state).
  // Storing the fraction (not an index) keeps the choice stable across
  // ViewModel re-renders while still working when tips arrive AFTER mount.
  const [tipFraction] = useState(() => Math.random())
  const tip =
    tips.length > 0
      ? tips[Math.floor(tipFraction * tips.length) % tips.length]
      : null

  return (
    <div
      className={
        action
          ? "flex h-full min-h-0 w-full select-none flex-col overflow-y-auto"
          : "flex h-full w-full select-none flex-col items-center justify-center overflow-hidden"
      }
    >
      <div
        className={
          action
            ? "my-auto flex flex-col items-center gap-3 px-4 py-6"
            : "flex flex-col items-center gap-3"
        }
      >
        <img
          src="/yaran-logo.svg"
          alt=""
          aria-hidden
          className="size-28 object-contain"
        />
        <pre
          aria-label="yaran"
          className="font-blocks text-[11px] leading-[1.15] text-muted-foreground"
        >
          {TEXT_LOGO}
        </pre>
        {tip && (
          <p className="mt-6 max-w-md px-6 text-center text-sm text-muted-foreground">
            <TipText tip={tip} />
          </p>
        )}
        {action}
      </div>
    </div>
  )
}

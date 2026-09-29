import { cn } from "@/lib/utils"

// The one chip for a name inside prose: a branch, a path, a file, a command, an
// agent, project or terminal name, a pull request number. The chip is what
// tells the name apart from the sentence around it, so callers drop the quotes
// they used to wrap it in. Sized relative to its surroundings, so it sits in a
// dialog title as comfortably as in body text. It wraps anywhere, because a path
// has no spaces to break at and a phone must never scroll sideways, and it keeps
// runs of spaces, because in a command they are part of the value. To a screen
// reader it is plain inline text.
function InlineCode({ className, ...props }: React.ComponentProps<"code">) {
  return (
    <code
      data-slot="inline-code"
      className={cn(
        "rounded bg-muted px-1.5 py-0.5 font-mono text-[0.85em] box-decoration-clone whitespace-pre-wrap wrap-anywhere",
        className,
      )}
      {...props}
    />
  )
}

export { InlineCode }

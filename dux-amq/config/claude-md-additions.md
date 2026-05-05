## Multi-agent environment (AMQ + dux)

This VM runs multiple Claude/Codex/Gemini agents in parallel via **dux** (terminal worktree orchestrator). Peer agents are reachable via the **AMQ** file-based message bus — use the `amq-cli` skill or call `amq` shell commands directly.

- Queue root: `/data/state/amq` (shared across all panes/agents on this VM)
- Your identity (`AM_ME`) is auto-set by the dux wrapper to the sanitised lowercased basename of the worktree directory (typically the original branch name; stable across branch renames inside the worktree)
- Peers can be listed with `amq who`; check your inbox with `amq list`
- The full command surface is in the `amq-cli` skill — load it when coordinating with another agent
- If a request implies coordination ("ask bob to review", "wait for codex's analysis", "tell the other agent..."), default to AMQ rather than asking the user to be a manual relay
- Persistent state lives on `/data/state/`: `~/.claude` and `~/.agents` are symlinks to it, so VM preemption (this is a spot VM) doesn't lose chat history or AMQ messages

## Multi-agent environment (AMQ + dux)

This VM runs multiple Claude/Codex/Gemini agents in parallel via **dux** (terminal worktree orchestrator). Peer agents are reachable through the Dux router: use `dux peer send <handle> "<message>"` for normal agent-to-agent communication.

- Queue root: `$AMQ_GLOBAL_ROOT` (shared across all panes/agents on this machine; Linux VM default: `/data/state/amq`)
- Your identity (`AM_ME`) is auto-set by the dux wrapper to the sanitised lowercased basename of the worktree directory (typically the original branch name; stable across branch renames inside the worktree)
- Peers can be listed with `dux peer list`; AMQ registry state can be refreshed with `dux peer sync-amq`
- Do not choose transports yourself for normal routing. Do not call `amq` or Claude Peers directly unless the user explicitly asks for low-level diagnostics. Dux uses Claude Peers for Claude targets and AMQ for non-Claude targets.
- If a request implies coordination ("ask bob to review", "wait for codex's analysis", "tell the other agent..."), default to `dux peer send` rather than asking the user to be a manual relay
- Persistent state lives under `$STATE_ROOT` (Linux VM default: `/data/state`); on persistent-disk VM setups, `~/.claude` and `~/.agents` are symlinked there so VM preemption doesn't lose chat history or AMQ messages
- If a wake notification you receive ends with an `[Orchestrator note]` paragraph asking you to emit `[task-done]` at the end of your reply, do exactly that — type the literal token `[task-done]` (lowercase, square brackets, no whitespace inside) as the final visible content of your reply. The orchestrator watches for that sentinel to clean up your context after a Worker-mode task; without it the next task you receive will be appended to the previous one's history

# Plan: shared main-workspace mode for dux agents
_Locked via grill — by Claude + Siavash. Revised after Codex review rounds 1–4._

## Goal

Let dux agents run directly in a project's **main workspace** (`Project.path`) instead of each getting its own `git worktree` clone. New default for freshly-configured installs; existing installs keep worktree isolation until they opt in (§C). Motivation: usually one agent per codebase, and per-agent worktrees make files drift out of sync.

**Scoped invariants (honest):**
1. dux's **automatic lifecycle/workspace-management** operations (worktree create/remove, session cleanup, purge, reset, orphan cleanup) never mutate or delete the user's real repo. *User-initiated* actions (stage, discard, pull, commit) still mutate the checkout by design — that is what an agent workspace is for.
2. Every place that used a session's *path* as identity now uses a durable per-session key, so identity, messaging, resume, purge, branch, and callbacks stay correct when sessions share a directory.
3. The multi-writer warning reflects **current-store** visibility only; agents under another `DUX_HOME` or unmanaged processes sharing the checkout are **undetectable** and documented as such.

## Core defect — path as identity (rounds 1–2)

`session.worktree_path` is a de-facto identity key across Peers routing (`peer.rs:554`), AMQ handle/env (`peer.rs:78,603`), inject match (`inject_runtime.rs:176`), purge target (`purge.rs:238`) + provider-dir encoding (`purge.rs:278`), commit-msg callback (`input.rs:1055`), refs watcher (`workers.rs:1191`), branch/PR metadata (`workers.rs:1339`), and three deletion paths. **Fix:** persisted immutable `agent_handle` + `session.id` are identity; path-keyed lookups survive only for workspace-wide state (current branch, changed files), fanned to all sessions on that path.

## Verified facts (code + research)

- Wrappers `flock` **`meta/config.lock`** (stable), not `config.json`, and run **unlocked if `flock` is absent** (`claude-amq:189`); Rust reconciliation currently takes no lock (`peer.rs:673`). AMQ root is shared across panes/agents (and potentially `DUX_HOME`s) → `agents/<handle>` is a **global** namespace; two stores minting `alice` collide physically. Peer loading drops exited sessions before reconciliation (`peer.rs:299`). Wake daemon is disowned via `setsid` and its stale check keys on app-wide `DUX_PID` (`claude-amq:374`, `dux-amq-inject-bridge:179`).
- `storage.rs` delete physically removes the row (`storage.rs:411`); migration runner is SQL-only and bumps `user_version` in-tx (`storage.rs:54,117`). `ALTER TABLE ADD COLUMN` can't add `NOT NULL`/`CHECK` and a unique index permits multiple NULLs — enforcing the contract needs a **table rebuild**.
- Provider wrapper claims the handle before `CreateAgentReady` persists the row (`workers.rs:1833` then `:19`); a `Spawning` state exists to persist-first.
- Project **registration** default-checks-out the remote default branch (`sessions.rs:111`, `input.rs:4568`). `session_prs` stores no branch; PR sync prefers the stored PR (`workers.rs:2512`). Startup auto-spawns non-stale path-existing sessions (`mod.rs:1760`).
- `reset_agent_data` warns-and-continues on DB load failure, then wipes `worktrees_root` + DB (`cli.rs:504,524`). `StatusLine` holds one replaceable message (`statusline.rs:11`). Untracked-dir discard legitimately `remove_dir_all`s inside a project (`git.rs:736`). Hard-purge advertises GDPR erasure (`README.md:315`).

## Approach

### A. Schema, identity, soft-delete
1. **Migration 0005 = atomic table rebuild.** In one transaction: create `agent_sessions_new` with all existing columns plus `shared_workspace BOOLEAN NOT NULL DEFAULT 0`, `agent_handle TEXT NOT NULL UNIQUE CHECK(length ≤ N and matches [a-z0-9_-]+)`, and `deleted_at` (soft-delete); backfill `agent_handle` in Rust (normalized basename derivation, deterministic `-2/-3` suffixing by stable row order, **globally** deconflicted per §B); recreate all indexes and the `session_prs` foreign keys; `DROP` old, `RENAME` new; set `user_version = 5`. Crash-atomic — no version-5-with-nulls window.
2. **Immutable, fail-closed handles.** Normalization only at migration/creation. DB boundary enforces NOT NULL/UNIQUE/CHECK. Loads **fail closed** on invalid/duplicate (corruption), never silently repair.
3. **Soft-delete + tombstone.** Ordinary session delete sets `deleted_at`, hides the row from active UI/routing/auto-resume queries, and **retains the full row** (provider path, handle, `store_id`, `session_id`) so a later hard purge can still erase everything. Physical row removal happens **only** after hard purge succeeds.
4. **Single accessor** `session.agent_handle()` through AMQ export, `DUX_AMQ_HANDLE`, sync, inject match, purge target. No path-derived handles.

### B. AMQ ownership, global handles, locking, wake (shared root aware)
5. **Global handle reservation under the shared lock.** Handles are unique across the **whole** shared AMQ root, not per-store. Reservation is atomic: acquire `flock` on `meta/config.lock` → verify the physical `agents/<handle>` key is free or owned by this `{store_id, session_id}` → reserve → release. Backfill/creation that hits a foreign-owned handle deconflicts with a suffix (immutable thereafter). Explicit migration policy for pre-existing foreign collisions.
6. **Persist-first ownership (use `Spawning`).** Persist UUID + reserved handle (soft-row) and reserve the AMQ handle under lock **before** spawning the provider; then spawn. Every spawn/persist failure leaves a recoverable row — never an owner marker/inbox with no row.
7. **Ownership marker `{store_id, session_id}`; own-store pruning only.** The wrapper records `{store_id, session_id}` atomically with registration. Reconciliation considers only its own store's rows, matched against **all unfiltered** rows (not the exited-filtered peer set); foreign-store and standalone-wrapper registrations are **never pruned**. `store_id` is persisted in durable `DUX_HOME` metadata (a file), so it survives and is recoverable.
8. **Mandatory shared lock everywhere.** All three provider wrappers **and** Rust reconciliation must lock `meta/config.lock` (via `rustix::flock`) for the complete read-modify-rename; a missing `flock` is a hard error, not a silent bypass.
9. **Delete/purge stops the wake daemon.** Record each session's disowned wake PID at spawn; before tombstoning or purging, terminate + verify that PID and remove its live registry entry under the AMQ lock, so a retained inbox is not drained after delete.
10. **Deletion vs purge:** tombstone (soft-row + retained inbox + reserved handle) on ordinary delete; **hard purge is the only op** that removes the inbox dir, frees the global handle (after exact-owner verification), and physically deletes the row.

### C. Config + consent
11. **Presence sentinel.** `[workspace] default_mode` (global) + `ProjectConfig.workspace_mode` (per-project). Preserve section presence: **absent ⇒ `worktree`** (legacy consent); fresh render writes `shared`. Shared-mode badge + real path in the create modal. **Eligibility (shared project not under a dux-managed root) is validated at config load, create, and reconnect** — not only registration.

### D. Messaging routing
12. **Shared → always AMQ.** If **either** resolved endpoint (sender or target) is shared, route via AMQ using `agent_handle`; **reject explicit `--transport claude-peers`** when either endpoint is shared. Worktree↔worktree keeps Peers-preferred. Test both directions.
13. **Sender identity + no ambiguous cwd.** Export `DUX_SESSION_ID` + `agent_handle` to companion terminals (`sessions.rs:610`); `session_for_cwd` rejects ambiguity and requires `--from <handle>`.

### E. Lifecycle & branch
14. **Create-agent (keep modal).** `CreateAgentRequest::SharedWorkspace`; modal settings apply; relabel branch→handle, autofill, skip branch checks, dir = `Project.path`, `shared_workspace = true`, `owns_worktree = owns_branch = false`. **Fork always isolates** (worktree) even inside a shared-default project.
15. **Registration honors mode.** Resolve effective mode at registration; **prohibit the default-branch checkout** (`sessions.rs:111`, `input.rs:4568`) for shared projects. Bar shared projects under any dux-managed root.
16. **Branch + PR = live workspace state.** For shared sessions ignore stored `branch_name` and the per-session known-PR shortcut (`workers.rs:2512`); query live `HEAD` **once per canonical path**, fan the branch + a single PR discovery to every session on that path, and **skip PR discovery while detached**.
17. **No startup auto-spawn** for shared sessions; manual reconnect through the §F gate. Auto-resume disabled for shared (fresh only).
18. **Worktree-link gated on owned-worktree presence** — not the project's default mode. Every call site (project-add `sessions.rs:180`, startup `mod.rs:1540`, create-agent `workers.rs:47`) creates the symlink / `.git/info/exclude` mutation only when the project actually has (or is creating) a worktree-mode session. So a shared-default project that spawns an isolated Fork still gets the link; a shared-only project stays byte-untouched.
19. **Conflict-detach** no-op for shared; handle immutable; rename = display title only; branch-rename hidden for shared.

### F. Deletion safety (scoped) + abort-on-incomplete
20. **Protected-workspace guard at whole-worktree/root entry points only** (`remove_worktree`, `reset_agent_data`'s `worktrees_root` wipe, session-delete/purge worktree steps): refuse when the canonical target is an ancestor/descendant of any registered project path. **Not** applied to contained-file ops (untracked-dir discard `git.rs:736`, mirroring).
21. **Abort before any mutation** in `reset_agent_data` and the orphan cleaner whenever config, sessions, tombstones, `store_id`, or the protected-path inventory cannot load completely — an invalid handle (fail-closed) must not fall through to the warn-and-delete path. Reset also purges every exactly-owned AMQ dir (or retains metadata) before deleting the DB, so tombstones aren't stranded.
22. **Honest shared hard-purge (GDPR).** Per-session purge cannot delete the shared provider dir without erasing siblings, so it reports provider history as **incomplete/error** and retains the (soft-deleted) row unless the operator accepts residual data or confirms a **workspace-wide** provider-history purge. Never a false "erased."

### G. Concurrency UI
23. Second live writer → confirmation; the warning renders as a **separately-derived header/sidebar segment** (not the replaceable `StatusLine`), labeled **current-store** visibility (foreign-`DUX_HOME`/unmanaged writers are undetectable — documented). Full Git-state cross-talk documented in `SECURITY.md` + threat model.

### H. Migration cleanup (opt-in, safe)
24. Cleaner admits **only canonical descendants of `worktrees_root` that are git-registered worktrees with no session row**, excludes the main worktree, shows dirty/untracked status per item, per-item confirmation, preserves the branch by default, never automatic, aborts on incomplete inventory (§F21).

### I. Docs & proof matrix
25. Update `README.md`, threat model, header summaries + module trees.
26. **Unit tests — two-sessions/one-path for every formerly path-keyed site, plus:** interrupted-migration durability + table-rebuild constraint enforcement (NOT NULL/UNIQUE/CHECK, FK preservation); global handle collision across two stores + foreign-registration non-pruning + exact-owner purge; persist-first spawn failure leaves a recoverable row; concurrent wrapper+Rust claim under `meta/config.lock`; wake-PID termination on delete; config absent/shared/worktree/fresh; routing AMQ-fallback + `--transport claude-peers` rejection **both directions**; ambiguous companion sender → `--from`; startup skip; commit-msg callback + refs fan-out + exact inbound injection by handle; live-HEAD branch + single PR fan-out incl. detached; soft-delete-then-hard-purge erases everything; reset abort-on-incomplete + owned-AMQ purge; deletion overlap guard at scoped entry points; shared create/delete leaves the repo untouched; cleaner restricted to `worktrees_root`.

## Key decisions & tradeoffs

- **Invariant scoped to automatic operations; user edits still mutate the checkout; badge is current-store only** — honest about what "iron-clad" can mean here.
- **Path is never identity; handles are globally unique + immutable + fail-closed; table-rebuild migration** — correct under a shared AMQ root.
- **Persist-first + `Spawning`, global reservation under mandatory `meta/config.lock`, own-store pruning, wake-PID kill on delete** — no orphaned owners, no cross-store clobber, no zombie drainers.
- **Soft-delete/tombstone; hard purge is the only free-and-erase** — later GDPR erasure stays possible; contradiction removed.
- **Shared → always AMQ (either endpoint); branch + PR from live HEAD; link gated on owned-worktree presence; eligibility re-checked at load/create/reconnect.**
- **Deletion guard scoped to worktree/root entry points; reset aborts on incomplete load and purges owned AMQ first; honest GDPR purge.**

## Risks / open questions

- **Scope reality:** genuinely large and cross-cutting — schema rebuild, wrapper protocol + global handle namespacing, wake-daemon lifecycle, soft-delete, PR identity, reset atomicity. Must land in sequenced phases behind the config default (dark until opted in): migration/identity → ownership/locking/wake → routing → lifecycle/branch → deletion/purge → UI/cleaner → docs.
- **Global handle collisions with pre-existing foreign registrations** need a one-time migration policy; document the deconflict behavior.
- **Foreign-`DUX_HOME` writers are undetectable** — accepted and documented, not solved.
- **AMQ-only routing for shared Claude agents** changes transport in the multi-agent case; observable via reject + badge; smoke-test two Claude agents in one repo.

## Implementation refinements (round 5 — non-blocking, fold in during build)

- Pick a concrete `agent_handle` length bound `N` (e.g. 64) and run `PRAGMA foreign_key_check` around the table rebuild to prove the `session_prs` FKs survived.
- Upgrade a legacy path-based AMQ marker (`claude-amq:222`) to `{store_id, session_id}` **only** when it maps unambiguously to one local row; otherwise preserve it as foreign and suffix the new handle.
- On startup, convert an interrupted PTY-less `Spawning` row into a **visible retryable** state while retaining its reservation (don't strand or auto-spawn it).
- Export `DUX_STORE_ID` to the PTY env; make the recorded wake PID **optional** and surface reduced delivery observability for providers launched without an AMQ wrapper.
- Reset follows the owned-AMQ-purge branch before deleting the DB; soft-deleted purge targets resolve by **UUID/handle**, rejecting ambiguous branch aliases.
- Document that a **workspace-wide** provider-history purge also removes **non-Dux** conversations stored under that workspace path (they share the provider dir).

## Out of scope

- Coordination/locking for concurrent agent *edits* (only the AMQ-registry lock is in scope).
- Changing the Claude Peers broker / broker-side session id.
- Persisting provider conversation IDs for precise shared resume (future).
- Cross-`DUX_HOME` writer detection.
- Auto-migrating/auto-deleting worktrees (cleaner is opt-in, `worktrees_root`-only).
- Multi-branch-in-one-directory; rewriting the worktree path.

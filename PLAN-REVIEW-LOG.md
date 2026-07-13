# Plan Review Log: shared main-workspace mode for dux agents
Act 1 (grill) complete — plan locked with the user. MAX_ROUNDS=5.

Grill resolved: motivation (agents' files drift out of sync; usually one agent per codebase), shared-as-default with worktree opt-in, use-whatever-branch-is-checked-out, allow multiple agents to share, per-session AM_ME identity (verified dux-only fix), delete/purge never touch the workspace, leave-existing + opt-in worktree cleanup. Two integration landmines verified in code (AMQ collision check is handle-keyed → OK; Claude Peers ids are per-PID random → OK) and one safety addition surfaced (purge is a second deletion path that must be gated).

## Round 1 — Codex
(gpt-5.6-sol, reasoning max, codex-cli 0.144.1, thread 019f57b6-88bf-70e2-9572-ed7a05a016ce)

The plan is not safe to implement yet. Its central identity assumptions are contradicted by current code.

1. **Claude Peers routing is ambiguous.** The plan calls `cwd` display-only and the router external ([PLAN.md:14](/Users/siavash/Projects/dux-amq-setup/PLAN.md:14)), but `dux peer` maps a session to the first broker peer with the same canonical cwd ([peer.rs:554](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:554)); shared sessions can therefore target the wrong process.  
Fix: Register/expose `DUX_SESSION_ID` through Claude Peers and match on it, or reject/fallback to AMQ whenever a shared cwd is ambiguous.

2. **Exporting `AM_ME` alone does not fix AMQ.** `DUX_AMQ_HANDLE`, sync, target resolution, and purge still use the worktree basename ([peer.rs:78](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:78), [peer.rs:603](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:603)), while inbound injection matches only basename/branch/id ([inject_runtime.rs:176](/Users/siavash/Projects/dux-amq-setup/src/app/inject_runtime.rs:176)); generated shared handles will not route consistently.  
Fix: Persist one normalized immutable `agent_handle` and make AMQ export, collision checks, sync, injection, peer routing, and purge use that single accessor.

3. **A “renamable handle” breaks live identity.** Environment variables cannot change in an already-running PTY, and the current rename flow can also rename the actual Git branch ([mod.rs:2794](/Users/siavash/Projects/dux-amq-setup/src/app/mod.rs:2794)), violating the promise that Dux never changes branches in shared mode.  
Fix: Keep `agent_handle` immutable, allow only display-title renames, and hide/reject branch rename for shared sessions.

4. **Provider resume will cross-wire conversations.** Dux stores only “provider was started” and invokes cwd-scoped/latest commands such as Claude `--continue` and Codex `resume --last` ([config.rs:2335](/Users/siavash/Projects/dux-amq-setup/src/config.rs:2335), [sessions.rs:560](/Users/siavash/Projects/dux-amq-setup/src/app/sessions.rs:560)); multiple sessions in one cwd have no durable provider-session discriminator.  
Fix: Persist provider-specific conversation IDs where supported, otherwise disable automatic resume for shared sessions and explicitly launch fresh/manual-resume sessions.

5. **Per-session purge would delete every sibling’s provider history.** Provider directories are derived solely from encoded `worktree_path` ([purge.rs:278](/Users/siavash/Projects/dux-amq-setup/src/purge.rs:278)), so the plan’s promise that provider-history deletion “still applies” would recursively remove all conversations for that shared repo.  
Fix: Delete only provider records owned by the target session; if ownership cannot be resolved, refuse that step and delete the common directory only during an explicitly confirmed all-sharing-sessions purge.

6. **Purge target selection becomes nondeterministic.** `build_plan` uses the first UUID-or-branch match ([purge.rs:238](/Users/siavash/Projects/dux-amq-setup/src/purge.rs:238)), while shared sessions normally have the same current branch.  
Fix: Resolve purge by UUID or immutable handle and reject branch targets that match more than one session.

7. **Path-keyed callbacks can select the wrong session.** Commit-message generation recovers the originating session using the first matching path and may use another agent’s provider/settings ([input.rs:1055](/Users/siavash/Projects/dux-amq-setup/src/app/input.rs:1055)); the refs watcher similarly stores only one session per path ([workers.rs:1191](/Users/siavash/Projects/dux-amq-setup/src/app/workers.rs:1191)).  
Fix: Carry `session_id` through every session-specific worker event and use path-to-many mappings only for genuinely workspace-wide events.

8. **Creation and fork semantics are incomplete.** The existing “name” modal also selects provider, context mode, permissions, system prompt, and watch settings ([sessions.rs:275](/Users/siavash/Projects/dux-amq-setup/src/app/sessions.rs:275)), while `ForkSession` explicitly creates and mirrors a worktree ([workers.rs:1683](/Users/siavash/Projects/dux-amq-setup/src/app/workers.rs:1683)); simply dropping the prompt leaves both behaviors undefined.  
Fix: Keep the modal but relabel/autofill the handle and skip branch checks in shared mode; specify that Fork always creates an isolated worktree session.

9. **Existing users are silently moved across a security boundary.** A missing new config section will receive `Config::default()`, so old configs would begin creating shared sessions without consent, despite previously guaranteeing isolation.  
Fix: Add an explicit config-schema migration preserving `worktree` for legacy configs while fresh configs default to `shared`, plus a visible shared-mode badge/path before launch.

10. **The deletion inventory is incomplete and the Boolean is not defense-in-depth.** `config reset --all` also iterates session paths and recursively removes contained directories/branches without checking the proposed flag ([cli.rs:504](/Users/siavash/Projects/dux-amq-setup/src/cli.rs:504)); a misclassified row or main repo under the managed root defeats the “iron-clad” claim.  
Fix: Audit every `remove_worktree`/`remove_dir_all` caller and centrally reject canonical `target == project_path` in addition to checking `shared_workspace`.

11. **The accepted concurrency risk is understated.** Shared agents do not merely contend on `index.lock`; they share the branch, index, staging area, commits, discard operations, and branch switches, so one agent can silently commit or discard another’s work ([PLAN.md:39](/Users/siavash/Projects/dux-amq-setup/PLAN.md:39)).  
Fix: Require confirmation when starting a second shared writer, show a persistent warning, and document the full Git-state cross-talk in both `SECURITY.md` and the long-form threat model as required by [CLAUDE.md:143](/Users/siavash/Projects/dux-amq-setup/CLAUDE.md:143).

12. **The one-time unused-worktree cleaner is speculative scope and another destructive path.** Switching defaults makes no existing worktree unused, and current delete/purge flows already provide explicit cleanup.  
Fix: Remove the cleanup action from this change and add it later only with a concrete orphan definition and demonstrated need.

VERDICT: REVISE
### Claude's response (round 1)

Codex found the plan's central weakness: session PATH is used as a de-facto identity key in ~8 places, so sharing the path breaks routing, purge, resume, and callbacks. I verified the load-bearing claims in code before accepting:
- #1 peer.rs:554 claude_peer_id_for_session matches session→peer by worktree_path == peer.cwd — the router IS in-repo (my "external" assumption was wrong). ACCEPTED.
- #5 purge.rs:278 provider dirs encoded from worktree_path → shared purge nukes all siblings' history. ACCEPTED.
- #10 cli.rs:504 reset_agent_data is a THIRD deletion path over all session paths. ACCEPTED.
- #2 handle derived from path in peer.rs/inject, not just the wrapper. ACCEPTED.

Accepted in full — 1,2,3,4,5,6,7,8,9,10,11. Plan rewritten around a persisted immutable agent_handle + session.id as the identity key everywhere (never path), with:
- Single handle accessor threaded through AMQ export/sync/inject-match/peer-routing/purge (#2).
- Peer routing: single-peer-in-cwd uses Peers; multiple → fall back to AMQ via distinct handle (no broker change) (#1).
- Auto-resume DISABLED for shared sessions to avoid cwd/latest cross-wiring; manual resume kept (#4).
- session_id threaded through commit-msg + session-specific worker events; refs/changed-files become path→many fan-out (#7).
- Three deletion paths guarded + central "canonical target == project_path" reject as defense-in-depth (#10); purge excludes shared provider dir + worktree, resolves target by UUID/handle, rejects ambiguous branch targets (#5,#6).
- Legacy configs resolve to worktree (consent preserved); only fresh configs default shared; visible shared-mode badge (#9).
- Keep the create modal (provider/context/perms/etc), autofill handle, skip branch checks; Fork always isolates (#8).
- Handle immutable, rename = display-title only, branch-rename hidden in shared mode (#3).
- Concurrency: confirm on 2nd writer + persistent badge + full git cross-talk in SECURITY.md/threat-model (#11).

PARTIAL on #12 (worktree cleaner): the user explicitly requested a one-time cleanup during the grill, so I kept it but adopted Codex's "concrete orphan definition" — narrowed to worktree dirs under worktrees_root with NO session row, opt-in and confirmed only. Rejecting outright would override an explicit user decision; narrowing satisfies the real critique (the mode switch itself orphans nothing).

## Round 2 — Codex

The revision fixes most round-one findings: immutable identity, display-only rename, isolated forks, session-ID callbacks, consent-preserving intent, full deletion inventory, resume policy, and concurrency disclosure. Material gaps remain:

1. **Claude Peers fallback still cannot reliably identify the target.** Transport is chosen before peers are listed, and `claude_peer_id_for_session` only returns an ID ([peer.rs:101](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:101)); with two stored shared sessions but only the wrong sibling live, “exactly one peer at this cwd” still misroutes.  
Fix: Auto-route every shared-workspace sender or target through AMQ and reject explicit `--transport claude-peers`; optimize only after a durable peer/session mapping exists.

2. **`agent_handle` lacks a collision and validation contract.** AMQ needs the handle alone to be unique, it becomes a filesystem path component ([peer.rs:662](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:662)), and the SQL-only migration runner cannot perform the Rust normalizer/backfill policy by itself ([storage.rs:117](/Users/siavash/Projects/dux-amq-setup/src/storage.rs:117)).  
Fix: Specify a transactional Rust backfill with nonempty canonical validation, deterministic duplicate handling, a unique index, validation on load, and creation-time collision rejection.

3. **AMQ ownership remains path-based despite the new identity.** The wrapper accepts the same handle when the cwd matches ([claude-amq:239](/Users/siavash/Projects/dux-amq-setup/dux-amq/wrappers/claude-amq:239)), while registry pruning considers an entry live as long as that path exists ([peer.rs:692](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:692)); a deleted shared session therefore leaves a reusable inbox containing old messages.  
Fix: Add a Dux session-ID ownership marker, prune by missing session ID, and reject handle reuse until the old AMQ directory is explicitly purged.

4. **Sender inference still uses cwd as identity.** `session_for_cwd` picks an arbitrary equal-depth session ([peer.rs:352](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:352)), and companion terminals currently receive no `DUX_SESSION_ID` or handle environment ([sessions.rs:610](/Users/siavash/Projects/dux-amq-setup/src/app/sessions.rs:610)).  
Fix: Export session identity to companion terminals and make cwd fallback reject ambiguity with an instruction to pass `--from`.

5. **Startup auto-resume bypasses the second-writer confirmation.** Returning false from `should_resume_session` only selects fresh arguments; `auto_resume_all_sessions` still spawns every shared candidate ([mod.rs:1760](/Users/siavash/Projects/dux-amq-setup/src/app/mod.rs:1760)), creating multiple fresh conversations without consent.  
Fix: Exclude shared sessions from startup auto-resume and require manual reconnect through the concurrency confirmation gate.

6. **Shared hard-purge becomes silently incomplete.** Omitting provider history avoids sibling loss, but then deleting the SQLite row reports success while transcripts remain, contradicting the documented GDPR erasure contract ([README.md:315](/Users/siavash/Projects/dux-amq-setup/README.md:315)) and losing the recovery identity.  
Fix: Report shared provider history as an explicit incomplete/error item and retain the row unless the operator explicitly accepts residual data or confirms a workspace-wide provider-history purge.

7. **`target == project_path` is not a sufficient deletion guard.** `reset_agent_data` later removes all of `worktrees_root` ([cli.rs:524](/Users/siavash/Projects/dux-amq-setup/src/cli.rs:524)); equality misses a protected repo nested below that target and a corrupted target nested inside the repo.  
Fix: Reject any canonical ancestor/descendant overlap between deletion targets and protected project paths, or forbid shared projects anywhere under Dux-managed roots.

8. **The legacy-config migration still needs a missingness sentinel.** Deserialization occurs before `migrate_config`, and struct-level `#[serde(default)]` can erase whether `[workspace]` was absent ([config.rs:30](/Users/siavash/Projects/dux-amq-setup/src/config.rs:30), [config.rs:946](/Users/siavash/Projects/dux-amq-setup/src/config.rs:946)).  
Fix: Preserve section presence via an `Option`/field-specific legacy default or inspect the TOML document before deserialization, then test absent, explicit-shared, explicit-worktree, and fresh-render cases.

9. **The worktree-link mutation is guarded at the wrong place.** Project registration and startup unconditionally create the repo symlink and modify `.git/info/exclude` ([sessions.rs:180](/Users/siavash/Projects/dux-amq-setup/src/app/sessions.rs:180), [mod.rs:1540](/Users/siavash/Projects/dux-amq-setup/src/app/mod.rs:1540)), so skipping only the create-agent call does not keep a fresh shared repo untouched.  
Fix: Gate every link call and create it only when the project actually has or is creating a worktree-mode session.

10. **The orphan cleaner can erase crash residue with uncommitted work.** “No session row” includes a worktree created immediately before a crash, and `remove_worktree` uses force removal.  
Fix: Admit only Git-registered worktrees, display dirty/untracked status, require per-item confirmation, and preserve the branch by default.

11. **The plan still lacks the required proof matrix.** Only a peer smoke test is mentioned despite the repository requiring unit tests for every change ([CLAUDE.md:41](/Users/siavash/Projects/dux-amq-setup/CLAUDE.md:41)).  
Fix: Add focused tests for migration collisions/invalid handles, legacy versus fresh config, ambiguous peers and companion senders, startup skipping, incomplete purge, overlap deletion guards, and shared create/delete behavior.

VERDICT: REVISE
### Claude's response (round 2)

All 11 accepted; verified the three most design-changing in code first:
- #5 mod.rs:1760 auto_resume_all_sessions spawns every non-stale path-existing session at startup → would auto-spawn all shared sessions bypassing the concurrency gate. CONFIRMED.
- #9 sessions.rs:180 + mod.rs:1540 create the worktree symlink / mutate .git/info/exclude unconditionally on project-add and startup, not just create-agent. CONFIRMED.
- #7 cli.rs:524 reset_agent_data removes all of worktrees_root wholesale; equality guard misses nested overlaps. CONFIRMED.

Incorporated all 11:
1. Shared sessions ALWAYS route via AMQ (dropped the fragile single-peer heuristic); reject --transport claude-peers for shared targets.
2. agent_handle uniqueness/validation contract: SQL migration adds columns + a separate transactional Rust backfill; normalized non-empty, UNIQUE index, deterministic dup suffixing, validate-on-load, creation-time collision rejection.
3. AMQ ownership marker = DUX_SESSION_ID; prune by missing session id (not path); reject handle reuse until old dir purged (touches wrapper — noted as scope).
4. Export session identity to companion terminals; session_for_cwd sender fallback rejects ambiguity, requires --from.
5. Exclude shared sessions from startup auto_resume_all_sessions entirely; manual reconnect through the confirmation gate.
6. Shared hard-purge honest re GDPR: provider history reported incomplete/error, sqlite row retained unless operator accepts residual or confirms a workspace-wide purge.
7. Deletion guard = ancestor/descendant overlap with any registered project path (not equality); shared projects barred from dux-managed roots.
8. Config missingness sentinel: Option<WorkspaceConfig> / raw-TOML inspect so absent⇒worktree, fresh⇒shared; 4 test cases.
9. Gate EVERY ensure_project_worktree_link call site (project-add, startup, create-agent), worktree-mode only.
10. Orphan cleaner: git-registered worktrees only, show dirty status, per-item confirm, preserve branch by default.
11. Added a full proof matrix (unit tests per CLAUDE.md) covering migration/config/routing/startup/purge/deletion/create-delete/cleaner.

No rejections this round — every finding was a real code interaction.

## Round 3 — Codex

The round-two findings are addressed at the intent level, but the revised plan still has material gaps:

1. **Migration 0005 is not crash-atomic.** The plan commits the SQL migration before a separate Rust backfill ([PLAN.md:25](/Users/siavash/Projects/dux-amq-setup/PLAN.md:25)); the runner advances `user_version` inside the SQL transaction ([storage.rs:54](/Users/siavash/Projects/dux-amq-setup/src/storage.rs:54)), so a crash can leave a version-5 database containing null handles with no specified completion marker.

   Fix: Commit column creation, backfill, unique index, and `user_version = 5` atomically, or require an idempotent durable backfill-complete check before every load.

2. **“Repair on load” contradicts immutable identity.** The schema permits null handles, while normal loads may “reject/repair” them ([PLAN.md:26](/Users/siavash/Projects/dux-amq-setup/PLAN.md:26)); silently changing an exposed handle can orphan its inbox or collide with another identity.

   Fix: Normalize only during migration; thereafter fail closed on invalid handles and enforce non-null, unique, bounded-length handles at the database boundary.

3. **AMQ deletion semantics remain contradictory.** The plan says ordinary deletion prunes/reclaims the inbox while also reserving the handle until that directory is explicitly purged ([PLAN.md:28](/Users/siavash/Projects/dux-amq-setup/PLAN.md:28), [PLAN.md:62](/Users/siavash/Projects/dux-amq-setup/PLAN.md:62)); both cannot be true.

   Fix: Define ordinary deletion as registry removal plus a retained ownership tombstone/inbox, and make hard purge the only operation that deletes the directory and frees the handle.

4. **“Missing session ID” is not a safe global ownership test.** Current peer loading removes exited sessions before reconciliation ([peer.rs:299](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:299)), and a shared AMQ root can contain sessions from another `DUX_HOME`; either would look missing from the current session set.

   Fix: Store `{store_id, session_id}` atomically, reconcile against all unfiltered rows from the owning store, and never prune foreign or standalone-wrapper registrations.

5. **AMQ registry updates still have a lost-update race.** The wrapper locks `config.json` during read-modify-write ([claude-amq:189](/Users/siavash/Projects/dux-amq-setup/dux-amq/wrappers/claude-amq:189)), but Rust reconciliation performs the same operation without that lock ([peer.rs:673](/Users/siavash/Projects/dux-amq-setup/src/peer.rs:673)); always-AMQ routing makes this race routine.

   Fix: Make wrapper claims and Rust reconciliation use the same exclusive lock and atomic session-ID claim protocol.

6. **The overlap guard is dangerously overbroad.** Applying ancestor/descendant rejection to every `remove_dir_all` caller ([PLAN.md:45](/Users/siavash/Projects/dux-amq-setup/PLAN.md:45)) would reject legitimate deletion of an untracked directory inside a project ([git.rs:736](/Users/siavash/Projects/dux-amq-setup/src/git.rs:736)) and worktree mirroring cleanup.

   Fix: Apply a dedicated protected-workspace guard only to whole-worktree/root deletion entry points, not generic contained file operations.

7. **Shared project registration can still switch the real branch.** The plan skips branch checks only during agent creation, but project registration currently defaults to checking out the remote default branch ([sessions.rs:111](/Users/siavash/Projects/dux-amq-setup/src/app/sessions.rs:111), [input.rs:4568](/Users/siavash/Projects/dux-amq-setup/src/app/input.rs:4568)).

   Fix: Resolve effective workspace mode during project registration and prohibit the checkout path for shared-mode projects.

8. **Shared branch metadata will become stale.** `branch_name` drives PR discovery ([workers.rs:1339](/Users/siavash/Projects/dux-amq-setup/src/app/workers.rs:1339)), but branch synchronization is disabled by default ([workers.rs:1150](/Users/siavash/Projects/dux-amq-setup/src/app/workers.rs:1150)); a branch switch in the shared directory leaves every sibling associated with the old branch.

   Fix: Treat the current branch as workspace-wide live state, watch/query `HEAD`, and fan updates—including detached HEAD—to every shared session before PR operations.

9. **The orphan cleaner can classify user-created worktrees as Dux orphans.** “Git-registered with no session row” ([PLAN.md:52](/Users/siavash/Projects/dux-amq-setup/PLAN.md:52)) also describes legitimate worktrees created outside Dux.

   Fix: Restrict candidates to canonical descendants of `worktrees_root`, exclude the main worktree, and retain the existing dirty-status confirmation.

10. **The concurrency badge cannot be persistent if implemented as the existing status message.** `StatusLine` holds one replaceable message ([statusline.rs:11](/Users/siavash/Projects/dux-amq-setup/src/statusline.rs:11)), and creation immediately overwrites it ([workers.rs:66](/Users/siavash/Projects/dux-amq-setup/src/app/workers.rs:66)).

   Fix: Render the multi-writer warning as a separately derived header/sidebar segment, not a transient status message.

11. **The proof matrix still omits central identity regressions.** It does not test commit-message callback ownership, refs path-to-many fan-out, exact inbound injection, AMQ tombstone/reuse behavior, concurrent wrapper claims, or migration interruption ([PLAN.md:56](/Users/siavash/Projects/dux-amq-setup/PLAN.md:56)).

   Fix: Add two-sessions/one-path tests for every formerly path-keyed site plus crash and concurrent-claim tests.

VERDICT: REVISE
### Claude's response (round 3)

All 11 accepted — all real, several sharpened by the user's shared cross-VM $AMQ_GLOBAL_ROOT:
1. Migration atomicity: migration 0005 becomes a Rust-backed single-transaction routine (add columns + backfill + unique index + user_version=5 committed atomically) — no version-5-with-null-handles window (respects the audit's P1-15 atomicity rail).
2. Immutability: normalize only at migration/creation; DB boundary is NOT NULL + UNIQUE + bounded; loads fail closed, never silently repair an exposed handle.
3. AMQ deletion semantics de-contradicted: session delete = registry removal + ownership tombstone (inbox kept, handle reserved); hard purge = the ONLY op that deletes the dir and frees the handle.
4. Ownership keyed on {store_id, session_id}, reconciled only against the owning store's unfiltered rows; foreign/standalone registrations never pruned (critical given the shared AMQ root holds other DUX_HOMEs).
5. Rust reconciliation takes the SAME flock on config.json as the wrapper + atomic claim protocol (fixes the lost-update race that always-AMQ routing would make routine).
6. Overlap guard scoped to whole-worktree/root deletion entry points only — NOT generic remove_dir_all (my round-2 version wrongly would have blocked untracked-dir discard at git.rs:736). Good catch.
7. Project REGISTRATION resolves mode and prohibits the default-branch checkout for shared projects (registration currently git-switches the real repo).
8. Shared branch = live HEAD (handle detached), fanned to all sessions before PR discovery; stored branch_name not trusted for shared.
9. Orphan cleaner restricted to canonical descendants of worktrees_root, excludes the main worktree.
10. Concurrency warning = derived header/sidebar segment, not the single replaceable StatusLine message.
11. Proof matrix expanded to two-sessions/one-path tests for every formerly path-keyed site + crash/interrupted-migration + concurrent wrapper/Rust claim.

Also added a "scope reality" risk: this is a large cross-cutting change; sequence it and land dark behind the config default. No rejections this round.

## Round 4 — Codex

All 11 round-three findings are represented, but several new lifecycle conflicts remain:

1. **Migration 0005 still cannot enforce its claimed schema.** Adding nullable `agent_handle TEXT`, backfilling, then adding a unique index ([PLAN.md:26](/Users/siavash/Projects/dux-amq-setup/PLAN.md:26)) does not make the column `NOT NULL` or enforce normalization/length; SQLite unique indexes permit multiple nulls.

   Fix: Rebuild `agent_sessions` with final `NOT NULL`, `UNIQUE`, and `CHECK` constraints inside the same transaction, preserving all columns, indexes, and `session_prs` foreign keys.

2. **A normally deleted session becomes impossible to hard-purge.** Current deletion physically removes its row ([storage.rs:411](/Users/siavash/Projects/dux-amq-setup/src/storage.rs:411)), while purge resolves only stored sessions; the filesystem tombstone lacks the provider path and other data needed for later GDPR cleanup.

   Fix: Soft-delete the complete session row with `deleted_at`, hide it from active UI/routing queries, and physically remove it only after hard purge succeeds.

3. **`store_id` has no durable lifecycle.** The plan never says where it is stored or what `config reset --all` does with it; deleting the database ([cli.rs:524](/Users/siavash/Projects/dux-amq-setup/src/cli.rs:524)) could strand tombstones that no future store can own or purge.

   Fix: Persist `store_id` in explicit durable metadata and make reset either purge every exactly-owned AMQ directory before deleting it or retain enough metadata for later cleanup.

4. **Fail-closed validation becomes fail-open during reset.** `reset_agent_data` currently warns when the database cannot load, then continues deleting the worktree root and database ([cli.rs:504](/Users/siavash/Projects/dux-amq-setup/src/cli.rs:504)); an invalid handle will deliberately trigger that path.

   Fix: Abort reset and orphan cleanup before any mutation whenever config, session, tombstone, or protected-path inventory cannot be loaded completely.

5. **AMQ ownership is claimed after the dangerous work has started.** The provider wrapper runs and claims its handle before `CreateAgentReady` persists the row ([workers.rs:1833](/Users/siavash/Projects/dux-amq-setup/src/app/workers.rs:1833), [workers.rs:19](/Users/siavash/Projects/dux-amq-setup/src/app/workers.rs:19)); persistence failure leaves an owner marker and inbox with no recoverable row.

   Fix: Reuse the existing `Spawning` state: persist UUID/handle first, reserve the AMQ handle under lock, then spawn, leaving a recoverable row on every failure.

6. **`store_id` ownership does not solve AMQ-global handle collisions.** Two stores can each satisfy their local unique index with `alice`, but both still address `agents/alice`; immutable legacy handles can therefore become unusable when a foreign registration already exists.

   Fix: Either namespace the physical AMQ key by store ID or atomically reserve the global handle before persistence, with an explicit migration policy for pre-existing foreign collisions and exact-owner verification before purge.

7. **The lock target is misstated and currently optional.** The wrapper flocks stable `meta/config.lock`, not replaceable `config.json` ([claude-amq:189](/Users/siavash/Projects/dux-amq-setup/dux-amq/wrappers/claude-amq:189)), and silently runs unlocked when `flock` is unavailable.

   Fix: Require all three provider wrappers and Rust reconciliation to lock `meta/config.lock` for the complete read-modify-rename operation, reusing the existing Rust `rustix::flock` primitive.

8. **Ordinary deletion leaves the session’s detached wake daemon alive.** Wake is deliberately disowned ([claude-amq:374](/Users/siavash/Projects/dux-amq-setup/dux-amq/wrappers/claude-amq:374)), and its stale check uses the app-wide `DUX_PID` ([dux-amq-inject-bridge:179](/Users/siavash/Projects/dux-amq-setup/dux-amq/scripts/dux-amq-inject-bridge:179)); deleting one session while Dux remains open lets that daemon keep draining the retained inbox.

   Fix: Before tombstoning or purging, terminate and verify the handle’s recorded wake PID, then remove its live registry entry under the AMQ lock.

9. **Live `HEAD` does not invalidate persisted PR identity.** `session_prs` records no branch, and PR sync prefers the previously stored PR ([workers.rs:2512](/Users/siavash/Projects/dux-amq-setup/src/app/workers.rs:2512)); switching branches can therefore keep displaying the old PR despite correct live-branch fan-out.

   Fix: For shared workspaces, ignore per-session known-PR shortcuts, discover once from live `HEAD` per canonical path, fan out the result, and skip discovery while detached.

10. **The transport rule still has a sender-side escape.** The plan promises AMQ whenever sender or target is shared, but explicitly rejects Claude Peers only for a shared target ([PLAN.md:39](/Users/siavash/Projects/dux-amq-setup/PLAN.md:39)); a shared sender targeting a worktree session can still force ambiguous Peers routing.

   Fix: Reject explicit Claude Peers when either resolved endpoint is shared and test both directions.

11. **Project default mode is still conflated with session mode.** A shared project may create an isolated Fork ([PLAN.md:43](/Users/siavash/Projects/dux-amq-setup/PLAN.md:43)), which needs the worktree link, while an existing project can be switched to shared after bypassing the registration-only managed-root check.

   Fix: Validate shared-workspace eligibility at config load/create/reconnect, and gate link creation on owned worktree presence rather than the project’s default mode.

12. **Two guarantees remain broader than the implementation can provide.** Dux’s explicit stage/discard/pull actions mutate the real checkout, and the derived writer badge cannot see another `DUX_HOME` or unmanaged process, contradicting [PLAN.md:6](/Users/siavash/Projects/dux-amq-setup/PLAN.md:6) and [PLAN.md:55](/Users/siavash/Projects/dux-amq-setup/PLAN.md:55).

   Fix: Scope the invariant to automatic lifecycle/workspace-management operations and label the writer warning as current-store visibility, documenting foreign writers as undetectable.

VERDICT: REVISE
### Claude's response (round 4)

All 12 accepted — refinements within round-3 areas (migration, AMQ ownership, deletion), several sharpened by the shared cross-store AMQ root:
1. Migration 0005 becomes an atomic TABLE REBUILD (ADD COLUMN can't do NOT NULL/CHECK; unique index permits multiple NULLs) preserving columns/indexes/session_prs FKs.
2. Soft-delete (deleted_at): ordinary delete retains the full row so later hard-purge can still GDPR-erase; physical removal only after purge.
3. store_id persisted in durable DUX_HOME metadata; reset purges owned AMQ dirs (or retains metadata) so tombstones aren't stranded.
4. reset/cleanup ABORT before any mutation on incomplete config/session/tombstone/protected-path load — fail-closed can't fall through to warn-and-delete.
5. Persist-first using the Spawning state: persist UUID/handle + reserve AMQ handle under lock BEFORE spawning the provider.
6. GLOBAL handle uniqueness across the shared AMQ root (per-store unique index doesn't stop agents/alice colliding physically): atomic global reservation under the lock + suffix deconflict + foreign-collision migration policy + exact-owner verify before purge.
7. Lock target corrected to meta/config.lock (stable), all three wrappers + Rust, mandatory flock (was silently optional).
8. Delete/purge records + kills the disowned per-session wake PID under the lock so a tombstoned inbox isn't drained.
9. Shared PR: discover once from live HEAD per canonical path, ignore the stored-PR shortcut, fan out, skip while detached.
10. Reject Peers when EITHER endpoint is shared (was target-only), test both directions.
11. Link gate keyed on OWNED-WORKTREE PRESENCE (not project default mode) so a shared-default project's isolated Fork still links; eligibility validated at load/create/reconnect.
12. HONESTY: scoped invariant #1 to AUTOMATIC lifecycle ops (user stage/discard/pull DO mutate the checkout by design); badge = current-store visibility, foreign-DUX_HOME writers documented as undetectable.

Convergence read: round-4 findings refine round-3 areas rather than opening new categories, so the design is deepening but not diverging. No rejections. Added a phased sequencing + "land dark behind the config default" implementation risk. Round 5 is the MAX_ROUNDS cap.

## Round 5 — Codex (final)

All 12 round-four blockers are addressed. No must-fix-before-implementation blockers remain.

Non-blocking implementation refinements:
- Choose the concrete handle-length bound N and run foreign_key_check around the table rebuild.
- Upgrade a legacy path-based marker only when it maps unambiguously to one local row; otherwise preserve it as foreign and suffix the new handle.
- On startup, convert an interrupted, PTY-less Spawning row into a visible retryable state while retaining its reservation.
- Export DUX_STORE_ID; make wake PID optional and surface reduced delivery observability for providers without an AMQ wrapper.
- Follow the owned-AMQ-purge branch for reset, and make soft-deleted purge targets resolve by UUID/handle with ambiguous branch aliases rejected.
- State that workspace-wide provider-history deletion also removes non-Dux conversations stored under that workspace path.

VERDICT: APPROVED

### Claude's response (round 5)

Converged. Folded all 6 non-blocking refinements into a new "Implementation refinements" section of PLAN.md. 

---

Converged after 5 rounds (4 REVISE → 1 APPROVED). The review turned a 1-page "agents share the main workspace" sketch into a hardened, phased spec: it caught that session PATH is used as identity in ~8 places, that a shared AMQ root needs GLOBAL handle uniqueness under a mandatory lock, that three deletion paths (not one) touch the real repo, that migration needs a crash-atomic table rebuild + soft-delete for GDPR, that startup auto-spawn and project registration both mutate the real checkout, and that the original "never touch the real repo" invariant was literally false (user edits mutate it by design). Awaiting user sign-off before any code.

---

## Act 3 — Build

Builder: Codex (gpt-5.6-sol, reasoning max). Phased per the plan; Claude reviews each phase (diff + independent gates + adversarial review) before committing.
Branch: shared-workspace (off main 5fc4472). Build thread: 019f5957-273c-7251-9c90-38e81c8fc599.

### Phase 1 — Codex build (§A: schema migration, identity, soft-delete) — DARK

Codex implemented migration 0005 (crash-atomic table rebuild + in-transaction Rust handle backfill + foreign_key_check), immutable fail-closed agent_handle, soft-delete tombstones, single accessor, and a Retryable lifecycle state for interrupted spawns. 16 files, +872/-54. 11 new proof tests. Report: no spec deviations; nothing reads shared_workspace/agent_handle for routing yet (Phase 2+ seam).

### Claude's verdict (Phase 1) — pre-commit

Independent gate rerun: all 5 cargo gates exit 0. Traced the load-bearing logic: migration is fully single-transaction (create new + Rust backfill + session_prs rebuild + foreign_key_check + user_version bump, commit-or-rollback); backfill derives basename→branch→id→"agent" (never empty), truncates to 64 (ASCII so char==byte, matches CHECK), dedups deterministically ORDER BY id with suffix-length-safe truncation; fail-closed load via validate_stored_handles; soft-delete + Retryable implemented. Launched an independent adversarial review of the foundation phase before committing (schema migration on real user DBs = highest stakes, per the audit's per-tier review precedent).

### Round 1 review — Phase 1 (independent adversarial)

Verdict: ISSUES — but all load-bearing correctness CONFIRMED clean (single-transaction atomicity, backfill column alignment, FK preservation, fail-closed load, soft-delete filter coverage, Retryable transitions, dark guarantee, no new panics). 1 should-fix + 3 nits:
- SHOULD-FIX: an FK-orphan session_prs row (session gone without cascade) would abort migration 0005 under foreign_keys=ON, and since App::new opens with `?`, it BRICKS TUI launch with a non-actionable error.
- nit: read-only CLI snapshot returns empty for the whole v4 user base until the TUI migrates (load_sessions selects agent_handle, fails on v4, swallowed).
- nit: TOCTOU between assign_unique_agent_handle read and upsert (safe under single-writer invariant).
- nit: deleted_at parsed-field vs SQL-visibility could diverge in the dead-code include-deleted path.

### Claude's response — Phase 1 fixes (applied directly)

Small/well-specified, so fixed directly (audit takeover precedent):
- Migration now copies only referentially-valid session_prs rows (`where session_id in (select id from agent_sessions_new)`) — orphans are dropped, migration succeeds, TUI can't be bricked. Repurposed the old orphan-triggers-rollback test into migration_0005_drops_orphan_session_prs_and_succeeds, and added migration_0005_rolls_back_atomically_on_failure with a real failure trigger (leftover agent_sessions_new table) that still proves single-transaction rollback.
- Added SessionStore::CURRENT_SCHEMA_VERSION + schema_version(); collect_sessions_snapshot now prints a "launch dux to migrate" note instead of silently showing zero sessions on a stale read-only DB.
- Added the single-writer-invariant comment at the handle assign/upsert seam.
- Added the deleted_at visibility-vs-parse comment for the future include-deleted caller.

### Phase 1 — committed & verified

Re-review fixes applied, all 5 cargo gates green. Committed 4bc947b..0d03ab1 (5 per-item commits) on branch shared-workspace. Foundation is DARK: schema v5 + immutable handles + soft-delete + Retryable exist and are tested; no routing/purge/identity behavior changed yet. Next: Phase 2 (§B — AMQ ownership, global handle reservation, mandatory meta/config.lock, wake-daemon lifecycle) which begins wiring identity into the AMQ registry + wrappers.

### Phase 2 — Codex build (§B: AMQ ownership, global handles, locking, wake)

Codex implemented §B5–B10 across 17 files (+1584/-704; peer.rs +918): durable per-DUX_HOME store_id (atomic create, fail-closed), global agent_handle reservation under a mandatory rustix::flock on meta/config.lock, {store_id,session_id,wake_pid} ownership markers, own-store-only pruning (foreign/standalone never pruned), persist-first spawn ordering (reserve+persist before spawn, Retryable on failure), wake-PID termination on delete, tombstone-on-delete/free-on-hard-purge, all three wrappers updated to lock + write the marker atomically, plus a tests/fakes/flock shim so the concurrency bats run on macOS. 11 new Rust tests + new bats. Still DARK. Documented deviation: the very-first SQLite insert failure leaves no row (nothing to recover) and cleans up the worktree; every later failure leaves a recoverable Retryable row.

### Claude's verdict (Phase 2) — pre-review

Independent gates: all 5 cargo gates + shellcheck exit 0; bats only the 2 known macOS finalize failures (pass on Linux CI). Read the load-bearing core: AmqRegistryLock holds flock on meta/config.lock across the whole read-modify-write and fails closed; reserve_and_persist checks local-used AND physical-marker claimability then persists+markers under the lock; marker_state classifies Free/Owner/Legacy/Foreign with Foreign as the safe default; reconcile prunes only own-store handles with no live session and keeps tombstoned ones; legacy-path upgrade requires the unambiguous single-session match. Launched independent adversarial review focused on lock correctness, reservation races, marker misclassification, persist-first ordering, wake-pid reuse, and the wrapper protocol before committing.

### Phase 2 review — independent adversarial

Verdict: ISSUES. Lock correctness + persist-first ordering + marker classification VERIFIED SOUND (same-inode flock mutual exclusion Rust↔wrappers, hard-fail on missing flock, foreign as safe default, fake flock uses real ruby File.flock so serialize tests are real). 5 should-fix + 3 applied nits:
- S1: session undeletable when its marker isn't exactly ours (store_id regenerated / transient error / worktree already removed) — delete must skip foreign AMQ cleanup, not fail.
- S2: bootstrap hard-fails on any sync_amq error; the shared meta/config.json is machine-writable → one corrupt shared file bricks dux for every DUX_HOME. Must log-and-degrade at boot.
- S3: global config.lock held during the ~1.5s wake SIGTERM→KILL loop stalls all stores; drop the lock before the kill/wait.
- S4: pid-reuse guard only matches "amq"+"wake" in cmdline → could SIGKILL another store's wake daemon; also require --me <handle> + --root <root>.
- S5: purge still targets agents/<basename> but inbox is now keyed by agent_handle → deconflicted/renamed session purges a possibly-FOREIGN inbox; derive the target from session.agent_handle() now.
- N1 char-slice next_global_handle; N2 add true concurrent same-handle contention test; N3 ensure_owner_marker partial-failure leaves empty dir → clean up.

### Claude's response + Codex fix round 1

All 5 should-fix + 3 nits routed to the same Codex session. S1/S2 are the important shared-machine availability fixes (deletion always succeeds locally; boot degrades AMQ instead of bricking every DUX_HOME). store_id-loss orphaning nit acknowledged as mitigated by S1.

### Phase 2 fix round 1 + commit

Codex fixed all 5 should-fix + 3 nits (S1 delete-skips-foreign, S2 boot-degrades, S3 lock-released-before-wake-kill, S4 pid guard requires --me+--root, S5 purge targets agent_handle, N1 char-slice, N2 concurrent-same-handle race test, N3 partial-marker-write cleanup), each with a test; 970 tests + all cargo gates + shellcheck green; bats only the 2 tolerated macOS finalize failures. Claude verified S1/S2/S3/S4/S5 logic directly (tombstone returns Ok on non-owned markers + caller logs-and-continues; bootstrap sync is now ()-returning log-and-degrade; wake kill runs after the lock block; is_amq_wake_process gates the kill on handle+root; purge targets agents/<agent_handle>). Given the localized fixes + test-per-item + direct verification, did not run a second full agent review of the fix round. Committed bd6e107..8c3cd93 (5 per-item commits). Phase 2 done — still DARK. Next: Phase 3 (§D routing — shared→always-AMQ, reject Peers when either endpoint shared, companion-terminal identity, no-ambiguous-cwd sender).

### Phase 3 — Codex build (§D routing) + commit

Codex implemented §D12/D13 in src/peer.rs (+companion env in sessions.rs/pty.rs), +255/-24, 10 new tests. choose_transport: either-endpoint-shared → AMQ (reject explicit claude-peers both directions); two worktree endpoints unchanged (dark, tested by worktree_endpoints_still_prefer_claude_peers). session_for_cwd: rejects same-depth cwd ambiguity with a --from hint, preserves deepest-worktree resolution; --from resolves exact agent_handle first. Companion terminals export DUX_SESSION_ID/DUX_STORE_ID/DUX_AMQ_HANDLE.

Claude verdict: all 5 cargo gates + shellcheck exit 0; verified choose_transport (correct both-direction shared→AMQ + dark worktree path) and session_for_cwd (deepest-worktree + same-depth ambiguity bail) directly. Low-risk pure-logic phase with comprehensive both-direction/dark/ambiguity tests — proportionate to skip a separate full agent review (reserved for the high-stakes concurrency/deletion phases). Committed b80085d..f817875. Still DARK (no shared sessions exist until Phase 4). Next: Phase 4 (§E lifecycle & branch — SharedWorkspace create request, registration honors mode / no real-branch checkout, branch+PR from live HEAD, no startup auto-spawn, worktree-link gated on owned-worktree presence).

# Shared main-workspace mode

Ported from the fork's shared-workspace phases 1 to 6. The frozen design spec,
verbatim from the fork, is
[`superpowers/specs/2026-07-13-shared-main-workspace-spec.md`](superpowers/specs/2026-07-13-shared-main-workspace-spec.md).
This page describes what runs on the crates layout today.

## What it is

By default dux gives every agent its own git worktree. In shared mode a new
project agent runs **directly in the registered project checkout** instead.
dux creates no worktree and no branch, never switches the checkout, runs no
startup command there, and never writes `.git/info/exclude`.

## Configuration

```toml
[workspace]
default_mode = "shared"      # or "worktree"
auto_resume_shared = false   # shared agents also auto-reopen at startup

[[projects]]
path = "$HOME/projects/example"
workspace_mode = "worktree"  # optional per-project override; omit to inherit
```

- **Consent is preserved.** A config written before this section existed has
  no `[workspace]` table and keeps worktree mode. Only a freshly created
  config renders `default_mode = "shared"`. Saving an unrelated setting never
  adds the section.
- A shared project may not live inside `DUX_HOME` or its worktrees root
  (symlinks and not-yet-created paths are resolved). The TUI refuses to load
  such a config, and a shared create is refused before any row is written.
- `Config.schema_version` from the fork is **not** ported. Upstream migrates
  config with idempotent, key-based rules on every load (`config_migrate.rs`),
  so a version ladder would have nothing to gate.

## Lifecycle rules

| Behaviour | Where | Test |
|---|---|---|
| New agent in a shared project runs in the checkout, records live HEAD (detached accepted) | `agent_job::run_create_shared_agent_job` | `shared_create_uses_real_checkout_without_worktree_or_link`, `shared_registration_accepts_detached_head_without_checkout` |
| Shared checkout inside dux state is refused before persisting | same, via `config::validate_shared_workspace_path` | `shared_create_rejects_managed_root_before_persisting`, `config_load_rejects_shared_project_inside_managed_root` |
| Fork, PR and adopted-worktree agents stay isolated | create request choice | `fork_from_shared_session_still_creates_isolated_worktree_row`, `fork_stays_isolated_under_shared_project_default` |
| Delete never removes the checkout; asking to is refused out loud | `AgentSession::deletion_may_remove_directory`, `Engine::{do,begin}_delete_session` | `begin_delete_session_never_removes_shared_workspace`, `shared_delete_dialog_hides_worktree_checkbox` |
| Branch rename refused, title rename allowed, handle immutable | `Engine::prepare_branch_rename` | `shared_session_rejects_real_branch_rename`, `shared_session_rename_changes_only_display_title` |
| Startup auto-reopen skips shared agents unless `auto_resume_shared` | `Engine::auto_reopen_candidates` | `startup_auto_resume_excludes_shared_sessions_by_default`, `startup_auto_resume_includes_shared_sessions_when_opted_in` |
| Worktree-conflict auto-detach never fires for shared agents | `Engine::detach_conflicting_worktree_session` | `conflict_detach_is_noop_when_either_session_is_shared` |
| Second live writer needs consent (TUI) / is refused (web) | `App::dispatch_create_agent_request`, reconnect, `wire::create_agent_command` | `second_shared_writer_requires_confirmation_for_create_and_reconnect`, `wire_create_agent_follows_shared_mode_and_refuses_a_second_writer` |
| Header shows `CURRENT STORE ONLY · N LIVE WRITERS`; rows show `SHARED` | `Engine::shared_multi_writer_summary`, `render_header`, `render_agent_row` | `live_multi_writer_badge_and_shared_row_badge_render` |
| Project-root worktrees link only for projects with an isolated agent | `engine::project_link_allowed` | `shared_only_project_skips_link_but_isolated_fork_creates_it` |

## Protected-workspace deletion guard

Every whole-worktree removal (`git::remove_worktree_keep_branch`, and so
`git::remove_worktree`) first runs `git::guard_whole_workspace_removal`
against every registered project checkout. The removal is refused when the
target is a project, is inside one, or contains one. Paths are resolved with
symlinks followed, including a target that is already gone, and relative or
`..` paths are refused. An inventory that cannot be read refuses the removal.
Contained-file operations, such as discarding an untracked directory, are not
affected.

## Honest limits

- The multi-writer warning sees only this dux home's agents. Agents under
  another `DUX_HOME` and unmanaged processes in the checkout are invisible, so
  the absence of the badge is not proof of exclusive access.
- Shared writers share one index, staging area and branch. One can stage,
  commit, discard or switch branch under another. Use Fork or worktree mode
  when changes must diverge.
- AMQ routing by immutable `agent_handle` for shared endpoints, and the AMQ
  ownership protocol under a shared root, live in the peer router
  (`crates/dux-core/src/peer`).

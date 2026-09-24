# Phase 15 — Runtime subsystem decomposition

**Track:** D (decomposition) · **Parallel-safe with:** 12, 13, 14, 16, 17
**Depends on:** 02, 05 · **Blocks:** 24

## Goal

Split the nine runtime modules over 500 lines into trees under the cap: PTY, raw input,
storage, model, git, diff, watch, peer, purge, inject runtime, and resume recovery.

## Context

`audit03` **explicitly deferred** decomposition — *"splitting thousands of lines during
a security/data-loss fix phase would create risk without behaviour proof"*
(`docs/audits/audit03/06-architecture-modernity.md`). That deferral is precisely why
`app/workers.rs` has since doubled from 2,058 to 4,394 lines and the tree grew 19.4%.
The deferral was reasonable then; the behaviour proof now exists (Phases 05–07), so the
reason no longer holds.

**Production-only line counts** are used below — several files are inflated by inline
tests that move with their subject under Phase 05.

## Module trees

```text
src/pty/                     # 2,161 total, 1,357 production
  mod.rs          60   re-exports, PerSessionEnv (:135-160)
  snapshot.rs     50   snapshot types
  client.rs      340   PtyClient::spawn* (:150-240), write_bytes, resize (:546), Drop (:662-727)
  reader.rs      130   reader thread, reader_loop (:255-340)
  handle.rs       70   PtyHandle wrapper
  terminal.rs    250   TerminalState (:830-1010), scan_recent_lines (:1008)
  events.rs       80   pause/resume, PendingIngest (:70), sync_pause_state (:480-486)
  color.rs       130   NamedColor -> ratatui mapping (:1180-1290)
  env.rs         100   apply_terminal_env* (:1297-1332)

src/raw_input/               # 763 total, 264 production
  mod.rs          20
  mouse.rs       135
  split.rs       120   the OSC/CSI splitter (:208-231)

src/storage/                 # 1,485
  mod.rs         130   SessionStore, open_connection (:886-897)
  store_id.rs    120
  migrate.rs     290   the table (:129-154), runner (:229-244)
  legacy_column.rs 200 migration 5 backfill (:264-350)
  sessions.rs    330   upsert_session (:589-681), load_sessions* (:770-800)
  prs.rs         250
  codec.rs       110   state_json encode/decode
  testkit.rs     210

src/model/                   # 887
  mod.rs         130
  session_state.rs 350 SessionState + transition (:126-330)
  session_settings.rs 250 to_pty_env (:635-711)
  agent_handle.rs  40  normalize_agent_handle
  agent_session.rs 130

src/git/                     # 2,236 total, 1,282 production
  mod.rs          60
  refs.rs        130   symbolic_ref, rev_parse
  status.rs      230   changed_files (:448-588), parse_numstat_record (:894)
  worktree.rs    200   add/remove/list (:599-700)
  safety.rs       90   guard_whole_workspace_removal, whole_workspace_target_is_within
                       — the natural home for a shared run_git(args, timeout) helper
  staging.rs     120
  mirror.rs      130
  links.rs       110   ensure_project_worktrees_link_ignored (:328-370)
  naming.rs       90

src/diff/                    # 985
  mod.rs          40
  syntax.rs       60   the SyntaxCache that becomes a OnceLock (Phase 08 item 6)
  render.rs      200   diff_file (:42-75)
  wrap.rs        160

src/watch/engine/            # 1,002
  mod.rs         130
  effect.rs       60
  runtime.rs     180   observe / rebaseline / schedule_fire_at (:415-452),
                       and the currently-dead is_active (:319)

src/peer/                    # 2,281 total, 1,332 production
  mod.rs          60   types (:27-70), append_session_env (:85-98)
  cli.rs         180   run_peer* (:71-238), parse_send_args (:240-309)
  routing.rs     260   infer_sender (:326-387), resolve_target (:389-421),
                       choose_transport (:423-459), session_aliases (:633-660)
  transport_amq.rs 40  amq_send (:479-497)
  transport_claude_peers.rs 120  claude_peers_* (:499-598)
  registry.rs    380   AmqRegistryLock (:713-741), marker_state (:931-958),
                       ensure_owner_marker* (:968-1017), reconcile_amq_root (:794-905),
                       write_atomic (:1076-1097)
  lifecycle.rs   290   tombstone/free (:1099-1263), terminate_wake_pid (:1265-1331)

src/purge/                   # 1,276 total, 1,084 production
  types.rs (:80-294) · plan.rs (:300-483) · containment.rs (:485-567)
  execute.rs (:573-765) · log_redact.rs (:767-961) · confirm.rs (:963-996)
  notify.rs (:998-1079)

src/app/inject_runtime/      # 1,756 total, 1,307 production
  consts.rs (:35-102) · receiver.rs (:154-202) · watcher.rs (:225-334)
  drain.rs (:337-504) · tick.rs (:506-745) · deliver.rs (:805-968)
  payload.rs (:1174-1306) · warn.rs (:1070-1172)

src/resume_recovery/         # 1,260 total, 920 production
  roots.rs (:24-98) · codex_capture.rs (:100-357) · scan.rs (:591-744)
  match.rs (:374-589) · copy.rs (:746-918)
```

## Work items

1. **Split in dependency order**, leaves first: `model/`, `raw_input/`, `diff/`,
   `watch/engine/`, then `storage/`, `git/`, `pty/`, then `purge/`, `peer/`,
   `resume_recovery/`, `inject_runtime/`.
2. **Introduce `git/safety.rs`'s shared `run_git(args, timeout)` helper.** Git safety was
   verified clean (`--porcelain=v1 -z`, `--numstat -z`,
   `symbolic-ref --quiet --short HEAD`, `-c color.diff=false` all in use) — a single
   helper keeps it that way by construction rather than by discipline, and gives Phase 09
   one place to enforce timeouts.
3. **Move `sanitise_handle` to one home.** It is duplicated at
   `inject_runtime.rs:157` and in `peer.rs` (used at `:328, 363, 390, 646`). Canonical
   home is `crate::amq_inject` — and Phase 18 will share it with the new crate.
4. **Move `inject_runtime.rs`'s ~200 lines of pure protocol code out of the app layer.**
   `inject_runtime.rs:96, 157, 163, 199, 1185, 1205, 1217, 1228, 1252, 1275, 1297` are
   all `self`-free and fully unit-tested. They belong in `src/amq_inject/protocol.rs`,
   not `src/app/`. Their 449 lines of tests exercise only this pure half — those tests
   move with them.
5. **`pty/terminal.rs` carries Phase 08's OSC cap** (item 3) and `pty/client.rs` carries
   the Drop-timeout leak fix (item 5) and Phase 09's non-blocking teardown (V4). If Track
   C has landed, move the fixed code; if not, move as-is and let Track C rebase. **Never
   both in one commit.**
6. **`diff/syntax.rs` carries the `OnceLock` change** from Phase 08 item 6 — same rule.
7. **`peer/registry.rs` carries Phase 09's non-blocking flock (V1)** — same rule.
8. **Preserve `storage/migrate.rs`'s crash-atomic semantics exactly.** The runner bumps
   `user_version` in-transaction; `tests/storage_migrations.rs` (662 lines) covers the
   chain. Moving is fine; restructuring the transaction boundary is not.
9. **Preserve `purge/`'s containment guards byte-for-byte.** `tests/purge_integration.rs`
   is 22 tests covering symlink escapes in both directions, foreign-inbox protection,
   per-category retry-after-failure, and documented ordering. This is the strongest test
   suite in the codebase — treat a red result as a real defect, never as a test to update.
10. **Add file headers** with generated trees per `CONVENTIONS.md` §1.
11. **Delete or document `watch/engine`'s dead `is_active` (`:319`).** Phase E's
    `#[allow]`/dead-code sweep catches it otherwise; cheaper to resolve while in the file.

## Acceptance criteria

- [ ] All eleven trees created; the original single files no longer exist.
- [ ] No file in any tree exceeds 500 lines.
- [ ] `run_git` helper exists and every git invocation routes through it.
- [ ] `sanitise_handle` has exactly one definition.
- [ ] The ~200 lines of pure inject protocol code live outside `src/app/`.
- [ ] `tests/purge_integration.rs` (22 tests) green — zero assertions modified.
- [ ] `tests/storage_migrations.rs` green — zero assertions modified.
- [ ] `tests/watch_engine_integration.rs` (5 tests, real PTYs) green.
- [ ] `tests/pty_integration.rs` green.
- [ ] `is_active` removed or documented.
- [ ] File headers present; `module-trees --check` passes.
- [ ] `cargo test --all-features` green, same test count.

## Validation

```bash
cargo test --all-features
cargo test --test purge_integration
cargo test --test storage_migrations
cargo test --test watch_engine_integration
cargo test --test pty_integration
ci/check-file-length.sh
cargo modules dependencies --acyclic --bin dux
cargo run -p xtask -- module-trees --check

# git-safety regression guard: every invocation must go through run_git
grep -rn 'Command::new("git")' src/ | grep -v 'git/safety.rs'   # expect: no output
```

## Risks

| Risk | Mitigation |
|---|---|
| Track C edits the same files concurrently | Track C is scheduled first and wins; this phase rebases. Never mix a behaviour fix and a move in one commit |
| `pty.rs`'s reader-thread/Drop interaction is subtle | The existing Drop test tolerates the detach branch and proves little — rely on Phase 08's new leak test, and move `client.rs`/`reader.rs` in a single commit so the pair stays consistent |
| Splitting `peer/` breaks the AMQ registry contract the bash wrappers depend on | `ownership.bats` (7 tests) and `wrappers-p1.bats` (20) pin it; run both after `peer/registry.rs` moves |
| Circular imports between `model/` and `storage/` | `cargo modules dependencies --acyclic` catches it; `codec.rs` exists as the seam |

## References

- `artifacts/research-runtime-memory.md` §3 (all trees, with source ranges)
- `artifacts/research-app-monoliths.md` R12, R13

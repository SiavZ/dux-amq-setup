# OUTCOME coverage ledger: fork `main` (229 commits) vs `wt/integration`

Auditor: independent outcome audit, 2026-09-25. Method: read every fork commit's
stat + body (`git show`), then located the equivalent behaviour in the merged tree
at `/Users/siavash/Projects/dux-port-wt/integration` by (a) integration port commits
that cite the fork sha (108 of 229 cite one directly), (b) byte-level tree diffs of
`dux-amq/`, `docs/`, `.github/` and root files against fork `main`, and (c) targeted
symbol/config greps in `crates/`. Where a fork commit's effect was later reversed on
fork `main` itself, the class reflects the fork's FINAL state (the port goal is
superset of fork main, not of every intermediate commit).

Deliberate-drop rulings applied (from the task brief): AI commit messages /
oneshot_args / oneshot_output; Config.schema_version; rust-toolchain.toml (MSRV
rust-version=1.88); gemini provider retirement; numbered SQL migrations
(ensure_column instead); fork SessionState typestate; duplicate-handle guard
3fb27457 superseded by 5531378f.

## Summary

| Class | Count |
|---|---|
| PRESENT | 220 |
| DROPPED-DELIBERATE | 4 |
| SUPERSEDED-BY-UPSTREAM | 3 |
| PARTIAL | 1 |
| MISSING | 1 |
| **Total** | **229** |

## MISSING / PARTIAL, ranked by user impact

1. **PARTIAL 1a74678f** (fix: plug resource leaks and harden AMQ delivery pipeline,
   #35). Seven of its eight sub-items are present: `symbolic-ref` plumbing
   (`crates/dux-core/src/git.rs:104`), mktemp-derived bridge filenames
   (`dux-amq/scripts/dux-amq-inject-bridge:432-435`), per-session map cleanup on
   delete (`Engine::forget_amq_session`, `crates/dux-core/src/engine/amq.rs:263-272`,
   removes session_settings / last_user_keystroke / pr_last_checked entries with
   tests at engine/mod.rs:6626, 9843), wrapper-side stale-wake kill and atomic
   identity claiming (wrappers byte-identical to fork main). What is absent: the
   explicit **cap of 4 on concurrent PR-check threads** that bounded `gh` subprocess
   storms from refs-watcher cascades. I grepped `crates/dux-core/src/{gh.rs,engine/*}`
   for any semaphore / MAX_CONCURRENT and found none; upstream bounds per-agent
   probes with in-flight keys (`pr_attach_in_flight_message`,
   `a_second_folder_repo_probe_is_a_no_op_while_the_first_is_in_flight`) and bounds
   each `gh pr view` call in time, not in concurrency. Impact: under a large
   refs-watcher event cascade, gh fan-out is bounded per agent but not globally.
2. **MISSING 33897148** (chore(audit02): gitignore `.claude/` + commit cleanup
   artifacts). The cleanup artifacts were retired on fork main by 562419e0 anyway,
   but the `.claude/` gitignore entry did not make it into the integration
   `.gitignore` (checked: only `.agent-mail/`, `.superpowers/` etc. are listed;
   `.claude/settings.local.json` shows as untracked noise in `git status`). Impact:
   cosmetic / housekeeping only.
3. **Process gap, not a commit** (folded into rows 4bc947b8 / 3da9bc6f / b603f551):
   `python3 ~/.jcode/scratch/fork_test_parity.py` on the integration tree reports
   8 UNACCOUNTED fork test names, all in `tests/storage_migrations.rs`
   (migration-0005 suite + migrate-from-empty/idempotent). The BEHAVIOUR is present
   and tested under new names (`crates/dux-core/tests/upgrade_shared_workspace.rs`:
   `an_upstream_database_gets_derived_valid_unique_handles_for_every_row`,
   `fork_state_json_rows_load_as_reconnectable_sessions`;
   `upgrade_database.rs`: `the_sort_order_backfill_preserves_the_order_the_user_last_saw`,
   `opening_a_database_this_build_created_is_a_no_op_the_second_time`), but no
   TEST_DISPOSITIONS-*.tsv row covers them. Whoever owns the final parity pass should
   add `obsolete`/`renamed` lines so the parity script reaches zero.

Note (not counted as a gap): **6e5a3a6c** pinned `macos-15-intel` for the
x86_64-apple-darwin release build. Integration's `release.yml` builds that target on
`macos-latest` instead. The user-visible invariant (no retired runner label, release
never queues forever, `dux-darwin-amd64.tar.gz` still ships; `supply-chain-rails.bats`
asserts no `macos-13`) holds, so it is classed PRESENT; only the literal label differs.

## Ledger (oldest first)

| sha | subject | class | evidence / gap |
|---|---|---|---|
| 561e9965 | feat: dux-amq overlay - wrappers, install script, docs | PRESENT | `dux-amq/` tree byte-identical to fork main except later-commit deltas (verified `diff -rq`); imported by 4ac87d46 "verbatim from fork main" |
| dfed17fb | fix(clipboard): OSC 52 fallback | PRESENT | `crates/dux-tui/src/clipboard.rs:96-140` `osc52_sequence`/`base64_encode`, 100 KiB cap; port 80ecebf7 |
| 478c9e3c | feat(restore): auto_resume_on_start | PRESENT | `crates/dux-core/src/config.rs` `[defaults].auto_resume_on_start`; port 9d790833 |
| 33b35948 | fix(wrappers): force amq wake --inject-mode=raw | PRESENT | `dux-amq/wrappers/{claude,codex,gemini}-amq` byte-identical to fork main |
| 12c1b272 | feat(render): vertical scrollbar | PRESENT | `crates/dux-tui/src/app/render.rs:4163` `render_agent_scrollbar` cites fork 12c1b272; port d479f930 |
| 42a12302 | fix(wrappers): /dev/tty stdin + basename identity | PRESENT | wrappers identical; dux side mirrors priority in `amq` `match_receiver` (basename first) |
| 4456054d | feat(claude-amq): default --dangerously-skip-permissions | PRESENT | Net effect: reversed on fork by 8fcbb2e6 (opt-in YOLO); integration claude-amq matches fork final state |
| 345d8827 | feat(install.sh): VSCode Ctrl-G passthrough | PRESENT | `dux-amq/vscode/settings-additions.json` identical; install.sh `configure_vscode_remote` consumes it |
| 16f85bd4 | docs: audit01 report + 18-phase plan | PRESENT | `docs/audits/audit01/audit01.md` present; plan set imported then retired on fork final state (562419e0), mirrored |
| 8f7da3a8 | chore(audit01): baseline lint fixes | PRESENT | Folded into imported trees; fmt/clippy gates enforced in pr.yml |
| 7c9bd17b | test(audit01): bats scaffolding | PRESENT | `dux-amq/tests/{lib/setup.bash,smoke.bats,fakes/}` present; empty `scrollbar_render.rs` placeholder superseded by real render tests in dux-tui |
| 85f6f68e | ci(audit01): overlay-ci + make overlay-test | PRESENT | `.github/workflows/overlay-ci.yml` + Makefile `overlay-shellcheck`/`overlay-test` targets |
| 9d4b3f6f | docs(audit01): preflight artifact | PRESENT | Matches fork final state: artifact retired by 562419e0, mirrored in integration |
| c19ab4e7 | fix(install): pin dux/amq/skills + sha256 | PRESENT | `dux-amq/install.sh` pinned `dux-amq-v0.1.1` (fork release) + amq v0.61.0 hashes |
| aeaba87e | feat(install): hard-fail preflight | PRESENT | install.sh `require_install_env` aggregated preflight; wrappers-p1.bats env -i cases |
| 61ebf28b | feat(install): versioned >>>/<<< markers | PRESENT | install.sh `strip_block` + versioned markers; `strip-block.bats` |
| 554255df | feat(install): pin amq + hash-guarded eval | PRESENT | install.sh `AMQ_BINARY_SHA256`; `bashrc-additions.sh` `_amq_shell_setup_guarded` (file identical) |
| a5e67926 | ci(security): pin actions by SHA | PRESENT | workflows pin actions by SHA (`checkout@11d5960a`, `dtolnay@4360b525`); per-job permissions |
| 8fcbb2e6 | feat(wrappers): default-deny YOLO + seeding | PRESENT | wrappers identical (`CLAUDE_AMQ_YOLO`/`CODEX_AMQ_YOLO`/`CLAUDE_AMQ_SEED_FROM_PARENT` gates); wrappers.bats cases |
| 7688c5db | fix(install): idempotency hardening | PRESENT | install.sh `meta/config.json` init gate; `strip-block.bats` + `install-idempotency.bats` |
| 2f669d91 | chore(audit02): preflight scaffolding | PRESENT | `docs/audits/audit02/audit02.md` findings present (plans retired per fork final state) |
| 2d9423ae | feat(security): sanitize operator strings | PRESENT | `crates/dux-core/src/sanitize.rs` `for_terminal` (\xNN hex form); chokepoint port c3d257fe; `tests/sanitize.rs` |
| fb9d1ea5 | fix(finalize): flock + atomic swap | PRESENT | `dux-amq/scripts/finalize-claude-migration.sh` + `finalize-migration.bats` identical to fork |
| dda3d23f | fix(pty): poison-tolerance + reader join | PRESENT | `crates/dux-core/src/pty.rs` let-else sentinels on render path; port 5eb96bb3 |
| 10d2266d | feat(storage): WAL + integrity + backup | PRESENT | `storage.rs` WAL + `integrity_check`; `engine/backup.rs` `spawn_backup_worker` + `[storage].backup_interval_minutes`; ports 3c3bbc19, 5eee35fd |
| 96c27c03 | ci(security): audit/deny/SBOM/provenance | PRESENT | `deny.toml`; release.yml `cargo auditable build`, SBOM, `attest-build-provenance`, `dux-checksums.txt` upload |
| b8b2ac9f | fix(test): hermetic install-idempotency bats | PRESENT | `install-idempotency.bats` differs from fork only by defaults rename + `\|\| false` bats fix; unshadow helper intact |
| 9b80934b | chore(toolchain): 1.85 -> 1.88 | DROPPED-DELIBERATE | rust-toolchain.toml ruling; equivalent is `rust-version = "1.88"` (Cargo.toml:17) + CI `toolchain: "1.88.0"` inputs |
| 8264b56f | feat(observability): tracing + JSON Lines | PRESENT | `crates/dux-core/src/logger.rs` tracing/appender/rotation + panic breadcrumb; `tests/logger_jsonlines.rs`; port 8d92c704 |
| b91ed679 | perf(app): git off UI thread | PRESENT | worker-dispatched changes panel; port 19f4da49 |
| cf6caeb1 | fix(wrappers): path encoder + realpath | PRESENT | `dux-amq/scripts/encode-claude-project-dir` + `encoder-fixtures.bats` + `fixtures/claude-paths.txt` present |
| 11d9274f | ci(matrix): macOS + OsStr git args | PRESENT | `crates/dux-core/tests/git_portability.rs`; OsStr args in git.rs; macOS legs in pr/test workflows; port da646187 |
| 28edf818 | feat(amq): HMAC envelope + replay | PRESENT | `amq-secret-init.sh`, `amq-send-signed`, `amq-receive-verify`, `amq-auth.bats` all present/identical |
| e393c1d1 | chore(rust): P1 hygiene bundle | PRESENT | git.rs `"--"` separators; `storage.rs` `is_safe_ident` allowlist (tests :4946-4988 cite e393c1d1); petname fallback; ports 3678f3a6, 5eee35fd |
| c6426735 | feat(observability): dux-amq doctor | PRESENT | `dux-amq-doctor` script + `doctor.bats`; `crates/dux-tui/src/doctor.rs` `run_doctor`; port f6c9737c |
| 07d9b0ba | perf(auto-resume): bounded concurrency | PRESENT | `crates/dux-core/src/auto_resume.rs` semaphore scheduler (concurrency/stale_days/stagger); `tests/auto_resume.rs`; ports 9d790833, 1831d455 |
| b603f551 | feat(storage): explicit schema versioning | DROPPED-DELIBERATE | numbered-migrations ruling; `SessionStore::migrate()` ensure_column; upgrade tests; config honour-once shim 921adb6d |
| 3e5c1c32 | feat(privacy): hard-purge | PRESENT | `crates/dux-core/src/purge.rs` + `purge_encoding.rs`; `cli.rs` `session purge/purge-all`; ports 93b0e7d6, 80fa413b |
| 6bfe258d | fix(wrappers): wake durability + preflight | PRESENT | `wrappers-p1.bats` full suite present (setsid/disown, pid file, collision marker, mtime guard) |
| 003e1bb5 | feat(wake): TIOCSTI fallback bridge | PRESENT | `dux-amq-inject-bridge` + `tiocsti-detect.bats` + sentinel + `DUX_AMQ_INJECT_MODE` in wrappers |
| e3bbc975 | docs(security): STRIDE threat model | PRESENT | `SECURITY.md` (20 T-rows) + `docs/operations/threat-model.md` + CLAUDE.md same-PR rule |
| d7d5df9b | docs(security): encryption-at-rest | PRESENT | `docs/operations/encryption-at-rest.md` + `scripts/install-gocryptfs.sh` present |
| 2d421673 | chore(audit02): final validation artifacts | PRESENT | Matches fork final state: artifacts retired by 562419e0, mirrored (docs/plans holds only rustport) |
| 9bb140dd | docs(audit02): branch protection doc | PRESENT | `.github/BRANCH_PROTECTION.md` present, updated for workspace job names |
| 0723c9d9 | chore(audit02): applied protection artifact | PRESENT | Recorded in BRANCH_PROTECTION.md (applied config + recipe); artifact itself retired with the plan set |
| 14ebb0c9 | fix(config): branch_sync_interval 0 | PRESENT | port 93e70c21 "branch sync and branch rename are opt-in" |
| 04bd9ead | refactor(app): extract GitState | SUPERSEDED-BY-UPSTREAM | upstream crates/dux-tui/src/app/* + dux-core Engine own the decomposition; pure refactor, no behaviour |
| 5ba99d54 | refactor(model): PtyHandle in SessionState | SUPERSEDED-BY-UPSTREAM | `engine/mod.rs:300` `providers: HashMap<TabId, PtyClient>`; SessionState typestate dropped per ruling |
| efb8224e | feat(model): SessionState enum + transitions | DROPPED-DELIBERATE | typestate ruling (TEST_DISPOSITIONS-ram.tsv); guards live per-operation, dispositions verified |
| d72ad8ad | refactor(app): UiState + RuntimeState | SUPERSEDED-BY-UPSTREAM | upstream App/Engine split supersedes; pure refactor |
| 2a54cbae | feat(limits): soft-warn pane count | PRESENT | port 9e2140bf; `config.rs` max_panes=0, `max_panes_soft_warn`=16 |
| 47fe44f6 | fix(amq-inject): T2 + reclaim + matcher + two-phase | PRESENT | `amq/config.rs` `verify_envelope=false` default; `queue.rs` `reclaim_stale_inflight`; `match_receiver`; `DeliveryPhase` two-phase |
| 4be38db4 | feat(amq-inject): rate-limited held log | PRESENT | `engine/amq.rs` `last_held_logged` 60s throttle + reason/depth fields |
| 9d28bcdc | fix(watch): two-phase SendText | PRESENT | `engine/watch_tick.rs` `flush_pending_watch_enters`; port c88b1c93 |
| 9a33b9d1 | fix(vscode): free Ctrl-O | PRESENT | `dux-amq/vscode/settings-additions.json` identical |
| 55986510 | docs(audit03): settings modal spec | PRESENT | `docs/audits/audit03/01-session-settings-modal.md` present |
| 9d86a520 | feat(storage): session_settings column | PRESENT | `storage.rs` ensure_column; `tests/session_settings_storage.rs` |
| f7fbbd04 | feat(model): SessionSettings + ContextMode | PRESENT | `crates/dux-core/src/session_settings.rs` (mode/yolo/watch_rule_arm/auto_clear/verify override) |
| c65653ec | feat(pty): spawn_with_env | PRESENT | `pty.rs` `spawn_with_env`; `agent_env.rs` `agent_launch_env`; port 8059aa64 + 3ed1030d PTY test |
| df9274b9 | feat(watch): auto-clear rule | PRESENT | `watch/builtin.rs` `auto_clear_rule_for` + `TASK_DONE_SENTINEL`; `tests/watch_engine_integration.rs` |
| 0baa481c | feat(amq-inject): Worker postscript | PRESENT | `amq/delivery.rs:77` `apply_inject_postscript` + tests |
| d0e601c5 | feat(app): settings modal + Ctrl-Shift-S | PRESENT | port 91fbf224; `crates/dux-tui/src/app/session_settings.rs` + render + keybinding |
| c7281798 | docs(audit03): settings landed | PRESENT | README.md "Per-Session Settings" (:287); SECURITY T15; dux-amq README postscript note |
| 70ae0fbb | fix(vscode): free Ctrl-P | PRESENT | vscode settings-additions.json identical |
| 651e6079 | fix(install): VSCode Remote-SSH auto-config | PRESENT | install.sh `configure_vscode_remote` reads the JSONC template via jq (single source of truth) |
| 32f46d2b | fix(app): unblock settings keyboard nav | PRESENT | port 91fbf224 (focus opens on ModeAttended; Title arrows navigate) |
| f1ec84d6 | feat(app): settings modal mouse | PRESENT | port 91fbf224 "mouse support" |
| 4ab5023e | feat(app): system prompt knob | PRESENT | `session_settings` system_prompt -> `DUX_SYSTEM_PROMPT`; claude-amq:301-313 `--append-system-prompt`; codex/gemini warn-drop |
| 29e21611 | feat(amq-inject): idle delivery | PRESENT | `amq/delivery.rs` `should_hold_for_quiet_window`; `active_session_quiet_secs`=60 |
| cae8e7b4 | feat(watch): overload-resume example | PRESENT | `crates/dux-tui/src/config.rs:2406` Overloaded (529) example; port f6b819ca |
| 6448c3f5 | feat: configure agents at creation | PRESENT | port 8221a5c3; `crates/dux-tui/src/app/new_agent_settings.rs` |
| 3a8ad183 | fix: new agent advanced mouse controls | PRESENT | ports 8221a5c3 + 465282a4 |
| 670b8c24 | fix: draggable center scrollbar | PRESENT | port d479f930 "draggable scrollback scrollbar" |
| d134b396 | fix: branch renames opt-in | PRESENT | port 93e70c21 |
| 9d9ffcef | fix: worktrees in dedicated editor windows | PRESENT | `crates/dux-core/src/editor.rs` `--new-window` (+tests); port a1658348 |
| 71c2a096 | fix: stabilize AMQ backlog | PRESENT | panic breadcrumb hook `logger.rs:95` `install_panic_hook` (fork crash.rs equivalent); `queue.rs` bounded scan + inflight claim |
| aaa59319 | fix: left list mouse scroll | PRESENT | `input.rs:9466` `handle_left_mouse_wheel`; port 71939f69 |
| 1d69de16 | fix: worktrees link in project root | PRESENT | `git.rs:1160` `PROJECT_WORKTREES_LINK_NAME` "dux-worktrees"; port a1658348 |
| 1a74678f | fix: resource leaks + delivery hardening | PARTIAL | Present: symbolic-ref (git.rs:104), mktemp bridge names, map cleanup (`forget_amq_session`), wrapper stale-wake kill, atomic claiming. Absent: the cap of 4 concurrent PR-check gh threads (no global concurrency bound found; upstream bounds per-agent probes via in-flight keys only) |
| 7bc3d7e4 | fix: harden AMQ delivery timing | PRESENT | `amq/config.rs` `phase_delay_ms`; `watch/delivery.rs` `MIN_ENTER_PHASE_DELAY_MS` |
| 3c306c65 | fix: auto-clear across tasks | PRESENT | `watch/builtin.rs` rule fires repeatedly with long cooldown |
| 1f34b24f | fix: backpressure AMQ delivery | PRESENT | `engine/amq.rs` bounded pending queues + held-wake handling |
| b0c52c34 | fix: quarantine rejected inject files | PRESENT | `queue.rs` `quarantine_expired` / `.expired/` |
| 349ee715 | fix: replace stale source registrations | PRESENT | wrapper source-marker refresh path; wrappers identical |
| edd6a644 | fix: expire stale inject messages | PRESENT | `engine/amq.rs:873-918` held-wake expiry with reasons |
| 0c4e4d0e | fix: auto-drain wakes under dux | PRESENT | bridge queueing under DUX_PANE + dux-side drainer in `engine/amq.rs` |
| b6638ae0 | fix: guard auto-clear during collaboration | PRESENT | `amq/activity.rs` + `auto_clear_collaboration_quiet_secs` (1800) wired in `amq_blocks_auto_clear` |
| 92698811 | fix: submit wakes before cooldown | PRESENT | `delivery.rs` SubmitPending path bypasses cooldown (test `submit...cooldown`) |
| e9b888c8 | fix: AMQ submit as recovery state | PRESENT | `engine/amq.rs` `DeliveryPhase::SubmitPending` recovery semantics |
| 4d38aa8a | fix: Codex AMQ submit key | PRESENT | `watch/delivery.rs:53` `submit_key_bytes_for_provider` |
| 8e523f02 | fix: Codex bracketed paste delivery | PRESENT | `amq/delivery.rs` bracketed-paste body builders + tests |
| d31e04f5 | feat: orchestrator mode policy | PRESENT | `amq/orchestrator.rs` `ORCHESTRATOR_SYSTEM_PROMPT` / `effective_system_prompt` / policy prompt |
| c2622dc0 | fix: codex orchestrator launch prompt | PRESENT | `amq/orchestrator.rs:92` `provider_needs_pty_orchestrator_policy` (codex/gemini/jcode) |
| 931f5ae0 | fix: no worker poll on startup | PRESENT | `build_orchestrator_startup_policy_prompt` ("do not poll them on startup") |
| cc7005d0 | fix: stale AMQ wake floods | PRESENT | `orchestrator.rs:97` cites cc7005d0 (checkpoint cadence, no cross-project polling) |
| 3da9bc6f | Add manual sidebar reordering | PRESENT | port f6a009f5 Shift-J/K; `project_order.rs` `update_session_order`; note: 1 fork sort-order test dispositioned obsolete (new sessions land at top by design) |
| 83bd3bfd | Fix fullscreen outside-click dismissal | PRESENT | `input.rs:31515` cites fork 83bd3bfd (structured + raw mouse paths) |
| 3fb27457 | Prevent duplicate AMQ handle spawns | DROPPED-DELIBERATE | superseded by 5531378f `assign_unique_agent_handle` (present in storage.rs; test 71d7ad7d) |
| 6f4399fe | Add Dux peer router | PRESENT | `crates/dux-core/src/peer/{router,amq,handle,session_store}.rs`; port 100b6f02; `dux peer` in crates/dux/src/main.rs:32 |
| 0d6fa312 | Force Claude peer routing + scroll | PRESENT | ports 100b6f02 + 16a18590 + 465282a4 |
| 55dba0f7 | Improve audit diagnostics + UI | PRESENT | port f6c9737c (dux doctor, Scrolling help section, keybind additions) |
| 981f83ad | Default Claude targets to Peers | PRESENT | `peer/router.rs` auto: Claude targets go over ClaudePeers (broker required) |
| f81cf361 | Hide unroutable peer sessions | PRESENT | router marks exited agents / missing worktrees unroutable; wrapper list filter identical |
| fc57e65b | Keep Claude/Codex scrollback in Dux | PRESENT | port 16a18590 pins the settled scroll defaults under auto mode |
| 16186dae | Translate forwarded scroll events | PRESENT | port 465282a4 wheel-tick translation |
| d88bb1ac | Allow unlimited retry watch rules | PRESENT | `watch/engine.rs` `is_unlimited` (zero budget = unlimited, test `zero_budget_means_unlimited`); examples in rendered config |
| 402f773a | Dux agents: auto-accept prompts | PRESENT | Net effect matches fork final state: wrapper uses the dev-channels path (claude-amq:296, per 3d520748) and codex trust handling (codex-amq:210-212) |
| 70cfd176 | Keep Claude Peers MCP out of temp installs | PRESENT | install.sh:328-335 `CLAUDE_PEERS_DIR` under STATE_ROOT |
| 3d520748 | WIP baseline: shutdown flag, dev-channels, docs dirs | PRESENT | dev-channels flag in claude-amq:296; engine shutdown paths (`shutdown_ptys`, reader exits); docs/audits per-audit dirs |
| 3bba0986 | audit03 P0-01: no config regen on rerun | PRESENT | install.sh regenerates only when config absent; provider-table-scoped sed; hermetic rerun test |
| 8aecbfe0 | audit03 P0-02: NOREPLACE reclaim | PRESENT | `queue.rs:309` reclaim with `RenameFlags::NOREPLACE` |
| be7bf48e | audit03 P0-03/04: purge fails closed | PRESENT | `purge.rs` strict-descendant containment + row retained on failure; port 80fa413b |
| 18a13536 | audit03 P0-05: exclude read hardening | PRESENT | `git.rs` `ensure_project_worktrees_link_ignored`: raw bytes, NotFound-only-empty, atomic tempfile, perms preserved |
| a387d005 | audit03 P0-06: bounded PTY teardown | PRESENT | `pty.rs` `PTY_DROP_HUP_GRACE`, HUP-first group signalling, reader detach; port b2182bdc |
| e8cfe851 | audit03 P0-07: reset-time overflow | PRESENT | `watch/reset_time.rs:73-82` `checked_add` + `try_from_secs_f64` |
| bebeb8a2 | audit03 P0-08: display-width truncation | PRESENT | render `display_width` truncation; port c6c1416a |
| 07f43dd2 | audit03 P0-09: hook trust default-on | PRESENT | codex-amq:210 `CODEX_AMQ_BYPASS_HOOK_TRUST` gate + warning |
| 3848c3d8 | audit03: threat model P0 updates | PRESENT | threat-model.md T1/T6/T13 long-form entries updated |
| 9332e885 | audit03 P1-13: bump anyhow/serial_test | PRESENT | Cargo.toml `anyhow = "1.0.103"`; advisory policy in deny.toml; port 938a0b22 |
| 830bc31b | audit03 P1-05: doctor sha direct | PRESENT | `dux-amq-doctor` `_TIMEOUT_BIN` wrapper (timeout passthrough) |
| 1da919d0 | audit03 P0-02: claim-time loss | PRESENT | `queue.rs:409` claim NOREPLACE + `.expired` quarantine; test `claim_fails_closed_when_the_inflight_destination_exists` |
| 101c1db5 | audit03 P0-06: no SIGKILL recycled pid | PRESENT | `pty.rs` reaped-pid guard + HUP -> grace -> KILL escalation; port b2182bdc |
| 15519969 | audit03 P0-03/04: purge-all malformed rows | PRESENT | purge-all continues on malformed rows with row-only plans, nonzero exit; port 80fa413b |
| a8a0ae4f | audit03 P1-05: doctor hash timeout | PRESENT | doctor.bats timeout passthrough case |
| 49f73d20 | audit03 P0-01: follow symlinks | PRESENT | install.sh:427 `sed -i --follow-symlinks` |
| e80151ad | audit03 P0-05/08: pin the fixes with tests | PRESENT | ports c6c1416a (render-level macro test) + a1658348 |
| 98f2b812 | audit03: threat model review rounds | PRESENT | threat-model.md claim/reclaim + purge-all durability notes |
| ec52425f | audit03 P0-02: NOREPLACE in release() | PRESENT | `queue.rs:590-597` release mirrors NOREPLACE; test `release_fails_closed_when_a_producer_recreated_the_original` |
| 6fb7b2bc | audit03 P1: transport + wrapper hardening | PRESENT | overlay scripts identical (STATE_ROOT propagation, `.unrouted` sentinel, DUX2 envelope, version floors) |
| 605ca48a | audit03 P1: CI/supply-chain + provenance | PRESENT | `root-installer.bats` + `supply-chain-rails.bats`; SHA256SUMS verification in install.sh; port 938a0b22 |
| 4b27a744 | audit03 P1: doctor read-only/JSON/redaction | PRESENT | doctor `--json`/`--anonymize`, read-only sqlite; purge target parity; ports f6c9737c, 80fa413b |
| 867b4357 | audit03 P1: atomic + fairness + quiet-window | PRESENT | single-transaction storage blocks; `queue.rs` `scan_cap` oldest-first fair; `first_pending_at`; u64::MAX generalized (delivery.rs:12) |
| 9c4c694f | audit03 P1: config defaults + atomic writes | PRESENT | `config_write.rs` temp+fsync+rename (+dir fsync, port 7fff0ed0); defaults reconciled |
| 24deeeee | audit03 P1: keybinding + diff + forking | PRESENT | ports a4ccbe27 (diff errors) + 4fb6a866 (subset shadowing) |
| 773a6b04 | audit03 P1: workers + rollback | PRESENT | ports 58f37a76, 45410bec, 19f4da49 (worker dispatch, config-save degradation, lifecycle rollback) |
| 1d8c98db | audit03 P1: threat model updates | PRESENT | SECURITY.md + threat-model.md (DUX2 auth, version floors, purge parity) |
| f2452e87 | audit03 P1-11: CLAUDE_PEERS_REV parse | PRESENT | `supply-chain-rails.bats` extracts the pin with grep -oE regardless of `${VAR:-...}` |
| 7d715a02 | audit03 P2: display-width sizing | PRESENT | `components/{button,checkbox}.rs` use renderer cell width; port c6c1416a |
| 5fc44720 | audit03: report + ledger | PRESENT | `docs/audits/audit03/` (8 files incl. 99-disposition.md) |
| 79ad41d3 | Spec: shared main-workspace mode | PRESENT | PLAN.md + PLAN-REVIEW-LOG.md at repo root; port 16036d0d adds user doc + spec + upgrade tests |
| 4bc947b8 | shared-workspace Phase 1: migration 0005 | PRESENT | `storage.rs` shared block (unique agent_handle, deleted_at, in-transaction backfill); `tests/upgrade_shared_workspace.rs`. Note: 5 fork migration-0005 test names lack same-named ports/dispositions (behaviour covered under new names) |
| ee8fd2ad | Phase 1: immutable agent_handle | PRESENT | `model.rs` `normalize_agent_handle` / `AGENT_HANDLE_MAX_LEN`; port b8859625 |
| 403b2a18 | Phase 1: tombstones + unique handle | PRESENT | `storage.rs` `deleted_at` + `load_sessions_including_deleted` + `assign_unique_agent_handle`; port b8859625 |
| cd3f21ae | Phase 1: interrupted Spawning -> Retryable | PRESENT | retryable lifecycle render + manual reconnect; dispositions in TEST_DISPOSITIONS-ram.tsv |
| 0d03ab13 | Phase 1: tests + fixtures | PRESENT | `tests/upgrade_shared_workspace.rs` + `tests/storage_identity.rs` |
| db7ada9d | log Phase 1 review | PRESENT | PLAN-REVIEW-LOG.md (present at root) |
| bd6e1074 | Phase 2: AMQ ownership | PRESENT | `peer/amq.rs` flock registry, `marker_state`, store_id, stale prune; port 5eee35fd |
| 5531378f | Phase 2: persist-first spawn | PRESENT | persist-before-launch spawn path; degrade-not-brick bootstrap; tests ported under fork names (71d7ad7d) |
| 8cfe2ee5 | Phase 2: purge targets agent_handle inbox | PRESENT | `purge.rs` handle-derived inbox targets; port 93b0e7d6 |
| 09982ad7 | Phase 2: wrapper locks + owner marker | PRESENT | wrappers flock `meta/config.lock` + atomic owner marker (files identical); `ownership.bats` |
| 8c3cd933 | Phase 2: ownership docs | PRESENT | `docs/operations/schema.md` ownership protocol section |
| 1a40d774 | log Phase 2 review | PRESENT | PLAN-REVIEW-LOG.md |
| b80085d0 | Phase 3: shared endpoints via AMQ | PRESENT | `peer/router.rs` shared endpoints force AMQ; `session_for_cwd` ambiguity hint |
| f8178755 | Phase 3: companion identity | PRESENT | `engine/companion.rs` DUX_* identity env; port 40bffc78 |
| ce708035 | log Phase 3 | PRESENT | PLAN-REVIEW-LOG.md |
| be131ef8 | Phase 4: [workspace] config | PRESENT | port 672e6a6d (`[workspace]` default_mode, consent-preserving absence sentinel) |
| 897fdc3d | Phase 4: live symbolic HEAD | PRESENT | `git.rs` live symbolic-ref query, detached-aware; port 170e93b4 |
| 4be59c35 | Phase 4: SharedWorkspace lifecycle | PRESENT | port 170e93b4 (create/register/branch/PR; shared never own worktree/branch) |
| 73c597ae | Phase 4: create modal + shared UI | PRESENT | port 08805b95 (handle relabel, shared badge, safe delete/rename) |
| 75e708bd | Phase 4: docs + T16 | PRESENT | README shared-workspace sections + SECURITY.md T16 |
| 8326d767 | log Phase 4 review | PRESENT | PLAN-REVIEW-LOG.md |
| f4f2257a | Phase 5: deletion guard | PRESENT | `git.rs` `guard_whole_workspace_removal` + `registered_project_paths` (fails closed); port 021ce84e |
| 454d8bca | Phase 5: reset fails closed | PRESENT | `cli.rs` reset aborts before mutation; port a3b15618 |
| 0b831554 | Phase 5: honest shared purge | PRESENT | `cli.rs` PURGE WORKSPACE / `--accept-residual-data`; exact-owner AMQ free; port 93b0e7d6 |
| 8cb42a21 | Phase 5: purge/reset docs | PRESENT | README purge semantics + threat-model non-Dux-history warning |
| 4d727da2 | log Phase 5 review | PRESENT | PLAN-REVIEW-LOG.md |
| d0ce0afc | Phase 6: consent + badge | PRESENT | port 08805b95 second-writer consent + live multi-writer badge |
| 5c9edb78 | Phase 6: orphan cleaner | PRESENT | `crates/dux-core/src/orphan_worktrees.rs` + TUI palette command; ports 80fa413b, b833ed43 |
| f42b3529 | Phase 6: docs cross-talk | PRESENT | README + threat-model cross-talk / current-store-only limitation |
| 826f28d3 | log Phase 6 | PRESENT | PLAN-REVIEW-LOG.md |
| 9990a0d3 | wake-termination test resilience | PRESENT | `peer/amq_tests.rs:717` `tombstone_releases_the_registry_lock_before_waiting_for_wake_exit` ported by name |
| ece934bd | log CI resolution | PRESENT | PLAN-REVIEW-LOG.md |
| 8eb821e6 | auto_resume_shared opt-in | PRESENT | `workspace.auto_resume_shared` (default false) in config + candidate filter; port 672e6a6d |
| 0a2c0371 | main-thread fork for auto-resume | PRESENT | port 7ec289f1 (spawn marshalled to the UI thread; macOS fork-safety) |
| a38187f3 | resume by provider session ID | PRESENT | `resume_recovery.rs` + `engine/provider_sessions.rs` + storage column; ports 7ec289f1, b7d0f3ad |
| 36568df7 | recover stale Claude targets | PRESENT | history recovery in resume_recovery; port 7ec289f1 |
| bc8673a7 | wait for Codex rollout | PRESENT | `parse_codex_rollout_identity`; port 7ec289f1 |
| 78923992 | route AMQ by agent handle | PRESENT | port c1eb3a3a (route by persisted handle, peer-like root resolution) |
| c69a09f0 | override stale orchestrator role | PRESENT | `engine/amq.rs` `orchestrator_policy_injected` map (cleared in forget_amq_session) |
| 3dd80427 | version stamp from git commit | PRESENT | `crates/dux-core/build.rs` `DUX_GIT_COMMIT` (+`-dirty`); `version.rs` `long()`; port 65e8903d |
| 3707af8c | configurable checkpoint prompt | PRESENT | `amq/orchestrator.rs:81` `resolve_orchestrator_checkpoint_prompt` + `amq/config.rs:110` `checkpoint_prompt` |
| c2c44378 | harness + PTY support (cline/kilo/ntl) | PRESENT | `config.rs:2339-2459` cline/kilocode/ntl/copilot defaults; port 139a26d6 (per-agent YOLO argv); docs 9a8d7d6e |
| d945e200 | prevent accidental editor launches | PRESENT | port 4fb6a866 (unbind plain `o`, subset-shadow detection) |
| 55ce75f3 | OpenCode scrolling during selection | PRESENT | port 465282a4 (`forward_mouse` policy, drags stay in dux) |
| e92a4dff | OpenCode clicks vs drags | PRESENT | port 465282a4 |
| 0e8efd57 | wake delivery restart-safe | PRESENT | wrappers `.wake.lock` + `amq wake recover-owner` (claude-amq:381-382); bridge restart handling; wrappers byte-identical |
| 6f01b99f | installer amq v0.61.0 | PRESENT | install.sh `AMQ_TAG v0.61.0` + pinned hashes |
| 22e6b1e3 | suppress auto-clear while busy | PRESENT | `engine/amq.rs:300` `amq_blocks_auto_clear` (cites 22e6b1e3; busy markers + pending mail + collaboration window) |
| 2bdd42ba | restore scrolling in Codex panes | PRESENT | port 465282a4 + config patch (dux-config.sed, validated by crates/dux/tests/overlay_config.rs) |
| bc3a9eec | restore copying from agent panes | PRESENT | port 465282a4; `bc3a9eec` cited in dux-config-changes.toml comment |
| 69766e06 | reject Claude bridge stubs on resume | PRESENT | resume_recovery bridge-stub rejection; port 7ec289f1 |
| 0befc8e9 | preserve selection after Escape | PRESENT | port c7de81c9; test `pending_escape_does_not_swallow_claude_mouse_selection` |
| 562419e0 | retire completed audit plans | PRESENT | integration mirrors fork final state: `docs/plans/` contains only rustport |
| 99c97081 | rustport plan set | PRESENT | `docs/plans/rustport/` 27 files incl. 8 research artifacts, with pre-workspace path preamble |
| 244b33c6 | release the fork's own code | PRESENT | release.yml `prepare-release` strips `dux-amq-`, rejects non-semver; `verify_fork_lineage.sh`; install.sh `DUX_REPO`/`dux-amq-v0.1.1` |
| 34ae3fa5 | clear yanked chacha20 + unsound lru | PRESENT | Cargo.lock chacha20 0.10.2, lru 0.18.5; deny policy port 938a0b22 |
| ef546f27 | rustport Phase 02 correction | PRESENT | rustport/02 + BRANCH_PROTECTION.md reflect the corrected (applied) branch-protection state |
| a8ea7e79 | drop cargo-edit | PRESENT | `.github/scripts/set_workspace_version.sh` (no cargo-edit); supply-chain-rails asserts its absence |
| 6e5a3a6c | macos-15-intel runner | PRESENT | release matrix uses `macos-latest` for both darwin targets; no retired labels (rails forbid macos-13); darwin-amd64 artifact still produced. Literal label differs; outcome preserved |
| 4f572aac | reproducible archives | PRESENT | release.yml:196 `--use-compress-program='gzip -n'` + epoch mtime; rails test |
| b2da48f6 | gh --repo without checkout | PRESENT | upload job sets `GH_REPO: github.repository` (equivalent of `--repo`) |
| a0f84a7f | progress off captured stdout | PRESENT | install.sh `log()` writes to stderr; port bd5d7eb7 |
| ed31f191 | resume parent codex thread | PRESENT | rollout identity prefers `session_id`, falls back to `id`; port 7ec289f1 |
| bc77466f | log reconnect failures | PRESENT | port bc366b5d (tracing::error on create/reconnect launch failures) |
| 8a4bfc41 | jcode default provider | PRESENT | `config.rs:2461` jcode entry (--no-update args, resume_by_id_args, install hint) |
| 9ffd8bf4 | jcode-amq wrapper + native delivery | PRESENT | `dux-amq/wrappers/jcode-amq`; bridge `DUX_AMQ_NATIVE_DELIVERY=1` path; SECURITY T20 |
| eb2d2eee | auto-resume jcode via metadata | PRESENT | jcode session-metadata resolver + native id shape gate; port 7ec289f1 |
| aaa7d388 | route to spawned pane (ancestry) | PRESENT | peer host-kind ranking; port 100b6f02 |
| 52850137 | jcode provider defaults fix | PRESENT | `config.rs` jcode `forward_scroll: Some(true)` + `--no-update` repeated in resume_by_id_args |
| 7b371221 | jcode by session + silence banner | PRESENT | bridge `clients:map` + `-S <session>` targeting; `AMQ_NO_UPDATE_CHECK=1` (line 5) |
| e4c1a91d | route to daemon-hosted session | PRESENT | ranking daemon-session > pane > client > bg-spare, newest then heartbeat; port 100b6f02 |
| 886dc187 | skip deleted sessions in reconciliation | PRESENT | peer AMQ sync skips tombstoned sessions; port 100b6f02 |
| 38963398 | wheel events without focus | PRESENT | port 465282a4 `forward_mouse` per provider + wheel translation |
| cdb39e2f | wheel test with Jcode replay | PRESENT | `crates/dux/tests/jcode_mouse_acceptance.rs`; port 69ee4a6b |
| 042ac638 | watch-rules palette modal | PRESENT | port 3ee4088e; `PromptState::WatchRules` + `render_watch_rules_prompt` + toggle |
| 5d4b0f94 | dux-side drainer | PRESENT | `crates/dux-core/src/amq/{queue,delivery}.rs` + `engine/amq.rs` drainer; `tests/amq_inject_integration.rs` |
| 696f4a8e | wait_until_capture + 5h resume | PRESENT | `watch/reset_time.rs` parsers + engine `wait_until_capture_*` tests |
| 7223f4d8 | generic watch engine | PRESENT | `crates/dux-core/src/watch/*`; port 5f9a180c; `tests/watch_engine_integration.rs` |
| d3e63e47 | diff trailing semicolon | PRESENT | folded into ported `crates/dux-core/src/diff.rs`; no behaviour |
| 33897148 | gitignore .claude/ | MISSING | `.claude/` absent from integration `.gitignore` (only `.agent-mail/`, `.superpowers/` etc.); `.claude/settings.local.json` shows as untracked noise. Cleanup artifacts were retired by 562419e0 anyway |
| 4935c47d | docs: audit02 landed changes | PRESENT | README (purge/doctor/env vars sections), CLAUDE.md, SECURITY.md, dux-amq README all carry the fork feature documentation |
| 411a59c2 | drain-with-timeout pty test | PRESENT | port ee603c11 (cites 411a59c2) |
| 98924474 | linux-only non-utf8 + STATE_ROOT check | PRESENT | `tests/git_portability.rs` linux-gated; install.sh STATE_ROOT-parent check |
| 24ff6c1e | toolchain input + pure-bash dirname | PRESENT | `toolchain: "1.88.0"` inputs in workflows; install.sh pure-bash dirname (file parity) |
| 6027480c | watch suppression window | PRESENT | `engine/watch_tick.rs` `suppress_until` map + skip window |
| 61b935d3 | auto-clear vs postscript sentinel | PRESENT | `watch/engine.rs` `rebaseline`; postscript uses `TASK_DONE_SENTINEL` |
| 04416e36 | rebaseline on window expiry | PRESENT | `watch_tick.rs:210-216` one-time rebaseline when suppression expires |
| e79bfbe5 | p2 hygiene bundle | PRESENT | CODEOWNERS (19 rules), dependabot.yml, storage warns, expand_path, release line tables (port da646187) |
| 44ead1ac | limits caps | PRESENT | port 9e2140bf `[limits]` guards (disk watermarks, scrollback cap, companion cap) |

## Verification method notes

- 108 of 229 fork shas are cited verbatim in integration port-commit messages
  (`git log --all --grep=<sha>`); each was still spot-checked against the named
  code, not taken on trust.
- `dux-amq/` diff vs fork main is limited to: README (path rewording), install.sh
  (fork-release pins + sed extraction), dux-config-changes.toml (documented end
  state + overlay_config.rs validation), inject-bridge (comment path + jcode
  native delivery), and bats hardening (`|| false`, richer fakes). All wrapper,
  script, vscode and config files are byte-identical.
- `.github/` and `docs/` diffs vs fork main are additive or path-updating only;
  nothing fork-specific was removed (BRANCH_PROTECTION.md, CODEOWNERS,
  dependabot, deny.toml, all four fork workflows' content, audits findings,
  rustport plan set, operations docs all present).
- Parity script re-run on the integration tree: 469 fork-only tests, 23 without
  same-named ports, 15 with verified dispositions, 8 unaccounted (all
  storage_migrations migration-0005 era; behaviour covered under new names, see
  summary item 3).

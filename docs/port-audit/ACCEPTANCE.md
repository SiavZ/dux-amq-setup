# Acceptance map: port of upstream dux into the fork, plus hot reload

Merged to `main`: PR #71 (`dcf82803`) and PR #72 (`b77c5d33`). Each requirement
below has its concrete check and the observed result, run on `main` at
`b77c5d33` unless stated otherwise.

| Requirement | Check | Observed |
|---|---|---|
| Upstream is in the fork | `git rev-list origin/main..upstream/server-mode` | 2 commits upstream made during the #72 merge (normal drift; everything up to f477065d is in) |
| Nothing the fork had is lost (commits) | Independent audit of all 229 fork commits (`COVERAGE_OUTCOME.md`) | 220 present, 4 dropped on purpose, 3 replaced by upstream code, 2 gaps closed in #71 |
| Nothing lost (tests) | `fork_test_parity.py` | 469 fork-only tests: present by name or a verified disposition; UNACCOUNTED 0 |
| Nothing lost (settings) | `config_parity.py` | 0 fork config fields missing (deliberate drops: oneshot_args, oneshot_output, schema_version) |
| Fork history kept | `git merge-base --is-ancestor 886dc187 origin/main` | reachable |
| Merged tree is the verified tree | tree hash of `origin/main` vs the verified PR head | identical (`798fc055` for #71, `05790429` for #72) |
| 8 default providers, gemini retired | `dux config regenerate` on the release binary | claude cline codex opencode kilocode ntl copilot jcode |
| Every setting documented in config | rendered config on the release binary | `[limits]`, `[storage]`, `[auto_resume]`, `[workspace]`, `[amq*]`, `max_concurrent_pr_checks`, `disk_high_water_pct`, `backup_interval_minutes`, `forward_mouse` all present with inline docs |
| Fork CLI surfaces | `dux --help` on the release binary | `peer` (send, list, sync-amq), `session` purge, `doctor`, `config reset --all` present |
| Overlay installer patches config correctly | `cargo test -p dux --test overlay_config` | 6/6 |
| Overlay shell suite | overlay-ci on `main` | bats 136/136 |
| **Hot reload keeps agents running** | `tools/reload-live-check.sh target/release/dux` (real TUI in tmux, real release binary, `reload-binary` from the palette) | PASS 6/6 runs: dux pid unchanged, agent pid unchanged and still dux's child, exactly one agent, adopted 1 of 1, session row `active`, handoff file removed, agent answers through the adopted pty |
| Reload quiesces AMQ before exec | `dux.log` of that run | `amq: quiesced for exec: AmqQuiesceReport { poll_thread_stopped: true, .. }` before `reload: adopted 1 of 1` |
| Reload leaks no descriptors | `lsof` on the reloaded dux | only its tty, the lock, log, sqlite (+wal/shm), and the adopted `ptmx` master with its reader/writer dups |
| Reload refuses safely | `~/.jcode/scratch/reload-refuse-check.sh` (real binary) | "Already running the newest dux on disk" and "Not reloading: ... is still working"; in both cases dux and agent pids unchanged, no exec, no handoff file |
| Tests pass on both platforms | CI on `main` after merge | Test (macos-14, now running `cargo test`: 6258 tests), Test (ubuntu-24.04), Security, overlay-ci, Website CI, Install script: all success |

## Known limits (not regressions)

- `dux-amq/scripts/finalize-claude-migration.sh` is a Linux VM script (`/data`,
  `mv -T`, flock). Its bats tests fail on macOS (bash 3.2 empty array under
  `set -u`, BSD `mv` has no `-T`); they pass on Linux CI. Same as on fork main
  before the port.
- A dux whose host terminal vanishes spins and ignores SIGTERM. A cold start
  does the same. Recorded in `RELOAD_DESIGN.md`.
- Reload: scrollback is not preserved; reload has no default key (palette
  only). Documented in `RELOAD_DESIGN.md`.

# Rustport baseline — captured 2026-08-31

Recorded at commit `HEAD` on branch `shared-auto-resume` (clean tree).

## Build health

| Gate | Command | Result |
|---|---|---|
| Format | `cargo fmt --all -- --check` | exit 0 (clean) |
| Lint | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 (clean) |

The repo starts green. Any warning introduced during the rustport is a
regression, not pre-existing noise.

## Toolchain drift (verified upgrade debt)

| | Version | Released |
|---|---|---|
| Pinned in `rust-toolchain.toml` | 1.88.0 | 2025-06-23 |
| Current stable | 1.98.0 | 2026-08-18 |

Ten minor releases / ~14 months behind. `rust-toolchain.toml` documents the
pin as deliberate ("bump deliberately when crates require it; do not switch
to `stable`"), and 1.88.0 was chosen as the max MSRV across the resolved
dependency graph at audit02 time. That rationale has expired: the constraint
was a floor, not a ceiling, and nothing re-evaluated it since.

Consequence for this port: every clippy lint added between 1.89 and 1.98 is
currently invisible. The toolchain bump must land **before** the
decomposition phases, so newly-written modules are linted by the same
compiler that CI will eventually use — otherwise the refactor ships against
a stale lint set and the bump later produces a second, larger cleanup.

## Source size census (the 500-line rule)

Total Rust in `src/`: **69,515 lines**. Files over 500 lines:

| File | Lines |
|---|---|
| `src/app/input.rs` | 13,266 |
| `src/app/render.rs` | 7,565 |
| `src/config.rs` | 4,952 |
| `src/app/sessions.rs` | 4,713 |
| `src/app/mod.rs` | 4,447 |
| `src/app/workers.rs` | 4,394 |
| `src/keybindings.rs` | 2,862 |
| `src/peer.rs` | 2,281 |
| `src/git.rs` | 2,236 |
| `src/pty.rs` | 2,161 |
| `src/cli.rs` | 1,991 |
| `src/app/text_input.rs` | 1,804 |
| `src/app/inject_runtime.rs` | 1,756 |
| `src/storage.rs` | 1,485 |
| `src/amq_inject.rs` | 1,311 |
| `src/purge.rs` | 1,276 |
| `src/resume_recovery.rs` | 1,260 |
| `src/watch/engine.rs` | 1,002 |
| `src/diff.rs` | 985 |
| `src/theme.rs` | 976 |
| `src/model.rs` | 887 |
| `src/raw_input.rs` | 763 |

Plus `tests/purge_integration.rs` (937) and `tests/storage_migrations.rs` (662).

## Bash surface to port (~3,600 lines)

| Path | Lines |
|---|---|
| `dux-amq/scripts/dux-amq-doctor` | 850 |
| `dux-amq/install.sh` | 565 |
| `dux-amq/wrappers/claude-amq` | 393 |
| `dux-amq/scripts/dux-amq-inject-bridge` | 307 |
| `dux-amq/wrappers/codex-amq` | 242 |
| `dux-amq/wrappers/gemini-amq` | 226 |
| `dux-amq/scripts/finalize-claude-migration.sh` | 149 |
| `dux-amq/scripts/amq-receive-verify` | 113 |
| `dux-amq/scripts/install-gocryptfs.sh` | 111 |
| `dux-amq/scripts/amq-send-signed` | 93 |
| `dux-amq/scripts/encode-claude-project-dir` | 93 |
| `dux-amq/config/bashrc-additions.sh` | 69 |
| `dux-amq/scripts/amq-secret-init.sh` | 41 |
| `install.sh` (root, release fetcher) | ~150 |

Target crate directory `dux-amq-rust/` already exists and is empty.
Behavior is currently pinned by 15 `.bats` suites under `dux-amq/tests/`.

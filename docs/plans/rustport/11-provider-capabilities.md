# Phase 11 — Provider capabilities: make "any CLI tool can be a provider" true

**Track:** C (correctness & resources) · **Parallel-safe with:** 08, 09, 10
**Depends on:** 07 (config property tests protect the schema change) · **Blocks:** 22

## Goal

Retire 7 of the 12 sites that branch on provider name, and close the compliance gap
where a user-added provider's chat history is never purged — **without** introducing a
trait, an adapter, or a protocol layer.

## Evidence

`CLAUDE.md` states: *"Any CLI tool can be a provider. Configure `command` in
`config.toml` and dux spawns it. No adapters, no protocol layer. Adding a new provider
is a config-only change, not a code change."*

`ProviderKind` (`model.rs:42-58`) is correctly a thin, unvalidated newtype over
`String`. The problem is entirely in callers. **12 production sites** (test fixtures
excluded):

| # | Site | Condition | What it changes |
|---|---|---|---|
| P1 | `watch/builtin.rs:90-97` | `match provider.as_str()` → `"claude"=>"/clear"`, `"codex"=>"/new"`, `"gemini"=>"/clear"`, `"opencode"=>"/clear"`, `_=>"/clear"` | The context-wipe slash command. **No config key exists.** Consumed at `app/mod.rs:3362`. Highest-value single fix |
| P2 | `model.rs:642-668` | `to_pty_env` matches 4 names when `yolo_permissions` set | Emits `CLAUDE_AMQ_YOLO` / `CODEX_AMQ_YOLO` |
| P3 | `app/sessions.rs:694-696` | `provider == "codex" && matches!(launch, ResumeId(_))` | Appends `-C <cwd>` to resume args |
| P4 | `app/sessions.rs:697-702` | `provider == "opencode" && yolo_permissions` | Appends `--auto` |
| P5 | `app/sessions.rs:649` | `provider != "claude"` in `should_resume_session` | Claude-only guard → `resume_recovery::claude_resume_target_exists` |
| P6 | `app/orchestrator.rs:84-86` | `!provider.eq_ignore_ascii_case("claude")` | Every non-Claude provider gets a PTY orchestrator policy |
| P7 | `app/inject_runtime.rs:1206-1215` | `eq_ignore_ascii_case("claude") \|\| ("codex")` | Bracketed-paste bytes vs `macro_payload_bytes` |
| P8 | `resume_recovery.rs:68`, `:74-99` | `!matches!(.., "claude" \| "codex")` then a per-name `match` | Session-ID capture strategy, backed by `enum FreshCapture { None, Claude{..}, Codex(..) }` (`:40-47`) — **a provider adapter in all but name** |
| P9 | `resume_recovery.rs:425`, `:493` | `identity.provider == "claude"` | Gates stranded-history recovery |
| P10 | `app/workers.rs:1896` | `matches!(.., "claude" \| "codex")` | Gates `dispatch_fresh_launch_preparation` |
| P11 | `app/workers.rs:2020` (+`:2179,2204,2212`) | `provider == "codex"` | Codex-specific warning text and session-ID persistence |
| P12 | `config.rs:2964` | `if name == "claude"` **inside the template renderer** | Only Claude gets the 4 worked watch-rule examples; the other 7 providers get a 3-line blurb |

**The compliance gap:** `purge.rs:132-134` and `:561-564` hardcode provider state
directories for four names. **A user-added provider's chat history is never purged by
`dux session purge`.** That is a completeness defect in a GDPR feature, not a style issue.

**Verified clean** (no production provider branching — do not re-audit):
`pty.rs`, `provider.rs`, `amq_inject.rs`, `auto_resume.rs`, `statusline.rs`,
`orphan_worktrees.rs`, `storage.rs`, `diff.rs`, `theme.rs`, `keybindings.rs`.

## In scope

The capability block, conversion of P1–P4, P6, P7, P12, and the `purge.rs` state
directories.

## Out of scope

- **P5, P8, P9, P10, P11 — the session-capture cluster → Phase 22.** These are not flag
  differences; they encode genuinely different *algorithms*. Claude: dux generates a
  UUID and injects `--session-id`, then verifies the resume target exists
  (`resume_recovery.rs:80-92`, `sessions.rs:649`). Codex: dux watches the provider's own
  rollout files and parses them to discover the id after the fact
  (`resume_recovery.rs:94-96, 284-330`), coordinated through a global mutex
  (`CodexCaptureCoordinator` `:101-105`) because concurrent launches in one workspace are
  ambiguous. **No config schema expresses "parse this provider's rollout files."**
- `peer.rs:461-478` (`is_claude_session` / `ensure_claude_peers_target`) — Claude Peers
  is a Claude-specific IPC transport, not a provider behaviour. Cleaner as a
  `supports_claude_peers` flag, but not the same violation. Revisit in Phase 22.
- `config.rs:104-149` migration arms — frozen by the append-only schema policy. **Do not
  edit.**

## Design

Widen `ProviderCommandConfig` (`config.rs:275-295`). No trait, no adapter binary, no
protocol layer — only more config data.

| Field | Type | Default | Retires |
|---|---|---|---|
| `clear_command` | `Option<String>` | `Some("/clear")` | P1 |
| `yolo_args` | `Vec<String>` | `[]` | P4 |
| `yolo_env` | `Vec<(String, String)>` | `[]` | P2 |
| `paste_mode` | `PasteMode` (`bracketed`\|`literal`) | `Literal` | P7 |
| `orchestrator_policy` | `OrchestratorPolicy` (`native`\|`pty`) | `Pty` | P6 |
| `state_dirs` | `Vec<String>` | `[]` | `purge.rs:132-134, 561-564` |
| `resume_cwd_flag` | `Option<String>` | `None` | P3 |

Shape notes:
- `clear_command` as `Option` lets a provider declare it has **no** context-wipe
  command — which `watch/builtin.rs:86-89` currently cannot express (its comment admits
  an unsupported provider "still fires the rule" and prints a help message).
- `yolo_env` as pairs, not a bool, keeps `to_pty_env` (`model.rs:637-668`) a single loop
  with no match.
- `resume_cwd_flag: Some("-C")` turns P3 into `args.extend([flag, cwd])`.
- `state_dirs` holds path templates resolved against `$STATE_ROOT` / `$XDG_DATA_HOME`,
  so `purge.rs` iterates config instead of a hardcoded triple.

## Work items

1. **Add the seven fields** with backward-compatible defaults, plus a `migrate_config`
   arm `4 → 5` using the same `uses_legacy_defaults` guard pattern as arms 2→3 and 3→4,
   so hand-customised provider entries are left alone. Defaults must be chosen so
   **existing configs behave identically**: `paste_mode` → `Literal`,
   `orchestrator_policy` → `Pty`, with the stock `claude` and `codex` entries in
   `default_provider_commands()` (`config.rs:2677-2856`) carrying the non-default values
   explicitly.
2. **Document every new key** in `config_schema()` — the tenet is 53/53 and Phase 07
   now enforces it mechanically.
3. **Convert the pure lookups: P1, P2, P4, P6, P7.** Mechanical, low risk.
4. **Convert P3** (`resume_cwd_flag`) — touches launch args, so cover with tests.
5. **Convert the `purge.rs` state directories.** This is the compliance fix: iterate
   `state_dirs` from config. `tests/purge_integration.rs` is genuinely strong (22 tests
   — symlink escapes both directions, foreign-inbox protection, per-category
   retry-after-failure, documented ordering pinned); extend it with a **user-added
   provider whose history must be purged**.
6. **Retire P12 indirectly.** Once watch-rule behaviour is expressed in capability
   fields, the renderer no longer needs a Claude-only example block
   (`config.rs:2964-3035`, ~75 lines). Extract the examples to an `include_str!`
   fragment and emit provider-neutral documentation. **This also brings
   `template/providers.rs` from ~380 to ~300 lines**, helping Phase 12's budget.
7. **Fix the `WatchRule` documentation gap.** Sub-fields are documented only through the
   Claude-only examples, so a user writing a Codex watch rule must read
   `src/watch/rule.rs` — leaving the config file, which is what the tenet forbids.
8. **Add a "config-only provider" integration test.** Define a fake provider entirely in
   `config.toml` — a shell script that echoes — and assert it spawns, its clear command
   fires, its yolo env is set, its paste mode is honoured, and **its state dirs are
   purged**. This is the executable form of the tenet and the regression guard for all
   future work.

## Acceptance criteria

- [ ] Seven capability fields added, defaulted, and documented in `config_schema()`.
- [ ] `migrate_config` arm 4→5 present, guarded, and tested (Phase 07 pattern).
- [ ] Existing configs produce byte-identical behaviour — verify with a
      before/after `to_pty_env` and launch-argv comparison for all four stock providers.
- [ ] P1, P2, P3, P4, P6, P7, P12 no longer branch on provider name.
- [ ] `purge.rs:132-134` and `:561-564` iterate config; **a user-added provider's state
      directory is purged**, proven by test.
- [ ] `grep -rn 'as_str() == "claude"\|== "codex"\|eq_ignore_ascii_case("claude")' src/`
      returns only the Phase 22 cluster (P5, P8–P11) and `peer.rs:461-478`.
- [ ] Config-only fake-provider integration test passes.
- [ ] `WatchRule` fields documented in the config file itself.
- [ ] `cargo test --all-features` green.

## Validation

```bash
cargo test --all-features
cargo test --test purge_integration
cargo test --test limits

# tenet check: the remaining hits must be exactly the Phase 22 cluster
grep -rn 'provider.*==\s*"' src/ --include='*.rs' | grep -v '#\[cfg(test)\]'
```

## Risks

| Risk | Mitigation |
|---|---|
| Defaults change behaviour for existing users | Stock entries carry non-default values explicitly; the migration arm only touches legacy defaults. Prove with an argv/env comparison, not by inspection |
| `state_dirs` misconfigured → purge deletes the wrong path | `purge.rs`'s existing symlink-escape guards are strong and tested both directions; resolve templates through the same guards and add a traversal test |
| Widening the schema collides with Phase 12's split | Phase 07 gates both; land 11 before 12 starts, or rebase 12 onto it — the field additions are additive |
| Someone "finishes the job" by also converting P5/P8–P11 | Explicitly out of scope here; those need a `SessionCapture` strategy (Phase 22), not a config field |

## References

- `artifacts/research-config-keys-theme-cli.md` §3.1, §6 (capabilities proposal)
- `artifacts/research-runtime-memory.md` §1.4 (resume/purge flow)

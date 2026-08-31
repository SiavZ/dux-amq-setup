# Phase 22 — Extract the session-capture strategy

**Track:** E (serialized) · **Runs alone**
**Depends on:** 11, 15 · **Blocks:** 24

## Goal

Retire the last five provider-name branches — the ones Phase 11 deliberately could not
touch — by extracting a named `SessionCapture` strategy, so that "adding a provider is a
config-only change" becomes true without pretending different algorithms are the same
algorithm.

## Why these five resisted the config approach

Phase 11 converted 7 of 12 sites into capability fields. The remaining five encode
**genuinely different algorithms**, not flag differences:

- **Claude:** dux generates a UUID, injects it via `--session-id`, then verifies the
  resume target exists (`resume_recovery.rs:80-92`, `app/sessions.rs:649`).
- **Codex:** dux watches the provider's **own rollout files** and parses them to discover
  the session id *after the fact* (`resume_recovery.rs:94-96, 284-330`), coordinated
  through a global mutex (`CodexCaptureCoordinator`, `:101-105`) because concurrent
  launches in one workspace are ambiguous.

**No config schema expresses "parse this provider's rollout files."** A `capture_mode`
enum with a strategy behind it is the honest modelling.

| # | Site | What it gates |
|---|---|---|
| P5 | `app/sessions.rs:649` | `provider != "claude"` inside `should_resume_session` → `claude_resume_target_exists`. This is the "reject Claude bridge stubs on resume" fix |
| P8 | `resume_recovery.rs:68`, `:74-99` | Capture strategy selection, backed by `enum FreshCapture { None, Claude{..}, Codex(CodexCapture) }` (`:40-47`) — **a provider adapter in all but name** |
| P9 | `resume_recovery.rs:425`, `:493` | Stranded-history recovery gating |
| P10 | `app/workers.rs:1896` | Gates `dispatch_fresh_launch_preparation` |
| P11 | `app/workers.rs:2020` (+`:2179, 2204, 2212`) | Codex-specific warning text and session-ID persistence |

## Design

Introduce `capture_mode` as a provider capability with three variants, and a
`SessionCapture` trait (or enum-dispatch — prefer the enum, since the set is closed and
config-selected):

| `capture_mode` | Behaviour |
|---|---|
| `none` (default) | No session-id capture; resume is unavailable. Every provider that is not Claude or Codex today. |
| `injected_uuid { flag: String }` | dux mints a UUID and passes it via the named flag; resume verifies the target exists. Claude: `flag = "--session-id"`. |
| `rollout_scan { dir: String, pattern: String }` | dux scans the provider's own state files after launch, coordinated by a mutex. Codex. |

This keeps the tenet honest: it is **not** a protocol layer, an adapter binary, or a
per-provider code path — it is a closed, config-selected strategy set, and the config
file documents which providers use which.

## Work items

1. **Add `capture_mode` to `ProviderCommandConfig`**, defaulting to `none`, with the stock
   `claude` and `codex` entries carrying their real values in
   `default_provider_commands()`. Add a `migrate_config` arm following the
   `uses_legacy_defaults` guard pattern. Document it in `config_schema()` — Phase 07
   enforces coverage mechanically.
2. **Rename and generalize `FreshCapture`** (`resume_recovery.rs:40-47`). It is already
   the right shape; it is simply named for the two providers that exist. Make its variants
   correspond to `capture_mode`, not to provider names.
3. **Convert P8** — strategy selection reads config rather than matching `as_str()`.
4. **Convert P5** — `should_resume_session` (`app/sessions.rs:649`) asks the strategy
   whether a resume target must be verified, rather than asking whether the provider is
   Claude.
5. **Convert P9, P10, P11.** P11's Codex-specific warning text becomes strategy-owned
   text; the session-ID persistence path becomes strategy-agnostic.
6. **Keep `CodexCaptureCoordinator`'s mutex.** The ambiguity it resolves — concurrent
   launches in one workspace producing indistinguishable rollout files — is real and
   belongs to the `rollout_scan` strategy, not to Codex the name.
7. **Decide `peer.rs:461-478`.** `is_claude_session` / `ensure_claude_peers_target` gates
   the Claude Peers transport. This is arguably legitimate — Claude Peers **is** a
   Claude-specific IPC mechanism, not a provider behaviour. **Recommendation:** convert to
   a `supports_claude_peers` capability flag for consistency, and note in `SECURITY.md`
   that enabling it for another provider is unsupported. Either way, **record the decision
   rather than leaving it as the one unexplained survivor.**
8. **Collapse the duplicate path encoders and handle normalisers.** The research flagged
   **three path encoders and two handle normalisers** across the tree. Phase 15 moved
   `sanitise_handle` to one home and Phase 18 lifted the encoder into `dux-amq-rust`;
   finish the job here so the count is one each. **This is the moment — otherwise the port
   adds a fourth encoder and a third normaliser.**
9. **Write the config-only-provider resume test.** Extend Phase 11's fake-provider test to
   declare `capture_mode = "injected_uuid"` and prove resume works for a provider defined
   entirely in `config.toml`. That is the executable proof the tenet now holds.

## Acceptance criteria

- [ ] `capture_mode` exists as a documented capability with a guarded migration arm.
- [ ] P5, P8, P9, P10, P11 no longer branch on provider name.
- [ ] `grep -rn 'as_str() == "claude"\|== "codex"\|eq_ignore_ascii_case("claude")' src/`
      returns **nothing** outside test fixtures and the recorded `peer.rs` decision.
- [ ] Claude and Codex resume behaviour is **unchanged** — verified by
      `tests/auto_resume.rs`, the resume-recovery suite, and a manual resume of each.
- [ ] `CodexCaptureCoordinator`'s mutex still serializes concurrent same-workspace launches.
- [ ] Exactly one path encoder and one handle normaliser exist in the workspace.
- [ ] A provider defined only in `config.toml` can resume, proven by test.
- [ ] The `peer.rs:461-478` decision is recorded in code and in `SECURITY.md`.
- [ ] `cargo test --workspace --all-features` green.

## Validation

```bash
cargo test --workspace --all-features
cargo test --test auto_resume

# the tenet, mechanically
grep -rn 'provider.*==\s*"' src/ --include='*.rs' | grep -v '_tests.rs'
# expect: only the recorded peer.rs decision

# duplicate-implementation check
grep -rn 'fn .*encode.*project_dir\|fn sanitise_handle\|fn normalize_agent_handle' \
  src/ dux-amq-rust/src/
# expect: exactly one of each

# manual: resume a Claude session and a Codex session; both must restore the real conversation
```

## Risks

| Risk | Mitigation |
|---|---|
| Resume is the feature users notice breaking first | `RESUME-PLAN.md` documents the per-agent resume design; re-read it before changing selection logic. Manual verification of both providers is an acceptance criterion, not optional |
| `rollout_scan` config makes a fragile path user-editable | Keep the stock Codex entry authoritative and document that changing it is unsupported; validate the directory exists at load and warn if not |
| The strategy enum becomes a de-facto adapter layer | It is a closed, config-selected set with no dynamic loading and no protocol — record that boundary in `CLAUDE.md` so a future contributor does not extend it into one |
| Collapsing encoders changes an encoding by one byte | `encoder-fixtures.bats` pins 12 recorded pairs from a live Claude install; `purge_encoding` has its own tests. Both must stay green |

## References

- `artifacts/research-config-keys-theme-cli.md` §3.1 (P5, P8–P11), §6 ("What this does NOT retire")
- `artifacts/research-debt-ci-security.md` §What the Rust port changes (A17, A18)
- `RESUME-PLAN.md`

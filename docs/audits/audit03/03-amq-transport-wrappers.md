# AMQ transport and provider wrappers

This subsystem covers handle derivation/claiming, wrapper launch policy, wake/bridge routing, and the optional authenticated envelope. It evaluates security against `SECURITY.md` and `docs/operations/threat-model.md`: one human/UID is trusted, but malicious repositories and agent output are not. AMQ binary internals remain out of scope; only the documented wrapper interface is considered. Baseline is `3d52074`.

## P0-09 — The Codex wrapper disables hook trust review by default

**Impact.** Enabled new or changed Codex hooks can run without the upstream review boundary on every Dux-launched Codex session. A malicious repository is explicitly in threat T1, and hooks can execute commands outside the agent sandbox.

**Baseline evidence.** `3d52074 — dux-amq/wrappers/codex-amq:169-200 — Codex launch arguments`: approval/sandbox bypass is correctly opt-in at lines 173-177, but line 189 unconditionally adds `--dangerously-bypass-hook-trust`. `3d52074 — SECURITY.md:49-58 — T1` requires default-deny for malicious-repository command execution. `3d52074 — docs/operations/threat-model.md:17-47 — T1` says dangerous flags are removed by default and only knowingly opted into.

**Upstream contract.** Current official OpenAI Codex source, [`codex-rs/core/src/config/mod.rs`](https://github.com/openai/codex/blob/main/codex-rs/core/src/config/mod.rs), records a startup warning for this flag: enabled hooks may run without review for that invocation. The upstream issue and fix history, [`openai/codex#24093`](https://github.com/openai/codex/issues/24093), independently shows the normal review screen states that hooks can run outside the sandbox after trust, and that this flag is intended to suppress that review. These are interface semantics, not an audit of Codex internals.

**Adversarial verification.** This flag is distinct from `--dangerously-bypass-approvals-and-sandbox`; the wrapper's YOLO default is therefore not a countermeasure. Some Codex versions 0.131.0–0.133.0 reportedly failed to honor the flag in TUI mode, but that was fixed upstream and the wrapper accepts any `codex` found on `PATH`. Hooks must also be enabled/discovered; the finding is the unconditional removal of their purpose-built trust gate when they are. That recreates a documented threat-model exploit boundary and remains P0.

**Required proof direction.** Remove the flag by default, expose an explicit narrowly named opt-in if headless workflows require it, and test launch argv in both states.

## P1-02 — Wrapper identity claiming has a check-then-create race

**Impact.** Two concurrently starting worktrees whose branch names normalize to one handle can both proceed, with the last symlink winner determining the recorded identity while both share AMQ state.

**Baseline evidence.** `3d52074 — dux-amq/wrappers/claude-amq:174-216 — identity collision detection` checks any existing registration, then calls `register_amq_handle`, removes the registration, and runs `ln -s ... || true`. The comment at lines 211-214 calls the gap harmless, but neither wrapper rechecks ownership after `ln`. `3d52074 — dux-amq/wrappers/codex-amq:61-90` and `dux-amq/wrappers/gemini-amq:63-91` replicate the sequence. If both see absence, A can create, B can remove A's link and create its own, and both launch because failures are ignored.

**Adversarial verification.** POSIX symlink creation is atomic, but the preceding unconditional `rm` destroys the exclusivity the comment relies on. Existing Bats coverage is sequential; it proves collision detection after one wrapper has registered, not simultaneous first claim. Same-UID malicious interference is out of scope, but ordinary concurrent pane startup is not. P1.

## P1-03 — `_unrouted` is both a valid handle and a reserved routing sentinel

**Impact.** A legitimate branch/explicit handle named `_unrouted` is routed to the currently selected session instead of its matching session.

**Baseline evidence.** `3d52074 — dux-amq/scripts/dux-amq-inject-bridge:57-67 — queue layout` claims a leading underscore cannot survive sanitization and therefore reserves `_unrouted`. The actual shell sanitizer at lines 159-169 permits `_`; `3d52074 — src/sanitize.rs:58-78 — amq_handle` also explicitly preserves `_`, including at the beginning. `3d52074 — src/app/inject_runtime.rs:546-554 — receiver resolution` treats the exact string `_unrouted` as the selected-session fallback before normal handle lookup.

**Adversarial verification.** Leading dashes are stripped, not underscores. Neither wrapper rejects or remaps the reserved value, and AMQ handles are documented as accepting underscore. The failure needs no adversarial peer and is reproducible with a supported branch name. P1.

## P1-04 — Strict HMAC mode does not provide a coherent authenticated-delivery boundary

**Impact.** The opt-in strict mode rejects valid signed multiline bodies, permits two concurrent verifiers to accept one nonce, does not bind the signed recipient to the receiving identity, and can replace the verified wake body with unverified co-queued drain output.

**Baseline evidence.** The broken edges form one advertised end-to-end contract:

- `3d52074 — dux-amq/scripts/amq-send-signed:60-68,86-105 — sender`: tabs are rejected but newlines are accepted and embedded in the signed envelope.
- `3d52074 — dux-amq/scripts/amq-receive-verify:63-100 — input/parser`: `read` consumes only the first line, so any valid multiline body is truncated before MAC recomputation and deterministically rejected.
- `3d52074 — dux-amq/scripts/amq-receive-verify:99-158 — verification`: the signed `RECEIVER` field is included in the MAC but never compared with actual `$AM_ME`/expected receiver.
- `3d52074 — dux-amq/scripts/amq-receive-verify:138-164 — nonce dedup`: `grep` and append are separate unlocked operations; concurrent processes can both observe absence and both authenticate the replay.
- `3d52074 — dux-amq/scripts/dux-amq-inject-bridge:94-110,195-213 — strict bridge`: the bridge verifies the wake argv, then under Dux drains up to 20 arbitrary queued messages and replaces `send_body` with that output. Verification therefore authenticates a trigger, not the body ultimately typed. It also redirects verifier stderr to `/dev/null` at line 105 while the next comment claims it was logged.

**Adversarial verification.** The default threat model accepts same-UID AMQ peer spoofing, so these do not qualify as P0 in the standard deployment. Strict mode is explicitly retained for deployments that cross a trust boundary, however, and each failure is a concrete shell-level reproduction rather than a cryptographic-strength complaint. A per-VM shared key cannot prove distinct same-UID principals, but it can and should uphold its stated message-integrity, recipient, and replay contract. P1.

**Required proof direction.** Define one byte-safe envelope, bind expected receiver, lock/check-and-record nonce atomically, and ensure the exact verified bytes are the exact queued/typed bytes. Tests must include multiline content, wrong recipient, simultaneous replay, and mixed queued bodies.

## Threat-model notes that were deliberately not findings

- Default skip-verification for AMQ peers is an explicitly accepted same-UID risk (T2), not a vulnerability to relabel as security theater.
- AMQ's internal queue/database implementation is out of scope. Findings stop at arguments, environment, files, and stdout/exit behavior consumed by these wrappers.
- Opt-in YOLO flags for Claude/Codex are consistent with T1 when disabled by default; the problem is the separate unconditional hook-trust bypass.

## Subsystem conclusion

The wrapper layer is intentionally simple and mostly shell-native. It does not need a protocol framework. It does need atomic ownership, an unambiguous sentinel, one end-to-end definition of authenticated bytes, and default preservation of provider trust prompts.

# Phase 19 — Port the wrappers and the inject bridge

**Track:** E (serialized) · **Runs alone**
**Depends on:** 18 · **Blocks:** 20, 23

## Goal

Port the three provider wrappers (861 lines) and the inject bridge (307 lines) to Rust
with **behavioural parity proven against the bats suite**, not asserted.

## Why this is the riskiest phase in the plan

These four scripts are the load-bearing interface between dux, the provider CLIs, and
AMQ. They hold the identity/registration protocol, the wake/coop handshake, the version
gate, and the message delivery path. **~60 of the 123 bats tests assert on recorded argv
from a PATH-shimmed fake `amq` binary**, and dozens of error strings are pinned verbatim.

The complete behavioural specs are in `artifacts/research-bash-port.md` §1 and §3 —
precise enough to reimplement without reading the bash. The complete acceptance criteria
are in `artifacts/bats-test-parity-matrix.md`. **This phase does not restate them; it
implements them.**

## The dispatch hazard that shapes everything

Every wrapper test works because `setup_isolated_home` puts `tests/fakes/amq` ahead of
the real binary on `$PATH` and the wrapper does a literal `exec amq coop exec …`.

**If the Rust port reimplements `amq coop exec`'s job in-process, the entire
`AMQ_FAKE_ARGV_FILE` recording mechanism has nothing to hook**, and ~60 argv-assertion
tests become untestable as written.

**Decision: keep the `exec amq …` subprocess boundary.** The wrappers' job is to compute
an argv and hand off; that is genuinely what they do, and preserving it keeps 60 tests
meaningful. Additionally, expose the argv computation as a `pub fn build_launch_argv(..)`
so it is unit-testable in-process **as well**.

## Module tree

```text
dux-amq-rust/src/
  wrapper/
    mod.rs             ~90   Provider enum {Claude, Codex, Gemini}; run(provider, argv)
    version_gate.rs   ~140   version_at_least + require_provider_version
                             (first-semver-substring-wins, `|| true` tolerance)
    identity.rs       ~200   managed-vs-standalone chain, is_dux_worktree, normalization
    registry.rs       ~340   claim_and_register_locked equivalent: agent dir,
                             polymorphic `.dux-amq-source` marker, meta/config.json merge
    transport.rs      ~190   INJECT_MODE, BRIDGE resolution, WAKE_ARGS, recover-owner gate
    seed.rs           ~210   Claude-only session-history seeding (rsync/cp fallback)
    launch.rs         ~240   per-provider argv assembly + exec into `amq coop exec`
  bridge/
    mod.rs             ~90
    startup.rs        ~160   5s backlog poll: kill(0) liveness + .wake.lock/.wake.prepared
                             generation equality
    envelope.rs       ~210   DUX2 / DUX1 / raw decode, incl. the permissive fallbacks
    drain.rs          ~180   under-dux `amq drain --json` substitution + count==0 handling
    deliver.rs        ~200   tmux vs queue decision tree
    queue.rs          ~170   `<ts>-<suffix>.msg` naming, `%s%N` BSD fallback, atomic write
```

## Work items

1. **Port `wrapper/version_gate.rs` first** — smallest, fully specified, and the exact
   strings are pinned: `"upgrade to >= 2.1.163"` (claude), `"upgrade to >= 0.39.0"`
   (codex), `"upgrade to >= 0.39.1"` (gemini), and `"version could not be parsed"`.
   Preserve first-semver-substring-wins matching and the `|| true` tolerance.
2. **Port `wrapper/identity.rs`.** The managed tri-var bundle
   (`DUX_STORE_ID` + `DUX_SESSION_ID` + `DUX_AMQ_HANDLE`) is **all-or-nothing** —
   partial env fails closed with `"must be set together"`. A managed handle is
   **validated verbatim, never repaired** — `'Bad/Handle'` yields
   `"invalid or overlong AMQ handle"` and creates no directory. Managed identity beats
   an inherited `AM_ME`.
   - `is_dux_worktree` must **reject a sibling `/worktrees-evil` prefix** (audit01 P0-5)
     and reject when `DUX_HOME` does not exist.
   - **Identity priorities #2–#4 are effectively untested today** — the
     `basename "$PWD"` path is only tested at raw-function level, and the `git
     symbolic-ref` fallback and the final `claude-$$`/`codex-$$`/`gemini-$$` PID fallback
     are **never exercised by any test**. Write the missing tests as part of the port.
3. **Port `wrapper/registry.rs`.** The marker is JSON with exactly `store_id` and
   `session_id` — **no legacy `wake_pid` field** — and the agent dir is mode **0700**.
   A foreign managed owner is **preserved, never overwritten**. Identity collision from a
   different `$PWD` fails with `"identity collision"` and leaves the marker untouched,
   **even when the marker is a broken symlink**. Same handle from the same `$PWD` is
   idempotent. Concurrent launches yield **exactly one winner and one loser**.
   - **Missing flock is a hard failure**: `"mandatory flock utility is unavailable"`
     before any registration write. Match `ownership.bats:49`.
   - Use `common/locks.rs` from Phase 18 so semantics match `peer.rs` exactly.
4. **Port `wrapper/transport.rs`.** Default is managed raw wake: argv contains
   `--require-wake`, `-y`, `--wake-inject-mode`, `raw`, and **not** `--no-wake`.
   `DUX_AMQ_INJECT_MODE=via` adds `--wake-inject-via <bridge abs path>`.
   A stale exact owner triggers `amq wake recover-owner` **before** `coop exec`; a
   refused recovery **propagates the exact exit code** (42 in the fake) and never reaches
   launch. An **ownerless legacy lock is left for coop's own `-y` migration** — no
   `recover-owner` call.
5. **Port `wrapper/seed.rs`** — Claude-only, **off by default**, enabled by
   `CLAUDE_AMQ_SEED_FROM_PARENT=1`. A warning-producing rsync must report
   `"rsync warnings"`; a clean one keeps `"seeded N past sessions"`.
6. **Port `wrapper/launch.rs`.** YOLO is **opt-in, never default**, for every provider.
   Claude: `CLAUDE_AMQ_YOLO=1` or legacy `CLAUDE_YOLO=1`; `CLAUDE_AMQ_SAFE` **only warns**
   (`"CLAUDE_AMQ_SAFE is deprecated"`) and changes nothing. Claude Peers dev-channel
   flags are **on by default**, suppressed by `CLAUDE_PEERS_DISABLE=1`. Codex: YOLO drives
   **only** the sandbox-bypass flag; hook-trust bypass needs its own
   `CODEX_AMQ_BYPASS_HOOK_TRUST=1` and prints `"hook trust review bypass enabled"`.
   - `DUX_SYSTEM_PROMPT`: claude emits `--append-system-prompt` and the value as
     **consecutive** argv entries, preserving embedded newlines; empty string is treated
     as unset. Codex and gemini **warn and drop**, with their two sentences pinned verbatim.
7. **Port `bridge/`.** The decision tree, verbatim from the matrix: outside dux with tmux
   → `send-keys … Enter`; `DUX_TMUX_TARGET` → `-t <target>`; `DUX_PANE` forces the file
   queue even when tmux is available; a **dead `DUX_PID` under dux drops the notification
   entirely**; no `$TMUX` and no `$AM_ME` routes to `.unrouted/` (never the queue root) —
   while an agent literally named `_unrouted` must **not** collide with that bucket.
   - Under dux with `AM_ROOT`, the wake becomes a real
     `amq drain --root --me --json` call, and the queued body strips control bytes.
     `count==0` queues nothing. **Strict signed delivery bypasses drain entirely** and
     queues the verified body byte-for-byte.
   - A raw Ctrl-C (`\003`) under dux is a pure no-op. Empty argv is a no-op in both modes.
   - Skip mode (the default) delivers unsigned bodies as-is, unwraps DUX2 **without**
     checking the MAC, and treats a malformed DUX2 (too few fields) as a raw body.
   - `AM_ME` sanitisation (`Feature/Login.v2` → `feature-login-v2`) must **exactly mirror**
     the wrapper's normalization, and neutralise `../../etc` traversal.
8. **Write the missing tests the bash never had**, listed in the matrix §3: the
   backgrounded `DUX_AMQ_STARTUP_OWNER_PID` bridge launch (never exercised through any
   wrapper), the oneshot `exec` short-circuits (`claude-amq -p/--print/--bare`,
   `gemini-amq -p/--print`, `codex exec …` — **completely untested today**), the
   `config.json.tmp` cleanup on a failed final `mv`, and the bridge's
   "drain returned invalid JSON" branch.
9. **Decide stream discipline explicitly.** bats' `run` **merges stdout and stderr** into
   `$output`, so dozens of assertions do not distinguish them. `assert_cmd` will not merge
   unless told to. Go through each pinned message and decide its stream deliberately —
   do not let the merge hide a wrong choice.
10. **Keep the bash scripts in place until parity is proven**, then remove them in a
    single commit alongside the installer switch (Phase 20).

## Acceptance criteria

- [ ] All 20 `wrappers.bats`, 20 `wrappers-p1.bats`, 7 `ownership.bats`, and 22
      `inject-bridge.bats` assertions pass **against the Rust binary**, via symlinks.
- [ ] Every pinned error string reproduced **byte-for-byte** (the list in the matrix §4.3).
- [ ] `build_launch_argv` is unit-testable in-process **and** the `exec amq …` boundary is
      preserved so the argv-recording fakes still work.
- [ ] Identity priorities #2–#4 (`basename $PWD`, git branch, PID fallback) have tests
      that did not exist before.
- [ ] Oneshot `exec` short-circuits have tests that did not exist before.
- [ ] Concurrent registration yields exactly one winner; concurrent wrappers across all
      three providers produce a valid `config.json` with exact owners.
- [ ] Missing flock fails closed before any write.
- [ ] Stream (stdout vs stderr) decided and documented for every pinned message.
- [ ] `dux` still interoperates: `dux peer send` round-trips through the Rust wrappers.
- [ ] No file exceeds 500 lines; headers present; Phase 02 gates pass.

## Validation

```bash
cargo test --workspace --all-features

# run the ENTIRE bats suite against the Rust binary via symlinks
export PATH="$PWD/target/debug/applets:$PATH"
bats dux-amq/tests

# specifically the argv-heavy suites
bats dux-amq/tests/wrappers.bats dux-amq/tests/wrappers-p1.bats \
     dux-amq/tests/ownership.bats dux-amq/tests/inject-bridge.bats

# end-to-end with the real app
cargo run -- peer send <handle> "parity check"
```

## Risks

| Risk | Mitigation |
|---|---|
| Going in-process breaks ~60 argv tests | Explicit decision above: keep the `exec` boundary **and** expose the argv builder |
| A pinned message differs by one byte | Run the full bats suite against the Rust binary; that is the acceptance criterion |
| The macOS `flock` fake degrades to a mkdir spin-loop with a 5 s ceiling | Concurrency tests on such hosts prove the spinlock, not `flock(2)`. Run the concurrency tests on Linux CI where the native primitive exists, and note the limitation |
| `wrappers-p1.bats` P1-D seed tests **skip under root** | Containerized CI running as root silently skips them; run that suite as a non-root user |
| Removing the bash scripts too early | Work item 10 — they stay until Phase 20 switches the installer |

## References

- `artifacts/research-bash-port.md` §1 (wrappers), §3 (bridge), §Bash-semantics hazards
- `artifacts/bats-test-parity-matrix.md` — the complete acceptance criteria
- `artifacts/research-runtime-memory.md` §4 (the Rust↔bash contract this must honour)

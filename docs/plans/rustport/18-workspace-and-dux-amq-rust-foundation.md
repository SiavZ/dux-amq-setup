# Phase 18 — Cargo workspace and `dux-amq-rust` foundation

**Track:** E (serialized) · **Runs alone**
**Depends on:** **01 (hard gate — new code must be linted by the new toolchain)** · **Blocks:** 19, 20

## Goal

Declare a workspace, scaffold the `dux-amq-rust` crate, and build the shared primitives
that all ten applets depend on — with byte-exact parity on the two things that cannot
drift: the HMAC signing scheme and the Claude project-directory encoder.

## Evidence

- The repo root `Cargo.toml` is a **single package with no `[workspace]` section**, and
  no other `Cargo.toml` exists in the tree. Adding `dux-amq-rust/Cargo.toml` inside the
  `dux` package directory without declaring a workspace makes it an **orphan that
  `cargo package` would try to include**.
- `dux-amq-rust/` already exists as an empty directory.
- The bash surface is ~3,600 lines across **11 shell entry points** and **10 applet
  names**, currently installed as nine separate scripts on `$PATH` plus one into
  `$STATE_ROOT/scripts`.
- Behaviour is pinned by **123 bats tests** across 14 files — the acceptance criteria
  are enumerated in `artifacts/bats-test-parity-matrix.md`.

## Workspace shape

```toml
# root Cargo.toml — a package can also be a workspace root, so `dux` stays put
[workspace]
members = ["dux-amq-rust"]
```

Benefits: one `Cargo.lock` (the existing pinned graph is reused, and `cargo deny` /
`cargo audit` cover both crates), one `target/`, and **`dux` can depend on
`dux-amq-rust` as a path dependency** to share `purge_encoding`, the handle sanitizer,
and the AMQ registry protocol instead of duplicating them.

## Dispatch strategy: one binary, multicall symlinks

| Option | Verdict |
|---|---|
| Nine `[[bin]]` targets | **Rejected** — with `opt-level="z"` + LTO + `strip`, each is still ~2–4 MB; nine copies is ~25–35 MB in the release tarball for one code base |
| Nine copies of one binary | Rejected, same reason |
| **One binary + symlinks, argv[0] multicall** | **Recommended** |

`Command::multicall(true)` has been **stable since clap 3.1.0 (2022-02-16)** and carries
through 4.x (current 4.6.6, 2026-08-06). With it, clap parses `argv[0]`'s basename as the
first subcommand. Upstream ships `examples/multicall-busybox.rs` and
`examples/multicall-hostname.rs`.

Mechanics that matter here:
- `std::env::args_os().next()` → `Path::file_name()` yields the symlink name, because
  `exec`/`Command::new` set `argv[0]` to the path actually used. `src/cli.rs`'s
  `resolve_doctor_script` calls `Command::new(path_from_which)`, so the basename is
  `dux-amq-doctor` — correct.
- **Two applet names end in `.sh`** (`amq-secret-init.sh`,
  `finalize-claude-migration.sh`). Symlink names must stay byte-identical for compat, so
  **the basename→applet map carries the `.sh` suffix as part of the key. Do not
  "normalize" it away.**
- Every applet must **also** be reachable as an explicit subcommand (`dux-amq doctor`,
  `dux-amq bridge`, …) so the tool works without symlinks and so `dux-amq install` can
  bootstrap. Per the clap cookbook, declare applets both at top level and under a `main`
  applet.

**Applet inventory (10):** `claude-amq`, `codex-amq`, `gemini-amq`, `dux-amq-doctor`,
`dux-amq-inject-bridge`, `amq-send-signed`, `amq-receive-verify`, `amq-secret-init.sh`,
`encode-claude-project-dir`, `finalize-claude-migration.sh`.

## What stays bash

- **Root `install.sh`** — a `curl | bash` release fetcher. Chicken-and-egg: it exists to
  put a binary on disk. **Keep as bash.**
- **`install-gocryptfs.sh`** — opt-in, never invoked by `install.sh`, **zero test
  coverage**, purely a `gocryptfs`/`mountpoint` driver. Lowest port value. **Leave as
  bash** and say so explicitly in `dux-amq/README.md` rather than leaving it ambiguous.

## Module tree — this phase builds `common/`, `auth/`, `encode/` only

```text
dux-amq-rust/src/
  main.rs                    ~120  argv[0] basename -> applet; clap multicall; anyhow -> exit code
  lib.rs                      ~70  pub mod re-exports; the shared surface `dux` links against
  common/
    mod.rs                    ~30
    env.rs                   ~190  bash ${VAR:-x} semantics (empty == absent); StateRoot /
                                   DuxHome / AmqRoot / LocalBin / SecretPath resolution
    handle.rs                ~130  sanitize_handle(), validate(<=64, ^[a-z0-9_-]+$),
                                   `.unrouted` fallback
    shquote.rs                ~90  printf %q parity (error messages are pinned byte-for-byte)
    ctrlbytes.rs              ~70  the exact C0-except-TAB/LF + DEL delete set
    proc.rs                  ~230  run-with-timeout; (timeout)/(not found)/(error) sentinels;
                                   exec-replacing-self
    locks.rs                 ~140  rustix::fs::flock wrappers: blocking-exclusive (registry),
                                   non-blocking (finalize); retry-on-EINTR
    atomic.rs                ~110  mktemp-in-dir + fsync + rename; the `.inflight.` prefix rule
  auth/
    mod.rs                    ~40
    secret.rs                ~120  AMQ_SECRET_PATH; init (32B urandom -> base64, 0600, no NL);
                                   read + trailing-\n trim
    sign.rs                  ~150  PAYLOAD "DUX2|me|to|ts|nonce|body_b64"; HMAC-SHA256;
                                   base64 MAC; TAB envelope
    verify.rs                ~300  the 11-stage rejection pipeline
    nonce_store.rs           ~130  mkdir-atomic claim + age-based prune
  encode/
    mod.rs                   ~100  Claude project-dir encoder — lift src/purge_encoding.rs
```

## Dependencies

Everything except `clap`, `hmac`, `sha2`, `subtle`, `base64`, and `which` is **already in
the lock file**. All are MIT/Apache-2.0 with MSRV ≤ 1.85, clearing `deny.toml`'s
allowlist and its crates.io-only, `allow-git = []` policy.

| Crate | Why |
|---|---|
| `clap` 4.6 | Multicall dispatch. **Note: the parent `dux` crate deliberately hand-rolls its arg parsing** — adding clap here is a departure. The alternative is a ~150-line hand-rolled dispatcher matching `dux`'s style. **Decide explicitly and record the decision.** |
| `rustix` 1.1 (already present) | `flock`, `setsid`, `kill_process`, `getsid`. **Zero new dependency** — `src/peer.rs` already uses `rustix::fs::flock` on the very same `meta/config.lock`, so this guarantees identical lock semantics between the Rust app and the ported wrappers |
| `hmac` 0.12 + `sha2` 0.10 | **Pure Rust, no openssl** — release builds target `*-unknown-linux-musl`, where linking openssl is a liability. 0.10 avoids a duplicate `sha2` (0.10.9 is already in the lock via `termwiz`) |
| `subtle` 2.6 | Constant-time MAC comparison. The bash uses `[[ "$MAC" != "$EXPECT" ]]`, which is **not** timing-safe. This is a security improvement with no observable behaviour change |
| `serde`/`serde_json`, `anyhow`, `base64`, `rand`, `tempfile`, `chrono`, `rusqlite`, `sysinfo`, `which` | Already present or trivially justified; `serde_json` replaces **every `jq` shell-out**, `rusqlite` removes the `sqlite3-cli-missing` degraded state, `sysinfo` removes an un-timeout-guarded `ps` fork (`dux-amq-doctor:700-705`) |

## Work items

1. **Add `[workspace] members = ["dux-amq-rust"]`** to the root `Cargo.toml` and confirm
   `cargo package` no longer treats the subdirectory as stray content.
2. **Scaffold the crate** with `main.rs` + `lib.rs` and a lib target (the lib target is
   what makes doctests run — see `CONVENTIONS.md` §1's caveat).
3. **Record the `clap` decision** in `docs/contributing/` — the parent crate hand-rolls
   arg parsing; this crate does not. Either is defensible; an unexplained inconsistency
   is not.
4. **Implement `common/env.rs` with exact bash `${VAR:-x}` semantics** — in bash, an
   **empty** variable is treated as absent by `:-`. A Rust `env::var().is_ok()` check
   would diverge. This is the single most likely source of silent behavioural drift
   across the whole port.
5. **Implement `common/shquote.rs` for `printf %q` parity.** Error messages are pinned
   byte-for-byte by bats, and several are built with `%q`. A Rust `format!` will not match
   without deliberate replication.
6. **Implement `common/locks.rs` on `rustix`**, with both blocking-exclusive (registry)
   and non-blocking (finalize) modes and EINTR retry. Must match `peer.rs`'s semantics
   exactly, since both lock `meta/config.lock`.
7. **Implement `auth/` bit-exactly.** The payload is
   `"DUX2|me|to|ts|nonce|body_b64"`, HMAC-SHA256, base64 MAC, TAB-delimited envelope.
   Port the 11-stage rejection pipeline in `verify.rs` and the mkdir-atomic nonce store.
   **Use `subtle` for the comparison** — the one deliberate improvement.
8. **Lift `src/purge_encoding.rs` into `encode/`** and have `dux` depend on it via the
   path dependency, so there is one encoder rather than two. It must remain bit-exact:
   `encoder-fixtures.bats` pins 12 recorded pairs from a live Claude Code install, plus
   absolute-path rejection (exit 2, `"absolute path required"`), single-trailing-slash
   stripping, and case preservation.
9. **Port the harness fakes into a Rust test-double crate** — an argv-recording stub with
   the exact `ARGV\n<arg>\n…\nEND\n` framing (many tests `grep -Fxq` literal lines),
   version stubs, and a real advisory-lock double.
   **Caution:** `tests/fakes/{claude,codex,gemini}` **exit 1** for any non-`--version`
   invocation — the trailing `if` is the script's last statement, so a false condition
   becomes its exit status. Currently inert; do not port them assuming "prints nothing,
   exits 0".
10. **Write the parity tests for `common/`, `auth/`, `encode/`** against the bats matrix:
    all 13 `amq-auth.bats` assertions and all 8 `encoder-fixtures.bats` assertions —
    except the byte-identical-across-three-wrappers test (`encoder-fixtures.bats:148`),
    which is **meaningless once there is one shared implementation. Delete it**; one unit
    test stands in for the intent.
11. **Add file headers** per `CONVENTIONS.md` §1 and wire the new crate into the Phase 02
    gates (`check-file-length`, `module-trees --check`, `cargo modules orphans`).

## Acceptance criteria

- [ ] Workspace declared; `cargo build`, `cargo test`, `cargo deny`, `cargo audit` all
      cover both crates; one `Cargo.lock`.
- [ ] `dux-amq-rust` has a lib target and `dux` depends on it by path.
- [ ] `clap`-vs-hand-rolled decision recorded.
- [ ] `common/env.rs` treats an empty env var as absent, with a test proving it.
- [ ] `shquote` matches `printf %q` for the full byte range 0x00–0xFF, property-tested
      against the shell.
- [ ] `locks.rs` semantics identical to `peer.rs`'s `rustix::fs::flock` usage.
- [ ] DUX2 sign/verify round-trips **byte-for-byte** with the bash implementation —
      cross-verified in both directions (bash signs → Rust verifies, and the reverse).
- [ ] Tabs, non-ASCII, and trailing newlines survive the body field exactly
      (`amq-auth.bats:169`).
- [ ] Replay/nonce dedup is race-free under concurrent delivery (`amq-auth.bats:190`).
- [ ] Wrong-recipient, MAC-mismatch, unsigned-drop, argv-vs-stdin precedence all match.
- [ ] Encoder matches all 12 recorded fixtures; `count >= 6` guard preserved.
- [ ] `subtle` used for MAC comparison.
- [ ] No file in the new crate exceeds 500 lines; headers present; gates pass.

## Validation

```bash
cargo build --workspace
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo deny --all-features check
ci/check-file-length.sh

# cross-implementation parity, both directions
dux-amq/scripts/amq-send-signed --to bob --me alice --body 'x' --print-only \
  | cargo run -p dux-amq -- amq-receive-verify
cargo run -p dux-amq -- amq-send-signed --to bob --me alice --body 'x' --print-only \
  | dux-amq/scripts/amq-receive-verify

bats dux-amq/tests/amq-auth.bats dux-amq/tests/encoder-fixtures.bats
```

## Risks

| Risk | Mitigation |
|---|---|
| `${VAR:-x}` empty-vs-absent divergence | Work item 4 plus an explicit test; this is the highest-probability silent drift in the port |
| `%q` output differs from Rust formatting | Property-test against the real shell over the full byte range |
| HMAC parity broken subtly (trailing newline in the secret, field ordering) | Bidirectional cross-verification is an acceptance criterion, not a spot-check |
| Adding `clap` diverges from the parent crate's style | Recorded decision (work item 3); revisit only with a written reason |
| The new crate escapes the Phase 02 gates | Work item 11 wires it in explicitly |

## References

- `artifacts/research-bash-port.md` §A, §B, §C, §D, §E, §0, §4, §5
- `artifacts/research-external-patterns.md` §1 (clap multicall), §Crate recommendations
- `artifacts/bats-test-parity-matrix.md` §1 (harness), §amq-auth, §encoder-fixtures, §4 (hazards)

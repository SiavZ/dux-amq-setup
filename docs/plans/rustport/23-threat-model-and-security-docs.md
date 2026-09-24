# Phase 23 — Threat model and security documentation

**Track:** E (serialized) · **Runs alone**
**Depends on:** 04 (provenance repaired), 19, 20 (the new surface exists) · **Blocks:** 24

## Goal

Close the gap between an unusually good threat model and the attack surface it does not
mention — eleven unlisted items, one overstated mitigation, and a new Rust crate that
changes several existing rows.

## Evidence

`SECURITY.md` carries a 19-row STRIDE table with a ~840-line long-form companion, and
**accepted risks are argued rather than hand-waved** — 13 mitigated, 2 accepted-risk
(T2, T14-by-inheritance), 2 planned-only (T10, T11), 1 documentation-only (T5), T3
partial. T13's claims were spot-verified and **hold in code**: `REGEX_SIZE_LIMIT = 64*1024`
(`watch/engine.rs:38`), `REGEX_DFA_SIZE_LIMIT` (`:40`), `MAX_RULES_PER_PROVIDER = 32`
(`:43`), applied at `:155-157`; `cooldown_ms` default 30,000 (`watch/rule.rs:44`);
`budget.max_attempts` default 5 (`:208`).

This phase is not a rewrite. It is closing specific, enumerated gaps.

### T3 is overstated — fix the code, then the row

`dux-amq/install.sh:283-305` pins the verified binary at `$STATE_ROOT/amq-bin/amq`, and
`bashrc-additions.sh:25,63-64` refuses to `eval` unless the hash matches. But
**`AMQ_BIN` and `amq-bin` appear nowhere in `src/`** — the Rust binary shells out to bare
`amq` resolved from `PATH` (`peer.rs:480`, `purge.rs:1013`). **A tampered `amq` earlier in
`PATH` is executed by dux with no hash check.**

### Attack surface with no threat ID

| # | Surface | Gap |
|---|---|---|
| 1 | **Claude Peers localhost broker** (`peer.rs:548-590`) — `TcpStream::connect_timeout` to `127.0.0.1:$CLAUDE_PEERS_PORT`, hand-rolled HTTP/1.1 | Grepping `SECURITY.md` **and all 840 lines** of `threat-model.md` for `claude peers`/`127.0.0.1`/`loopback`/`localhost`/`tcp` returns **zero hits**. (a) **No authentication** — no token, HMAC, or nonce, in direct contrast to T2's envelope for the AMQ path; any local process reaching that port can post. (b) `CLAUDE_PEERS_PORT` is an **unvalidated env override** (`:549-552`), so control of dux's environment redirects a channel carrying repo-derived bodies to an arbitrary local sink. (c) **`read_to_string` at `:573` is unbounded** — a hostile or wedged broker OOMs dux; the 3 s read timeout (`:556`) bounds time, not size. **This is the only network egress in the binary and it has no row.** |
| 2 | **Claude Peers MCP server install** (`dux-amq/install.sh:337-372`) | Third-party TypeScript registered as a **user-scoped MCP server for every Claude session on the box**, with `bun install` unpinned and scripts enabled. `SECURITY.md:15-28` "Scope" does not mention it, and `CLAUDE.md` explicitly names "a new MCP integration" as requiring a STRIDE row |
| 3 | **`src/crash.rs:24-28,41-45`** | Panic payload and full `Backtrace` written **verbatim, unsanitized** to `dux-crash.log` — **no `sanitize::for_terminal` call**, unlike `dux.log` (T8). Panic messages routinely interpolate branch names and PTY text, so raw OSC/CSI lands in a file operators `cat`. `create(true).append(true)` sets no mode → **0644**, while `sessions.sqlite3` is deliberately 0600 (`storage.rs:35,55`). No rotation, no size cap |
| 4 | **`dux.log` file mode** | `tracing_appender`'s rolling builder sets no mode → **0644**. On a shared host the JSON-Lines log (worktree paths, branch names, PR titles, PTY excerpts) is world-readable |
| 5 | **Arbitrary configured provider execution** (`pty.rs:206-219`, `provider.rs:54-62`) | **No `env_clear()`** on either path — `pty.rs:210-214` only *adds* vars, so children inherit dux's full environment **including any API keys**. `provider.rs:56-62` substitutes `{prompt}` into argv and the commit-message prompt embeds repo diff content, so repo-controlled text can produce an argv element beginning with `-` (option injection into the provider CLI; not shell injection). T1 covers what the agent does once running; nothing covers what dux is configured to run |
| 6 | **`purge.rs:1013-1015`** | `Command::new("amq").args(["send", branch, …])` — `branch` sits in argv position 2 with **no leading-`-` check**. `git.rs:1114` rejects `-`-leading names for branches *dux creates*; a pre-existing branch in a cloned repo is not covered here |
| 7 | **`src/git.rs`, 44 `Command::new("git")` sites** | Argv arrays throughout, `--` separators on sensitive paths, correct plumbing discipline — but **no `GIT_*` environment hygiene anywhere**: every git child inherits `GIT_DIR`, `GIT_CONFIG_GLOBAL`, `GIT_SSH_COMMAND`. Also `git pull --ff-only` (`:148`) and the `gh` CLI paths are **network egress not enumerated** |
| 8 | **`npx skills`** | Writes into `~/.claude/skills/`, which `SECURITY.md:21-22` puts *in scope*, with no threat ID |
| 9 | **`clipboard.rs:122-147`** | OSC 52 written directly to `/dev/tty` — into the operator's **real** terminal, outside dux's emulator. Payload is base64 (`:125`) with a 100 KiB cap (`:10,135`), so this is low risk — but it is an unlisted channel from agent-controlled pane text to the host clipboard |
| 10 | **`editor.rs:109-120`** | `Command::new(&editor.command)` against a five-name allowlist resolved by scanning PATH. Surface is PATH-shadowing, not arbitrary execution. Low, but unlisted |
| 11 | **`src/lockfile.rs`** | Zero mentions of `lockfile`/`dux.lock`/"single-instance" in either document; `:123` opens with no explicit mode |

Adequately covered already — **do not re-audit:** `orphan_worktrees.rs` (T17),
`resume_recovery.rs` (T18), `storage.rs` (T4/T15, correct 0600).

### What the port itself changes

- **The binary-integrity `eval` guard** (`bashrc-additions.sh:26-30`) is T3's mitigation.
  Moving it into Rust weakens the property from *"bash refuses to eval"* to *"a binary
  checks itself"*. **Keep the shell-side guard.**
- **The DUX2 HMAC envelope** (T2/T14's opt-in rail) exists only in the two bash scripts.
  Phase 18 reimplements it; the rows must reflect constant-time comparison (now via
  `subtle`, an improvement over the bash's `[[ != ]]`), the atomic nonce-dir replay guard,
  and receiver binding.
- **Cross-language `flock` on `meta/config.lock`** (T7) is load-bearing: Rust and bash must
  lock the *same inode*. `peer.rs`'s `rust_and_wrapper_claims_serialize_on_config_lock` is
  the **only** test proving it. Phase 18 deliberately reuses `rustix` for exactly this reason.
- **TIOCSTI detection has zero Rust logic today** — `grep -ri tiocsti src/` yields one
  config *comment* (`config.rs:1712`). Phase 20 moves it into Rust; T-row must follow.

## Work items

1. **Fix T3's bypass in code first, then correct the row.** Make dux resolve `amq` through
   the pinned `$STATE_ROOT/amq-bin/amq` with a hash check, rather than bare `amq` from
   `PATH` (`peer.rs:480`, `purge.rs:1013`). A row that overstates its mitigation is worse
   than an accepted risk.
2. **Add threat rows for all eleven unlisted surfaces**, appending as `T20`, `T21`, …
   **Never reshuffle existing IDs** (project rule). Each row gets its long-form section in
   `docs/operations/threat-model.md`.
3. **Fix items 3 and 4 in code**, not just in the table: sanitize crash-log content through
   `sanitize::for_terminal`, set 0600 on both `dux-crash.log` and `dux.log`, and add
   rotation plus a size cap to the crash log. These are cheap and remove the finding rather
   than documenting it.
4. **Decide on `env_clear()` for provider spawns (item 5).** Full `env_clear()` would break
   providers that need `PATH`, `HOME`, and their own credentials. The honest fix is an
   **allowlist** plus an explicit, documented statement that dux passes its environment to
   configured providers. Whichever is chosen, **say so in the config comment for
   `[providers.*].command`** — the user is choosing what to execute.
5. **Add leading-`-` rejection at `purge.rs:1013`** (item 6), reusing `git.rs:1114`'s
   existing check rather than writing a second one.
6. **Add `GIT_*` environment hygiene** (item 7) in the shared `run_git` helper Phase 15
   introduces — one place, by construction.
7. **Address the Claude Peers broker (item 1) concretely.** At minimum: validate
   `CLAUDE_PEERS_PORT`, bound the `read_to_string` at `:573`, and document the absence of
   authentication as an **accepted risk with its blast radius stated** — a local-only,
   unauthenticated channel is defensible on a single-user host and indefensible on a shared
   one. Say which deployment it assumes.
8. **Write `scripts/validate-threat-model.sh`** — specified at `SECURITY.md:157-161`
   against inputs that no longer exist (Phase 04). Point it at the live tree and wire it
   into CI so the table cannot drift from reality again.
9. **Update the `opaline` supply-chain assessment** from Phase 01 item 10 into a threat
   row: six months old, 16,073 total downloads, single maintainer, compiled into the
   shipped binary, unsigned crates.io publishes, and **neither `cargo vet` nor `cargo crev`
   configured**. It is the weakest link in an otherwise well-hardened supply chain.
10. **Re-scope `SECURITY.md:15-28`** to cover the new `dux-amq-rust` crate, its ten
    applets, and the MCP server install.
11. **Add the missing `--version` flag.** `grep '"--version"' src/` is empty;
    `CARGO_PKG_VERSION` is used only in the TUI (`app/mod.rs:1460`, `render.rs:442`).
    Combined with `strip = true`, **a field binary cannot be identified** — which makes
    incident response and vulnerability triage guesswork.

## Acceptance criteria

- [ ] T3's bypass fixed in code; the row's claim matches behaviour.
- [ ] Eleven new threat rows appended (T20+), each with a long-form section; **no existing
      ID renumbered**.
- [ ] Crash log sanitized, 0600, rotated, size-capped; `dux.log` 0600.
- [ ] Provider-spawn environment policy decided, implemented, and documented in the config
      comment.
- [ ] Leading-`-` rejection at `purge.rs:1013`.
- [ ] `GIT_*` hygiene in the shared `run_git` helper.
- [ ] `CLAUDE_PEERS_PORT` validated; `read_to_string` bounded; the authentication gap
      documented as an accepted risk with a stated deployment assumption.
- [ ] `scripts/validate-threat-model.sh` exists, runs in CI, and passes.
- [ ] `opaline` risk documented as a threat row.
- [ ] `SECURITY.md` scope covers `dux-amq-rust` and the MCP install.
- [ ] `dux --version` and `dux-amq --version` work.
- [ ] `rust_and_wrapper_claims_serialize_on_config_lock` still green — the cross-language
      lock still targets the same inode.

## Validation

```bash
scripts/validate-threat-model.sh
cargo test --workspace --all-features
cargo test rust_and_wrapper_claims_serialize_on_config_lock

# permissions
ls -l ~/.dux/dux.log ~/.dux/dux-crash.log     # expect 0600 on both

# no bare `amq` from PATH
grep -rn 'Command::new("amq")' src/            # must route through the pinned path

# identifiability
cargo run -- --version
cargo run -p dux-amq -- --version
```

## Risks

| Risk | Mitigation |
|---|---|
| Eleven new rows dilute a currently sharp document | Each row states mitigation **or** accepted risk with blast radius — the existing document's strength is that it argues rather than hand-waves; match that standard |
| `env_clear()` breaks provider authentication | Allowlist, not clear; test each of the four stock providers end-to-end before shipping |
| Moving the binary-integrity check into Rust weakens T3 | Explicitly **keep the shell-side guard**; the Rust check is additional, not a replacement |
| `validate-threat-model.sh` becomes another aspirational spec | It is wired into CI as an acceptance criterion — a specified-but-unwritten check is exactly the failure this phase is fixing |

## References

- `artifacts/research-debt-ci-security.md` §Security posture, §Attack surface with no
  threat ID, §What the Rust port changes, §Production-readiness gaps
- `SECURITY.md`, `docs/operations/threat-model.md`

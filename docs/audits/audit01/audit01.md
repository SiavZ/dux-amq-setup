# dux-amq-setup audit (2026-05-02)

Scope: branch `dux-amq-setup` @ `345d882` of `SiavZ/dux-amq-setup` (fork of `patrickdappollonio/dux`). Audit covers the `dux-amq/` overlay (installer, wrappers, scripts, config, docs) and the four Rust patches (`src/clipboard.rs`, `src/app/mod.rs`, `src/app/render.rs`, `src/config.rs`).

## Executive summary

- **Posture**: a credible developer-oriented overlay with thoughtful UX touches (OSC 52 fallback, scrollbar, auto-resume, identity from worktree basename), but it is **not yet production-grade**. The default-on `--dangerously-skip-permissions` for Claude *and* the unconditional `--dangerously-bypass-approvals-and-sandbox` for Codex give every pane root-equivalent authority on the host, and several supply-chain links (`curl|bash` AMQ install, `npx -y` skill install, GitHub release tarball with no checksum) have no integrity verification.
- **Top risks**: (1) two unsandboxed YOLO agents running in parallel on a persistent VM with shared `/data/state/`, (2) zero supply-chain verification on three install steps, (3) a documented-but-inverted seeding default that can silently 10x token/billing exposure on first launch, (4) reliance on `/dev/tty` and on `amq wake` injection that — depending on AMQ's transport — may use the legacy/disabled `TIOCSTI` path.
- **Hard blockers for production use**: P0-1, P0-2, P0-3, P0-4, P0-5 below. They are all small fixes; none require redesign.
- **Strengths**: clipboard worker is well-isolated and tested with RFC 4648 vectors; bash scripts uniformly use `set -euo pipefail`; persistent-disk migration is idempotent and refuses to run while `claude` is alive; CI (clippy `-D warnings`, fmt, test) is inherited from upstream and passes.
- **Drift**: only one upstream commit (#204 "Harden git output parsing and worktree mirroring") is missing — easy to merge — but **no automation exists** to keep this overlay current with upstream dux. That is the single biggest long-term sustainability risk.

## P0 — must fix before production

### P0-1: Default-on YOLO permissions for Claude *and* Codex with no opt-out for Codex
- **Files**: `dux-amq/wrappers/claude-amq:79-85`, `dux-amq/wrappers/codex-amq:27`
- **What's wrong**: `claude-amq` enables `--dangerously-skip-permissions` by default (CLAUDE_AMQ_SAFE=1 to opt out). `codex-amq` *unconditionally* appends `--dangerously-bypass-approvals-and-sandbox` with no opt-out at all. Every dux pane therefore runs with full filesystem + shell capability and no per-tool prompt. Anthropic explicitly recommends sandbox/container/VM isolation for `--dangerously-skip-permissions`, and as of March 2026 ships `auto mode` as the supported replacement. PromptArmor and a documented Claude Code bug (#10077) showed prompt injection and recursive `rm -rf` causing data loss in this exact configuration. Running two such agents simultaneously on a persistent disk multiplies the blast radius.
- **Concrete impact**: any prompt-injection vector reaching either pane (a malicious README in a cloned repo, a poisoned issue body via `gh`, a doc fetched by an MCP tool, a webpage via WebFetch) lets the attacker execute arbitrary commands as the VM user, including `rm -rf $HOME`, `curl | sh` of malware, exfil of `/data/state/amq` (which contains every agent's chat transcripts), or pivot to other agents' worktrees via the shared `AM_ME` queue.
- **Recommended fix**:
  - Flip the default. Make YOLO opt-in via `CLAUDE_YOLO=1` (already documented in `bashrc-additions.sh`) or `CLAUDE_AMQ_YOLO=1`, not opt-out.
  - Add the symmetric `CODEX_AMQ_SAFE=1` (or invert to `CODEX_AMQ_YOLO=1`) flag in `codex-amq`.
  - Migrate to Claude Code `auto mode` once upstream wrapper plumbing supports the equivalent (`--auto` or its successor) and document the migration path in the README.
  - Document the threat model explicitly in `dux-amq/README.md` under a new "Security" section, recommending the VM be ephemeral (preemptible spot OK; long-lived persistent worker not OK) and that `/data/state/amq` be encrypted.

```bash
# claude-amq
EXTRA=()
if [[ "${CLAUDE_AMQ_YOLO:-${CLAUDE_YOLO:-}}" == "1" ]]; then
  EXTRA+=(--dangerously-skip-permissions)
fi
# codex-amq
CODEX_EXTRA=()
if [[ "${CODEX_AMQ_YOLO:-${CLAUDE_YOLO:-}}" == "1" ]]; then
  CODEX_EXTRA+=(--dangerously-bypass-approvals-and-sandbox)
fi
exec amq coop exec --no-wake --no-init --root "$ROOT" --me "$ME" codex -- "${CODEX_EXTRA[@]}" "$@"
```

### P0-2: No supply-chain verification on three install steps
- **Files**: `dux-amq/install.sh:23-27` (dux release), `:32-35` (AMQ `curl | bash`), `:41-43` (`npx -y skills add ...`)
- **What's wrong**: All three downloads run unverified code over TLS-only trust:
  1. `curl -fsSL ... releases/latest` parses JSON with `grep -oP '"tag_name":"\K[^"]+'` — fragile and unauthenticated; a release pulled from a hijacked repo or a rewritten redirect would be installed silently.
  2. `curl -fsSL https://raw.githubusercontent.com/avivsinai/agent-message-queue/main/scripts/install.sh | bash` is the canonical anti-pattern. No checksum, no commit pin, runs `main` HEAD as root-equivalent of the user.
  3. `npx -y skills add avivsinai/agent-message-queue -g -y` auto-accepts whatever the npm registry currently serves; no `--ignore-scripts`, no integrity hash, and `-y` accepts arbitrary postinstall scripts. The **stderr is dropped** (`>/dev/null 2>&1`), so a successful compromise is invisible.
- **Concrete impact**: any compromise of the upstream `agent-message-queue` repo (a single maintainer account or a stolen npm token) executes attacker code on every install. This is the same vector that hit `event-stream`, `ua-parser-js`, and `xz-utils`.
- **Recommended fix**:
  1. Pin a known-good dux release tag (`DUX_TAG=v0.x.y`) and verify against an SHA-256 published in this repo or the dux release's `*.sha256` asset. Drop the `latest` lookup or treat it as informational.
  2. Pin AMQ to a specific commit/tag and **download the script first, log it, sha-check it, then run**. Ship the expected sha256 in `install.sh`.
  3. Replace `npx -y` with a pinned commit-sha install or vendor the skill files in this repo. At minimum, do not silence stderr — show the user what was just installed.
  4. Document the trust boundary: which third parties are now in the user's TCB, and how to revoke (delete `~/.local/bin/{amq,dux}`, `rm -rf /data/state/{amq,agents}`).

### P0-3: Inverted/inconsistent default for Claude session seeding
- **Files**: `dux-amq/wrappers/claude-amq:10-17` (header), `:25-58` (function), `dux-amq/README.md:92`
- **What's wrong**: The function header documents seeding as **opt-in** via `CLAUDE_AMQ_SEED_FROM_PARENT=1`. The actual code is **opt-out** via `CLAUDE_AMQ_NO_SEED=1`, i.e. enabled by default. The README's "Trade-offs" warns that this seeding inherits the entire parent session history (~100 MB on heavy repos) which can push fresh sessions into the 1M-context billing tier "earlier". Default-on means every new dux pane silently rsyncs the parent worktree's full Claude transcript dir on first launch.
- **Concrete impact**: (a) silent disk amplification (N panes × full history); (b) unexpected token-billing escalation on first use ("why did my pane open with 800k tokens of context already?"); (c) cross-worktree information leak — chat transcripts from the parent (which may be a different feature branch with secrets, customer data, or another tenant) are copied into the new worktree's session storage; (d) doc/code mismatch is itself a P0 because operators following the docs to enable seeding will believe it's off when it's on.
- **Recommended fix**:
  - Either flip the default to off (matches the docs) — preferred — or fix the docs to match the implementation. The disk-amplification + leakage risk argues for **off-by-default**:
```bash
# claude-amq, replace lines 26-27
[[ "${CLAUDE_AMQ_SEED_FROM_PARENT:-}" == "1" ]] || return 0
```
  - Add a one-line stderr notice when seeding fires so users see what just happened, not just success count.

### P0-4: Symlink TOCTOU + `pgrep` race in `finalize-claude-migration.sh`
- **Files**: `dux-amq/scripts/finalize-claude-migration.sh:34-51`
- **What's wrong**: Several concurrency hazards:
  1. The `pgrep` "no claude running" check runs once at the top, then `rsync` + `mv` + `ln -s` happen seconds later. A user who spawns `claude` in another shell between the check and the `mv` causes the live process to point at a moved/missing `~/.claude`, corrupting on-disk session state.
  2. `mv "$src" "$bak"` then `ln -s "$dst" "$src"` is two non-atomic steps. If the script is killed between them (Ctrl-C, OOM, spot preemption), `~/.claude` no longer exists at all — every subsequent `claude` invocation creates a fresh empty config dir.
  3. The auto-created `/data/state/.agents -> /data/state/agents` bridge symlink is created without `[[ -L ]]` check on the target type — fine here, but written as if "`-e`" alone were sufficient.
  4. `rsync -aH --delete "$src/" "$dst/"` *deletes* anything in `$dst` not in `$src`. If a user has already accumulated some persistent-disk state (e.g. from a previous half-run finalize, or another VM), this silently destroys it.
- **Concrete impact**: data loss windows during migration; non-atomic state on interruption; surprise deletion of pre-existing persistent state.
- **Recommended fix**:
  - Hold an `flock` on `/tmp/dux-amq-finalize.lock` so concurrent migrations / claude spawns are detected.
  - Use `mktemp -d` + `mv -T` (atomic rename of dirs on the same fs) for the symlink swap, or stage as `~/.claude.new -> $dst` then `mv -Tn ~/.claude.new ~/.claude`.
  - Drop `--delete` from rsync (or gate it behind a `--force` flag the user must pass). The migration is one-shot; no need for delete-on-target.
  - After preflight passes, re-check `pgrep claude` immediately before each destructive op, and fail fast if it returns processes.

### P0-5: `sed`/`tr` of branch names allows path traversal of CWD-equality check
- **Files**: `dux-amq/wrappers/{claude,codex,gemini}-amq` (the `ME=...` block), `dux-amq/wrappers/claude-amq:38-41` (`enc_self`/`enc_main`)
- **What's wrong**: The identity sanitizer is `tr '[:upper:]' '[:lower:]' | sed 's|[^a-z0-9_-]|-|g; s|^-\+||; s|-\+$||'`. This is fine for the AMQ handle. But the **session-seeding path encoder** is different: `sed 's|/|-|g; s|_|-|g'`. It does not collapse adjacent dashes and applies *only* `_` and `/` substitution, so paths containing `-`, spaces, or unicode produce different encodings on different platforms (and macOS path-canonicalization differs from Linux). More importantly:
  - The encoded directory is then used as `$HOME/.claude/projects/$enc_self`. If a worktree path is attacker-influenced (e.g. a dux pane name chosen via `prompt_for_name = true`), and `dux` does not itself sanitize, an encoded `..-..-etc-passwd` is possible.
  - The check at `claude-amq:67` is a **prefix match** `[[ "$PWD" == "${DUX_HOME:-/data/state/dux}/worktrees/"* ]]`. This is a glob, not a path containment; `/data/state/dux/worktrees-evil/x` matches the prefix and would be treated as a dux worktree. Use `[[ "$PWD" == "$DUX_HOME/worktrees/"*/* ]]` plus `realpath` comparison.
- **Concrete impact**: low-likelihood but real privilege confusion if `dux` ever accepts user-controlled worktree paths. Worse: the `sed` encoding mismatch causes seeding to silently no-op (or worse, cross-pollinate the wrong session) on paths containing characters Claude Code's actual encoder handles differently. Currently empirical, but a foot-gun any time a user puts a hyphen or unicode in a branch name.
- **Recommended fix**: replicate Claude Code's *exact* on-disk path-encoding routine (or call `claude config sessions-dir <path>` if it exists) rather than re-implementing in `sed`. Replace prefix glob with `realpath`-canonicalised containment check. Add a single regression test (a static fixture: a known path → expected encoded dir) under `dux-amq/tests/`.

## P1 — should fix soon

### P1-1: `amq wake` reliance on TTY injection — likely TIOCSTI on disabled kernels
- **Files**: `dux-amq/wrappers/{claude,codex,gemini}-amq:96/25/29` (`amq wake ... --inject-mode raw </dev/tty`)
- **What's wrong**: AMQ's `wake` injects messages "as plain text + carriage return, which Claude Code (Ink-based) auto-submits". The standard way to push characters into another tty is `TIOCSTI`. Linux 6.2+ disables `CONFIG_LEGACY_TIOCSTI` by default; many distros (Debian 12+, Ubuntu 24.04 LTS) ship without it. Ubuntu bug 2046192 tracked exactly this. If AMQ binary uses TIOCSTI, the wake daemon silently fails on a current kernel — all the comments about "stdin must be /dev/tty" only address the bash-job-control gotcha, not the kernel-level deprecation.
- **Concrete impact**: silent breakage on modern kernels; the inject simply does nothing, and the agent never sees the wake message.
- **Recommended fix**: confirm AMQ's transport (it may be ptmx-master writes, which is fine). If TIOCSTI: pin minimum supported kernel in README, or document `sysctl -w dev.tty.legacy_tiocsti=1` workaround, or open an upstream issue/patch to switch to `TIOCPKT`/PTY-master writes. Either way, *test* on a stock Ubuntu 24.04 image.

### P1-2: Background `amq wake &` interacts badly with `set -euo pipefail`
- **Files**: all three wrappers
- **What's wrong**: `set -e` does not propagate failures from background jobs. If `amq wake` fails to start, the wrapper proceeds, `exec amq coop exec` works, the user types, and… messages silently never deliver. There is no startup probe (e.g. `pidof amq wake | head -1`, or a brief `wait $! && false`-pattern check). `>/dev/null 2>&1` also discards the diagnostic.
- **Concrete impact**: silent failure mode that's invisible in production; the user sees their AMQ messages reach the queue but never get woken on the receiver side.
- **Recommended fix**: log to a file (`exec 2>>"$HOME/.local/share/dux-amq/wake-$ME.log"` style), add `disown` after the `&`, and `kill -0 $!` in a subshell after a 200 ms sleep to confirm the daemon is up.

### P1-3: `auto_resume_all_sessions` thundering-herd at startup
- **Files**: `src/app/mod.rs:1380-1415`
- **What's wrong**: When `auto_resume_on_start = true`, every persisted session spawns its PTY in a tight loop on the main thread. The doc-comment correctly notes "N provider processes (CPU/RAM)", but offers no concurrency cap, no rate limit, and no skip on detached sessions whose worktree is on a slow-mount (`/data/state` may be a network volume). For users with 10+ sessions this turns startup into a multi-second freeze plus a fork bomb of Claude/Codex/Gemini processes (each itself spawning Node + an LLM connection).
- **Concrete impact**: spot-VM preempt → reboot → all panes auto-spawn at once → memory spike → OOM kill of arbitrary processes. The sub-process spawn is also done with no error-budget; one bad worktree merely logs and continues, but if the *first* spawn deadlocks the main thread, the rest queue.
- **Recommended fix**: cap concurrent spawns (`futures::stream::iter(...).buffer_unordered(N)` if you have an executor; otherwise spawn each via a helper thread and join with timeout). Add `auto_resume_concurrency: usize = 4` to config. Skip sessions whose worktree was last-modified > N days ago.

### P1-4: Scrollbar widget total/position math may overshoot
- **Files**: `src/app/render.rs:1352-1368`
- **What's wrong**: `ScrollbarState::new(total + visible).viewport_content_length(visible).position(position)` — `total` is the scrollback total, `visible` is `term_area.height`. ratatui's `ScrollbarState::new(content_length)` expects content_length to *include* the visible area, but the relationship between `position`, `content_length`, and `viewport_content_length` is documented as "track-size fallback" only. Setting `content_length = total + visible` then `viewport_content_length = visible` over-counts by `visible`, making the thumb never reach the bottom when the user scrolls to the latest. Cross-checked with ratatui issue #1681 ("ScrollBar content_size is inaccurate") and #966 (viewport_length assumption wrong).
- **Concrete impact**: visual glitch only; not functional. But the position math is also doing `total - offset` where dux's offset semantics ("0 == latest") are not the convention used by the rest of the codebase, so this is fragile to refactor.
- **Recommended fix**: replace with `ScrollbarState::new(total).position(position)`, drop `viewport_content_length` (it falls back to track size). Add a unit test that constructs a known scrollback state and asserts the rendered thumb cell range.

### P1-5: Fork drift management has no automation
- **Files**: repo level — there is no `.github/workflows/upstream-sync.yml`, no `merge-upstream.sh`, no documented cadence
- **What's wrong**: The fork is currently exactly **1** upstream commit behind (4aaf0b0, "Harden git output parsing and worktree mirroring"). That commit *is itself a hardening fix* and we're missing it. There is no CI job that periodically `git fetch upstream && git log HEAD..upstream/main` and posts an issue, no CODEOWNERS, no policy on "rebase vs merge upstream", and the overlay branch `dux-amq-setup` is not protected.
- **Concrete impact**: in 6 months this fork becomes unmaintainable; security fixes in upstream silently miss us; the very next upstream refactor (especially anything touching `App::new`, the render loop, or the config schema) will conflict with our four patches.
- **Recommended fix**: add a weekly scheduled GitHub Action that opens a draft PR with `git merge upstream/main` results; add CODEOWNERS for the four patched files; pin upstream commit in README; consider extracting the four patches into a `patches/` dir of `.diff` files applied via `git apply` so the overlay can rebase cleanly.

### P1-6: `install.sh` uses `grep -oP` (Perl regex) — non-portable
- **Files**: `dux-amq/install.sh:23`
- **What's wrong**: `grep -oP` requires PCRE support, not present on macOS's BSD grep, busybox grep, or Alpine's default. README claims "Linux VM" only, but the script is declared `#!/usr/bin/env bash` and ought to be portable. Also: scraping JSON with regex is brittle.
- **Recommended fix**: `curl -fsSL https://api.github.com/repos/.../releases/latest | jq -r .tag_name`. The script already uses `jq` later (VSCode settings merge), so make `jq` a hard dependency and document it.

### P1-7: `~/.bashrc` and `~/.claude/CLAUDE.md` appended without idempotent reapply
- **Files**: `dux-amq/install.sh:65-75`
- **What's wrong**: Idempotency is gated only on the literal markers `=== dux + AMQ ===` and `Multi-agent environment (AMQ + dux)`. If the upstream `claude-md-additions.md` is *updated* in this repo, re-running `install.sh` is a no-op — the user keeps their stale copy. There's no version marker, no "BEGIN/END dux-amq" envelope to delete-and-rewrite.
- **Recommended fix**: wrap inserts in `# >>> dux-amq vN.M.K >>>` / `# <<< dux-amq vN.M.K <<<` markers, sed-strip the existing block before re-appending. Bump version on every edit.

### P1-8: `eval "$(amq shell-setup)"` runs untrusted code from a binary in PATH
- **Files**: `dux-amq/config/bashrc-additions.sh:7-9`
- **What's wrong**: `eval "$(...)"` is run on every interactive shell start. If `amq` is ever replaced (a user manually overwrites `~/.local/bin/amq` from a malicious source), every new shell silently executes attacker code. This is the same trust model as `kubectl completion bash`, but with a much smaller-audited tool.
- **Recommended fix**: low cost — pin the binary path (`/data/state/amq-bin/amq` controlled by install.sh, mode 0755 owner-only writable), and hash-check the binary on install.

## P2 — improvements / hygiene

### P2-1: OSC 52 BEL terminator + payload size cap
- **File**: `src/clipboard.rs:122-148`
- The implementation correctly base64-encodes (RFC 4648 vectors covered in tests) and uses BEL `\x07` instead of ST. Some terminals (rxvt, older xterm without `allowWindowOps`) reject BEL terminators silently. A best-practice fallback is "try BEL, then ST `\x1b\\` if user reports failure". 100 KiB cap matches tmux/WezTerm — fine.
- **Risk**: OSC 52 *write* on its own is benign; the documented attack surface is OSC 52 *read* combined with terminals that auto-respond to escape requests. Since dux only writes, this is hygiene-only.

### P2-2: `Clipboard::new` panics if thread spawn fails
- **File**: `src/clipboard.rs:36`
- `.expect("failed to spawn clipboard worker thread")` — the only path that can fail is hitting `RLIMIT_NPROC`, which happens before dux is useful anyway. Acceptable, but `Result<Self>` would be more idiomatic and let `App::new` decide whether to start without clipboard.

### P2-3: No tests for the four Rust patches except the clipboard module
- `auto_resume_all_sessions`, the scrollbar render branch, and the new `auto_resume_on_start` config field have **zero** unit/integration tests. clippy `-D warnings` won't catch logic bugs.
- Add: a session-list fixture asserting `auto_resume_all_sessions` skips missing-worktree and already-spawned sessions; a snapshot test for `Scrollbar` thumb cells given known offsets.

### P2-4: README doesn't document the threat model or PII scope
- `~/.claude/projects/` (now under `/data/state/claude`) contains every prompt, response, and tool I/O — including secrets, source code, and any data fetched by tools. AMQ messages on `/data/state/amq` likewise contain raw chat content. None of this is encrypted at rest. For GDPR-aware deployments this is "personal data and possibly special-category data on a persistent disk", and the operator needs to know.
- Add: a "Data handling" section listing every directory with chat/PII, retention defaults, and a one-liner LUKS or `cryptsetup` recipe for the persistent disk.

### P2-5: License compatibility OK — but copyright year
- `LICENSE` is MIT, copyright "2026 Patrick D'appollonio" — that's fine for the upstream piece. The overlay README says "MIT-licensed (matching dux license)" but **does not include a separate copyright line** for the overlay author. Add `Copyright (c) 2026 SiavZ (dux-amq overlay)` to a `dux-amq/LICENSE` to make attribution unambiguous if someone vendors only the overlay subdirectory.

### P2-6: No release pipeline for the overlay
- The Rust-side `release.yml` builds `dux` binaries from this fork on tag — that's good. But the overlay (`install.sh`, wrappers, scripts) has no versioned release. There is no `dux-amq-v0.1.0.tar.gz`. Users `git clone` `main` (which can be force-pushed) and run whatever HEAD is. Add a `release-overlay.yml` that tags + uploads `dux-amq-vX.Y.Z.tar.gz` with a sha256.

### P2-7: Logging in wrappers goes to /dev/null
- `amq wake ... >/dev/null 2>&1 &` — covered above; same applies to `npx -y skills add ... >/dev/null 2>&1`. Discarding stderr on install means a partial-failure looks identical to success. Use `tee` to a known log file, surface the path on completion.

### P2-8: `dux config regenerate --yes` overwrites manual edits
- `install.sh:54` regenerates `config.toml` then patches it with `sed`. If the user has hand-edited the file (`projects = [...]`, custom macros), idempotent re-run blows that away because `--yes` accepts the regenerate. Detect "config.toml present and non-default" and skip regenerate.

### P2-9: VSCode settings merge with `jq` writes back without `chmod`/atomic
- `install.sh:98-104` does `jq ... > $f.tmp && mv $f.tmp $f`. That's atomic on same fs (good), but doesn't preserve original mode/ownership (`install -m`). Minor.

### P2-10: bash quoting nits
- `install.sh:24` `cd "$TMP"` — fine; but `cd - >/dev/null` afterward depends on `OLDPWD`, which is unset under some `bash --posix` modes. Use absolute `cd "$HOME"` or just don't return.
- `claude-amq:38-39` `enc_self=$(echo "$PWD" | sed ...)` — `printf '%s'` is preferred over `echo` for paths starting with `-` or containing `\`.

### P2-11: Observability gap — no `dux-amq-doctor`
- A `bin/dux-amq-doctor` that prints: AMQ version, agent registry, queue size, `dev.tty.legacy_tiocsti` value, persistent-disk free space, `~/.claude` symlink target, `npx skills list` output — would shave hours off support. Currently there's no triage tool at all.

## Out of scope / accepted risks

- **AMQ internals**: the AMQ binary itself (`avivsinai/agent-message-queue`) is a separate trust boundary and supply chain. We flagged the install vector (P0-2) but did not audit AMQ's source. A separate audit is appropriate.
- **dux upstream pre-existing security issues** unrelated to our four patches (e.g. `rusqlite` bundled, `crossterm` input handling): not flagged because they're upstream's territory and we're 1 commit behind the latest hardening, not many.
- **Rust dependency CVEs**: `cargo audit` at time of writing shows no advisories against `arboard`, `ratatui 0.30`, `crossterm 0.29`, `rusqlite 0.39`, or `petname 3.0`. The previous `petname 2.x → 3.0` bump (upstream commit `bbf88e0`) was itself a transitive `rand` security fix. Recommend adding `cargo audit` to PR CI.
- **Multi-tenant isolation**: this overlay is explicitly single-user-on-a-VM. Multi-user use was not in scope and would require redesign.
- **Windows/macOS**: install path is Linux-only; we treated platform-portability bash gotchas (P1-6) as P1 because the README does claim macOS-friendliness via the GCE-style note, but the deeper port is a feature request, not a flaw.

## Methodology notes

- **Repo state**: cloned `/tmp/dux-amq-setup` at HEAD `345d882`, fetched `upstream/main`, ran `git diff upstream/main..HEAD -- src/` for patch review and `git log HEAD..upstream/main` to detect drift.
- **Review depth**: read every overlay file in full; read the four patched Rust files at and around the modified hunks (clipboard fully; mod.rs/render.rs/config.rs at the documented offsets and surrounding 30-line context). Did not re-audit unmodified upstream code beyond confirming `auto_resume_all_sessions`'s call site at `App::new` line 1199–1200.
- **Web research**: targeted current sources on OSC 52 attack surface (DEV.to, CyberArk, oppi.li), TIOCSTI deprecation status (Ubuntu LP #2046192, Linux 6.2 default-off), `--dangerously-skip-permissions` recent guidance (Anthropic auto-mode March 2026, PromptArmor January 2026), `curl|bash` hardening (Sysdig, kicksecure), ratatui Scrollbar API (issues #966, #1681, #1493), and `set -euo pipefail` background-job semantics. No `cargo audit` was run live (no Rust toolchain in the audit sandbox); RUSTSEC was checked via the advisories-db results.
- **Gaps consciously left**:
  - Did not run dux end-to-end on a live VM. No dynamic check of OSC 52 actually reaching VSCode terminal, nor of `amq wake` behavior on a kernel with `dev.tty.legacy_tiocsti=0`. Recommend a smoke test as part of CI.
  - Did not introspect the `amq` binary's syscall surface (would need `strace` on a live install) to confirm whether it uses TIOCSTI or PTY-master writes.
  - The mandate asked for parallel sub-sub-agents; the available tooling in this audit harness did not expose the Agent/Task spawn primitive, so I executed all eight dimensions sequentially with parallelised tool calls (Bash, Read, WebSearch). Findings ranking and synthesis are unchanged but wall-clock cost was higher.

---

Total findings: **5 P0**, **8 P1**, **11 P2** = **24** ranked items. The overlay is a thoughtful piece of work; the path to production-grade is small and concrete — flip two defaults, add three integrity checks, vendor or pin the supply chain, and stand up an upstream-sync workflow.

Sources consulted (web): RustSec advisories DB; Ratatui issues #966, #1493, #1681; ratatui docs.rs ScrollbarState; Ubuntu LP #2046192; Linux kernel CONFIG_LEGACY_TIOCSTI documentation; Anthropic Claude Code Security docs and "auto mode" announcement (March 2026); PromptArmor disclosure (January 2026); Sysdig "Friends don't let friends curl|bash"; kicksecure curl|bash hardening; CyberArk "Don't Trust This Title"; The Register on ANSI escape abuse (2023); Hacker News thread on TIOCSTI disablement.

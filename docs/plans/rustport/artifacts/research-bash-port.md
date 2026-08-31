# Bash → Rust port research brief (`dux-amq-rust`)

Scope: the ~3,600 lines of bash under `dux-amq/` (3 wrappers, 8 scripts, 2 installers,
4 config fragments) that must be reimplemented in the empty `dux-amq-rust/` crate.

Companion artifacts in this directory:

- `00-baseline.md` — build health, toolchain drift (1.88.0 pinned vs 1.98.0 stable), source census.
- `bats-test-parity-matrix.md` — **the acceptance criteria**: 123 tests across 14 `.bats`
  files, harness/fakes spec, coverage map, confirmed-untested behaviors, and 10 porting
  hazards. This brief cross-references it rather than restating it.

Method note: three sub-agents read the wrappers, the scripts, and the installers in full
(`cat -n`, line-by-line) with an explicit anti-assumption mandate; a fourth produced the
bats matrix. Every load-bearing claim below carries a `file:line` cite. Where a briefing
premise turned out to be wrong against the current tree, that is called out explicitly.

**Two briefing premises were falsified and must not be ported:**

1. *"`claude-amq:189` runs unlocked (flock-absent fallback)."* False. Line 189 is handle
   normalization (`if (( MANAGED_IDENTITY == 0 )); then`). The flock check is at
   `claude-amq:269-272` and is a **hard fail-closed `exit 1`** when `flock` is missing.
   No code path in any wrapper touches `$ROOT/agents/$ME` or `meta/config.json` without a
   resolved `flock` binary. Reproducing a "fallback" would be a security regression.
2. *"`claude-amq:374` starts a wake daemon."* Misleading. `claude-amq:390` backgrounds
   `dux-amq-inject-bridge` with a bare `&`. That process is a **bounded 5-second poll
   loop**, not a daemon: no `setsid`, no `nohup`, no `disown`, no PID file of its own, and
   it self-terminates.

---

## Behavioral specs

### 0. Shared invariants across all 11 shell entry points

| Invariant | Detail |
|---|---|
| Shebang / options | `#!/usr/bin/env bash` + `set -euo pipefail` everywhere. |
| Default form | **Only `${VAR:-x}`** (colon-dash) is used anywhere in the wrappers — verified by grep across all 861 wrapper lines. The default therefore fires for *set-but-empty* as well as unset. A Rust port must treat `Some("")` as "absent" for every env read. |
| `STATE_ROOT` | `${STATE_ROOT:-/data/state}` — the root of everything. |
| `DUX_HOME` | `${DUX_HOME:-$STATE_ROOT/dux}` |
| `AMQ_GLOBAL_ROOT` | `${AMQ_GLOBAL_ROOT:-$STATE_ROOT/amq}`, re-exported as `AM_ROOT`. |
| `LOCAL_BIN` | wrappers/bridge: `${LOCAL_BIN:-$HOME/.local/bin}`. **`dux-amq/install.sh:24` is the exception** — it hardcodes `"${HOME}/.local/bin"` with no env override. |
| Handle charset | `^[a-z0-9_-]+$`, max 64 chars. |

---

### 1. `wrappers/claude-amq` (393), `codex-amq` (242), `gemini-amq` (226)

All three share one skeleton; differences are concentrated in four places (oneshot
detection, YOLO flag, system-prompt handling, Claude-only seeding).

#### 1.1 Common control flow (claude-amq line numbers; codex/gemini in parens)

1. **`version_at_least(actual, minimum)`** — `claude-amq:22-31` (codex 9-18, gemini 9-18).
   Splits both on `.` via `IFS=. read -r maj min patch <<<"$str"`. All three components of
   `actual` must match `^[0-9]+$` or it returns 1. Comparison forces base 10 (`10#$x`) so
   zero-padded components are not read as octal.
2. **`require_provider_version(provider, minimum)`** — `claude-amq:33-51` (codex 19-38,
   gemini 19-38).
   - `command -v "$provider"` missing → stderr `'%s-amq: %s is not installed or not on PATH\n'`, `exit 1`.
   - `output=$("$provider" --version 2>&1 || true)` — **`set -e` deliberately defused**; a
     nonzero `--version` exit is tolerated and its output still parsed.
   - `actual` = **first** match of `[0-9]+\.[0-9]+\.[0-9]+` anywhere in combined
     stdout+stderr (`grep -Eo ... | head -1`, also `|| true`). "First version-shaped
     substring wins" is the exact semantics — not "the real version".
   - Empty → `exit 1`, message embeds the raw output with **`printf %q`** shell-escaping:
     `'%s-amq: refusing %s because its version could not be parsed from %q; minimum supported is %s\n'`.
   - Below floor → `exit 1`: `'%s-amq: refusing vulnerable %s %s; upgrade to >= %s\n'`.
   - **Floors:** claude `2.1.163` (`claude-amq:53`), codex `0.39.0` (`codex-amq:40`),
     gemini `0.39.1` (`gemini-amq:40`). Runs **before** the oneshot short-circuit, so
     `--version` is always shelled out once even for oneshot invocations.
3. **`is_dux_worktree()`** — `claude-amq:55-81` (codex 45-53, gemini 45-53). Byte-identical
   across all three (pinned by `encoder-fixtures.bats` L148). `realpath --` both `$PWD` and
   `${DUX_HOME:-$STATE_ROOT/dux}/worktrees`, each `|| return 1`. Containment is a `case`
   glob of `"$pwd_real/"` against `"$worktrees_real"/*` — the trailing-slash idiom enforces
   a **path-segment** boundary, defeating the `/worktrees-evil/...` prefix collision
   (audit01 P0-5).
4. **Oneshot short-circuit** — provider-specific, see 1.2.
5. **Seeding** — Claude only, see 1.3.
6. **Identity resolution** — `claude-amq:163-195` (codex 59-83, gemini 61-85):
   - If **any** of `DUX_STORE_ID` / `DUX_SESSION_ID` / `DUX_AMQ_HANDLE` is non-empty, **all
     three** must be non-empty or `exit 1`:
     `'<prog>-amq: DUX_STORE_ID, DUX_SESSION_ID, and DUX_AMQ_HANDLE must be set together\n'`.
     Then `ME="$DUX_AMQ_HANDLE"`, `MANAGED_IDENTITY=1`.
   - Else `ME="${AM_ME:-}"`, `MANAGED_IDENTITY=0`.
   - Fallback chain when `ME` empty: (a) `is_dux_worktree` → `basename "$PWD"`;
     (b) `git -C "$PWD" symbolic-ref --quiet --short HEAD 2>/dev/null || true`;
     (c) `"<provider>-$$"` (the wrapper's own PID).
   - **Normalization only when `MANAGED_IDENTITY == 0`** (`claude-amq:187-191`):
     `tr '[:upper:]' '[:lower:]' | sed 's|[^a-z0-9_-]|-|g; s|^-\+||; s|-\+$||'`
     — lowercase, one-for-one substitution (**runs are NOT collapsed**), then strip a
     leading run of `-` and a trailing run of `-`.
   - Validation applies to **both** kinds (`claude-amq:192-195`): non-empty,
     `${#ME} <= 64`, `^[a-z0-9_-]+$` — else `exit 1`
     `'<prog>-amq: refusing invalid or overlong AMQ handle\n'`. A managed handle containing
     uppercase or symbols is **rejected, never repaired** (pinned by `ownership.bats`).
7. **Exports** — `export AM_ME="$ME"` (201/87/89); `ROOT="${AMQ_GLOBAL_ROOT:-$STATE_ROOT/amq}"`,
   `export AM_ROOT="$ROOT"` (203-204/89-90/91-92);
   `ME_REGISTRATION="$ROOT/agents/$ME/.dux-amq-source"` (206/92/94);
   `FLOCK_BIN="${DUX_AMQ_FLOCK:-flock}"` (207/93/95).
8. **`claim_and_register_locked()`** — `claude-amq:209-266` (codex 96-146, gemini 98-148).
   Function body is byte-identical across all three. See 1.4.
9. **Preconditions** — `mkdir -p "$ROOT/meta" "$ROOT/agents" || exit 1` (268/148/150), then
   `command -v "$FLOCK_BIN"` → `'<prog>-amq: mandatory flock utility is unavailable\n'`,
   `exit 1` (269-272); then `command -v jq` → `'<prog>-amq: jq is required for atomic AMQ registration\n'`,
   `exit 1` (273-276). **Ordering trap:** the `mkdir -p` runs *before* both checks, so a
   run that aborts for missing `flock` still leaves `meta/` and `agents/` created.
10. **The lock** — see 1.5.
11. **Flag assembly** — see 1.6.
12. **Bridge / transport resolution** — `claude-amq:323-377` (codex 157-188, gemini 159-190):
    - `LOCAL_BIN="${LOCAL_BIN:-$HOME/.local/bin}"`, `BRIDGE="$LOCAL_BIN/dux-amq-inject-bridge"`;
      if not `-x`, `BRIDGE=$(command -v dux-amq-inject-bridge 2>/dev/null || true)` (empty on miss).
    - `INJECT_MODE="${DUX_AMQ_INJECT_MODE:-}"`; if empty,
      `[[ -f "$STATE_ROOT/dux/.tiocsti-state" ]] && INJECT_MODE=via || INJECT_MODE=raw`.
    - `WAKE_ARGS=(--require-wake -y)`, then `case "$INJECT_MODE"`:
      `raw` → `+= (--wake-inject-mode raw)`;
      `via` → require `-x "$BRIDGE"` else `exit 1`
      `'<prog>-amq: dux-amq-inject-bridge not found; refusing to start without wake\n'`,
      then `+= (--wake-inject-via "$BRIDGE")`;
      **anything else** → stderr `%q`-quoted warning `'... %q not understood; using raw\n'`
      and falls through to the `raw` args (does **not** exit).
13. **Dead-owner recovery** — `claude-amq:379-383` (codex 224-228, gemini 212-216):
    ```
    if jq -e '.owner.pid | numbers' "$ROOT/agents/$ME/.wake.lock" >/dev/null 2>&1; then
      amq wake recover-owner --root "$ROOT" --me "$ME" --strict >/dev/null
    fi
    ```
    The `recover-owner` call is **not** `||`-guarded and is **not** the `if` condition, so a
    nonzero exit propagates through `set -e` and kills the wrapper with that exact code
    (`wrappers-p1.bats` uses 42 as the stand-in). No wrapper-level message is printed.
14. **Startup backlog bridge** — `claude-amq:385-391` (codex 230-236, gemini 218-224):
    ```
    if [[ -n "${DUX_PANE:-}" && "${DUX_AMQ_VERIFY:-0}" != "1" ]]; then
      [[ -x "$BRIDGE" ]] || { printf '<prog>-amq: dux-amq-inject-bridge not found; refusing to start without backlog recovery\n' >&2; exit 1; }
      DUX_AMQ_STARTUP_OWNER_PID="$$" "$BRIDGE" &
    fi
    ```
    Prefix-assignment scopes `DUX_AMQ_STARTUP_OWNER_PID` to that child only. The bridge is
    invoked with **zero argv**, inherits the wrapper's stdio unchanged, is never `wait`ed
    on, and is **not** `setsid`/`nohup`/`disown`ed. The very next statement is the `exec`.
15. **`exec`** — see 1.7.

#### 1.2 Oneshot short-circuit (provider-specific)

| Wrapper | Detection | Cite |
|---|---|---|
| claude-amq | `for arg in "$@"` scanning **every** arg for literal `-p`, `--print`, or `--bare` → `exec claude "$@"` | 83-87 |
| codex-amq | `[[ "${1:-}" == "exec" ]]` — **first positional only**, matching Codex's own subcommand → `exec codex "$@"` | 55-57 |
| gemini-amq | scans every arg for `-p` / `--print` (**no `--bare`**) → `exec gemini "$@"` | 55-59 |

Hazard: the claude/gemini scan is context-free — `-p` appearing as the *value* of a
preceding option also triggers the oneshot path. Preserve or fix deliberately.

#### 1.3 `seed_session_history()` — Claude only (`claude-amq:89-161`)

Gate: `[[ "${CLAUDE_AMQ_SEED_FROM_PARENT:-}" == "1" ]] || return 0` (91). Called as
`seed_session_history || true` (161) — belt-and-suspenders over its own `return 0` paths.

1. `git_dir=$(git -C "$PWD" rev-parse --absolute-git-dir 2>/dev/null) || return 0` (94)
2. `git_common_dir=$(git -C "$PWD" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) || return 0` (95)
3. `[[ "$git_dir" == "$git_common_dir" ]] && return 0` (96) — main checkout, nothing to seed.
4. `main_worktree` = 2nd field of the first `^worktree ` line of `git worktree list --porcelain` (97-98);
   empty or `== "$PWD"` → return 0 (99).
5. `command -v encode-claude-project-dir` missing → stderr `'claude-amq — encode-claude-project-dir not on PATH; skipping seed\n'`, return 0 (107-110).
6. `pwd_str=$(printf '%s' "$PWD")`, `main_str=$(printf '%s' "$main_worktree")` — `printf '%s'`
   rather than `echo` so a leading `-n`/`-e` is not eaten (P2-3, 111-116).
7. `enc_self=$(encode-claude-project-dir -- "$pwd_str") || return 0`, same for `enc_main` (117-118).
8. `self_dir="$HOME/.claude/projects/$enc_self"`, `main_dir="$HOME/.claude/projects/$enc_main"` (119-120).
9. `[[ ! -d "$main_dir" ]] && return 0` (122); `compgen -G "$self_dir/*.jsonl"` succeeds → already seeded, return 0 (123-125).
10. `mkdir -p "$self_dir"` (127); `rsync_log=$(mktemp -t claude-amq-seed.XXXXXX)` or return 0 with
    `'claude-amq — could not allocate tempfile for seed log; skipping seed'` (136-139).
11. `rsync -a "$main_dir"/ "$self_dir"/ 2>"$rsync_log"` if `rsync` on PATH, else
    `cp -r "$main_dir"/. "$self_dir"/ 2>"$rsync_log"` (140-150). On nonzero:
    `'claude-amq — seed partial (rsync rc=$seed_rc); see $rsync_log'` (or `cp rc=`).
12. `n=$(find "$self_dir" -name '*.jsonl' | wc -l)` (152). If `[[ -s "$rsync_log" ]]`:
    `warn_lines=$(wc -l <"$rsync_log" | tr -d '[:space:]')` and print
    `'claude-amq — seeded $n files (with $warn_lines rsync warnings; see $rsync_log)'`,
    **keeping the log file**. Else print `'claude-amq — seeded $n past sessions from $main_worktree'`
    and `rm -f "$rsync_log"` (153-159).

All output is stderr. Seeding never fails the wrapper.

#### 1.4 `claim_and_register_locked()` — the registration critical section

Runs entirely inside the flock. Byte-identical across the three wrappers.

- Rejects a symlinked or non-directory `$ROOT/agents/$ME` as **foreign**:
  `'[dux-amq] identity collision: %s has a foreign ownership marker\n'`.
- Rejects an existing agent dir with **no** marker:
  `'[dux-amq] identity collision: %s has an ownerless agent directory\n'` (two call sites).
- Existing marker owned by a different `$PWD`/session:
  `'\033[1;31m!\033[0m [dux-amq] identity collision: %s already registered to %s (current: %s).\n'`.
- `mkdir -m 0700` on create; `chmod 0700` reapplied unconditionally.
- **Marker file type is polymorphic** — a Rust port must `lstat` and branch:
  - standalone identity → a **symlink**: `ln -s "$PWD" "$ME_REGISTRATION"`;
  - managed identity → a **regular JSON file** `{store_id, session_id}`, staged via
    `mktemp "$agent_dir/.owner.tmp.XXXXXX"` then `mv`, with `rm -f "$tmp"` on the failure branch.
- `$ROOT/meta/config.json` is read, merged with `jq`, written to
  `mktemp "$ROOT/meta/config.json.tmp.XXXXXX"` and `mv`'d into place. **The final `mv` has no
  `|| return 1` and the tmp file has no `rm -f` on that failure path** — asymmetric with the
  `.owner.tmp` cleanup; relies entirely on the nested shell's `-e` to halt.
- Failure paths `return 1`; `bash -c`'s status propagates through `flock` to the wrapper's
  `set -e`, aborting with that exact code and **no additional wrapper-level message**.

**Cross-process contract with the Rust dux side.** `src/peer.rs` implements the same
protocol: `AmqRegistryLock::acquire` opens `<root>/meta/config.lock`
(`create(true).read(true).write(true).truncate(false)`) and takes
`rustix::fs::flock(fd, FlockOperation::LockExclusive)` (`src/peer.rs:722-731`);
`OWNER_MARKER = ".dux-amq-source"` (`src/peer.rs:28`) is parsed as
`{store_id, session_id, wake_pid?}` JSON, a **symlink** marker is classified as
`MarkerState::Legacy` (preserved, never reclaimed), and anything else is `Foreign`
(`src/peer.rs:930-958`). `src/peer.rs::rust_and_wrapper_claims_serialize_on_config_lock`
(2076-2114) is a **Rust test that spawns the bash `claude-amq` wrapper** and asserts it
blocks on the held lock and only then writes `.dux-amq-source`. That test must keep passing
against the Rust wrapper.

#### 1.5 Locks — exactly one per wrapper

```
"$FLOCK_BIN" -x "$ROOT/meta/config.lock" bash -euo pipefail -c claim_and_register_locked
```
`claude-amq:279`, `codex-amq:153`, `gemini-amq:155`.

- **Pathname form** of `flock(1)` — flock opens/creates the file itself; no fd number.
- **Exclusive (`-x`), blocking** — no `-n`, no `-w`. A contender waits indefinitely.
- Critical section = the whole function: agent-dir validation/creation, marker write,
  `chmod`, and the `config.json` read-merge-write. All providers contend on the **same**
  `meta/config.lock`, so registration is serialized cross-provider.
- The body runs in a **fresh bash process**; it is available only because of
  `export -f claim_and_register_locked` (`claude-amq:278`) via the `BASH_FUNC_*` env
  mechanism. There is **no Rust equivalent of exporting a closure** — inline it.
- `export ROOT ME ME_REGISTRATION DUX_STORE_ID DUX_SESSION_ID` (277/151/153) exports
  `DUX_STORE_ID`/`DUX_SESSION_ID` **even when unset**, so the child sees them as
  exported-empty rather than absent.
- **`flock` absent → hard `exit 1`.** `DUX_AMQ_FLOCK` only lets a test substitute a
  different binary name; it is not a bypass the script chooses.

#### 1.6 Flag assembly (provider-specific)

| Concern | claude-amq | codex-amq | gemini-amq |
|---|---|---|---|
| YOLO env | `${CLAUDE_AMQ_YOLO:-${CLAUDE_YOLO:-}} == "1"` (285) | `${CODEX_AMQ_YOLO:-${CLAUDE_YOLO:-}} == "1"` (195) | **none — no YOLO surface exists** |
| YOLO flag | `--dangerously-skip-permissions` | `--dangerously-bypass-approvals-and-sandbox` | — |
| Extra flags | `--dangerously-load-development-channels server:claude-peers` unless `CLAUDE_PEERS_DISABLE == "1"` (290-297) | `--dangerously-bypass-hook-trust` iff `CODEX_AMQ_BYPASS_HOOK_TRUST == "1"` (200-213) | — |
| `DUX_SYSTEM_PROMPT` | `--append-system-prompt "$DUX_SYSTEM_PROMPT"` when non-empty (299-314); value verbatim, newlines survive | warn-only, flag dropped (220-222) | warn-only, flag dropped (192-210) |
| `CLAUDE_AMQ_SAFE` | any value → deprecation warning, **no behavioral effect** (316-321) | — | — |

`CLAUDE_YOLO` is a **shared legacy toggle** — one variable flips both the claude and codex panes.

#### 1.7 `exec` semantics

All three `exec` (replace the process image), never fork, for both the oneshot path and the
final launch.

```
# claude-amq:393
exec amq coop exec --no-init --root "$ROOT" --me "$ME" "${WAKE_ARGS[@]}" claude -- ${EXTRA[@]+"${EXTRA[@]}"} "$@"

# codex-amq:238-242  (two-branch instead of the nounset-safe idiom)
if (( ${#CODEX_ARGS[@]} > 0 )); then
  exec amq coop exec --no-init --root "$ROOT" --me "$ME" "${WAKE_ARGS[@]}" codex -- "${CODEX_ARGS[@]}"
else
  exec amq coop exec --no-init --root "$ROOT" --me "$ME" "${WAKE_ARGS[@]}" codex --
fi

# gemini-amq:226
exec amq coop exec --no-init --root "$ROOT" --me "$ME" "${WAKE_ARGS[@]}" gemini -- "$@"
```

Order matters and is pinned by ~60 argv assertions: `EXTRA` accumulates YOLO →
peers-channel → system-prompt; `CODEX_ARGS` accumulates hook-trust → YOLO; the user's
`"$@"` is appended **last** in every case. The provider name is a positional argument to
`amq coop exec`; the `--` marks the start of the provider's own argv.

#### 1.8 Traps, temp files, exit codes

- **Zero traps.** `grep` for `trap|SIGINT|SIGTERM|disown|setsid|nohup` across all 861 wrapper
  lines returns nothing. A Ctrl-C during the pre-`exec` phase can leave a half-created
  `agent_dir` or a stray tmp file.
- Temp files: `claude-amq-seed.XXXXXX` (removed only on a fully clean seed),
  `.owner.tmp.XXXXXX` (cleaned on its own failure), `config.json.tmp.XXXXXX` (**not**
  cleaned on final-`mv` failure).
- Exit codes: every explicit failure is `exit 1`, except `recover-owner`'s propagated code
  and `claim_and_register_locked`'s propagated `return 1`. Exact stderr strings are pinned
  — see `bats-test-parity-matrix.md` §4 hazard 3 for the full list.

---

### 2. `scripts/dux-amq-doctor` (850)

#### 2.1 CLI and exit policy

`--json` → `JSON=1` (31); `--anonymize` → `ANON=1` (32); `-h|--help` → usage heredoc to
**stdout**, `exit 0` (34-47). **Any other arg** → `printf 'doctor: ignoring unknown arg: %s\n' "$1" >&2` and
continue (49) — there is no way to make the script exit nonzero via argv.

**`exit 0` is unconditional** (line 850). No WARN/FAIL changes it. This matters because
`src/cli.rs::run_doctor` only merges the bash JSON when `output.status.success()`.

#### 2.2 Timeout machinery

`timeout` is shadowed by a shell function (65-73) resolving the real binary once via
`command -v timeout || command -v gtimeout || true` (64). **If neither exists the wrapped
command runs unbounded** — the "never hangs" guarantee silently degrades. `to()` (87-99)
runs `timeout "$N" "$@" 2>/dev/null` and maps exit 124 → `"(timeout)"`, 127 →
`"(not found)"`, other nonzero → `"(error)"`. `is_err()` (104-109) matches those three plus
the empty string. `DOCTOR_TIMEOUT_SEC` defaults to 5 (88); the `du` call overrides it to 15 (418).

#### 2.3 Section inventory (call order fixed at 786-794)

| # | Section | Runs | OK/WARN/FAIL | JSON keys |
|---|---|---|---|---|
| 1 | Versions (283-341) | `dux --help \| head -1` (via `to bash -c`; `--version` is unimplemented and would trigger a sqlite migration), `amq/claude/codex/gemini --version` | informational | `versions.{dux,amq,claude,codex,gemini,overlay}` |
| | | `overlay` is **scraped** by `awk` from `^DUX_AMQ_VERSION=` in `${DUX_AMQ_INSTALL_SH:-$SCRIPT_DIR/../install.sh}` (306-317), extracting the `:-X` default or a literal quoted string; `"(unknown)"` on miss. Fallbacks: dux/amq → `"(unknown)"`, providers → `"(missing)"` | | |
| 2 | Binary integrity (346-395) | `sha256sum`/`shasum -a 256` of `${AMQ_BIN:-$STATE_ROOT/amq-bin/amq}` vs first field of `$AMQ_GLOBAL_ROOT/binary.sha256` | `match` ok / `mismatch` red / `binary-missing` plain / `record-missing` warn | `binary_integrity.{status,expected,actual,tiocsti_sentinel}` |
| | | Also reports `$DUX_HOME/.tiocsti-state` as `present`/`absent`, informational only | | |
| 3 | Persistent disk (400-446) | `df -h "$STATE_ROOT" \| tail -n +2 \| head -1`; `du -sh "$STATE_ROOT"/* \| sort -hr \| head -5` at 15 s | informational | `disk.df`, `disk.top_dirs` |
| 4 | AMQ (451-561) | gate on `$AMQ_GLOBAL_ROOT/meta/config.json`; enumerate `agents/*` **directories** (not config.json's list); per agent depth via `find inbox/new -maxdepth 1 -type f \| head -101 \| wc -l` capped as `"100+"`; oldest age from `find -printf '%T@'` | `100+` red; oldest ≥60 min red; any pending <60 min yellow; empty dim | `amq.{initialized,queue_root,agent_count}`, `amq.agents[]` = `{name,depth,oldest_age_min}` |
| | | Text line: `printf '  %-30s %s%-5s%s msg%s\n' name color depth C_OFF suffix`, suffix = `" (oldest 3h12m)"` or empty | | |
| 5 | Symlinks (566-603) | `$HOME/{.claude,.agents,.codex,.gemini}` | `ok` (target starts `"$STATE_ROOT/"`) / `off-persistent` warn / `not-symlink` warn / `missing` | `symlinks` object keyed by basename → `{target,state}` |
| 6 | Kernel (608-634) | `uname -r`; TIOCSTI knob from `/proc/sys/dev/tty/legacy_tiocsti` else `sysctl -n dev.tty.legacy_tiocsti` else `"(unsupported-kernel)"` | **plain `kv()` only** — the inline comment (618-620) says it should be a security warning but no `warn()`/`bad()` is ever called | `kernel.{release,legacy_tiocsti}` |
| 7 | Sessions DB (639-678) | `$DUX_HOME/sessions.sqlite3`; `sqlite3 -readonly` for `PRAGMA journal_mode`, `PRAGMA integrity_check`, `select status,count(*) from agent_sessions group by status` | `sqlite3-cli-missing` when the CLI is absent | `sessions_db.{present,journal_mode,integrity,counts}` |
| 8 | Runtime (683-709) | first whitespace field of first line of `$DUX_HOME/dux.lock`; `kill -0`; `ps -o rss=` and `ps -o etimes=` | stale → text shows `"$pid (stale — process not running)"` | `runtime.{dux_pid,uptime_s,rss_kb}` |
| 9 | Recent errors (714-782) | `$DUX_HOME/dux.log` + `dux.log.*` under `nullglob`; ≤1000 tail lines; `jq 'select(.level=="ERROR")'`; last 5. Each path is `%q`-quoted before interpolation into a `bash -c` string (738) | informational | `recent_errors` array |

Text line: `printf '  %s %s\n' ts msg`, with `ts = .timestamp // .ts // ""` and
`msg = .fields.message // .message // .msg // (.fields|tostring) // ""`.

**Three JSON-shape defects to decide on, not blindly replicate:**
- `disk.top_dirs` is an **array** normally but a bare sentinel **string** when `du` failed (443).
- `runtime.dux_pid` is hardcoded `0` in JSON for a stale lock even though the real PID is
  known and shown in text mode (695) — information loss unique to `--json`.
- `amq.agents[].depth` is always a **string** (can be `"100+"`), never a number (556-558).

**One real leak:** with `--json` and **without** `--anonymize`, `recent_errors` re-serializes
the raw parsed JSON-Lines objects verbatim via `jq -s '.'` (779) with no field allowlist. With
`--anonymize` only `{timestamp, message}` are emitted (766-776).

#### 2.4 Anonymizer (123-238)

- `anon_branch`/`anon_agent` (144-200): first-seen-first-numbered (`branch-1`, `agent-1`, …),
  backed by parallel arrays seeded with a dummy `""` at index 0 so real entries start at 1.
  Mutations only survive via an explicit `-v <outvar>` argument — calling without one inside
  a subshell loses the counter.
- `anon_text` (208-238): literal substring replace `$HOME` → `/HOME` (**not** path-boundary
  aware), then a loop peeling each `$DUX_HOME/worktrees/<branch>/...` → `/WT/<alias>/...`,
  reusing the branch cache so repeats map to the same alias.
- JSON emit applies `anon_text` **again** to every string value (841). Idempotent, but
  redundant — do not replicate "anonymize twice" as if it were load-bearing.

#### 2.5 Integration contract with `dux doctor` (must not break)

`src/cli.rs:864-1007`:
- Resolution order: `DUX_AMQ_DOCTOR_BIN` env (if the path exists) → `which dux-amq-doctor`
  → `<exe-dir>/../dux-amq/scripts/dux-amq-doctor`.
- JSON mode: runs `<script> --json [--anonymize]`, requires `status.success()`, parses stdout
  as JSON, requires the **root to be an object**, then shallow-merges the Rust
  `sessions_db_rust` key over it (`merge_doctor_json`). Non-zero exit → warning on stderr and
  Rust-only JSON.
- Text mode: runs `<script> [--anonymize]` letting it own stdout (so its TTY colour detection
  works), then appends the Rust `== Sessions DB (Rust-side) ==` block.

So the Rust doctor must: exit 0 always, emit a JSON **object** at the root with the nine keys
above, and accept exactly `--json` / `--anonymize` (ignoring unknown args with the same
stderr line).

---

### 3. `scripts/dux-amq-inject-bridge` (307)

#### 3.1 Startup backlog recovery (91-119) — the "daemon"

There is **no PID file owned by this script and no persistent daemon.** It is a one-shot
5-second poll loop, backgrounded by the wrappers.

- Guard (92-97): `DUX_AMQ_STARTUP_OWNER_PID` must be all digits, `AM_ROOT`/`AMQ_GLOBAL_ROOT`
  non-empty, `AM_ME` matching `^[a-z0-9_-]+$` — else stderr + **`exit 0`** (not an error).
- `ROOT="${AM_ROOT:-${AMQ_GLOBAL_ROOT}}"` (98) — **no final hardcoded fallback here**
  (contrast line 229, below).
- `LOCK="$ROOT/agents/$AM_ME/.wake.lock"`, `PREPARED="$ROOT/agents/$AM_ME/.wake.prepared"`
  (99-100). Both are written by the external `amq` binary.
- Loop: **50 iterations × `sleep 0.1` = 5 s hard bound** (102-113).
  - **Staleness = `kill -0 "$STARTUP_OWNER_PID" 2>/dev/null || exit 0`** (103). That is the
    whole staleness mechanism — a liveness check on an *external* PID, not a self-owned file.
  - `jq -er` reads `.owner.pid` and `.generation` from `$LOCK`; requires `.owner.pid` to
    equal the expected owner **and** `$PREPARED`'s `.generation` to match (104-108). Only
    then `ready=1`.
  - **Race:** between `kill -0` and the `jq` read the owner can exit and its PID be reused.
    The **generation equality check is the actual safety net** — port that, not just the
    liveness check.
- Timeout → `'...managed wake was not ready within 5s; backlog remains unread\n'` on stderr,
  `exit 0`. Backlog loss is accepted silently.
- Success → `ENVELOPE='[AMQ] startup backlog recovery'` (118), a synthetic marker that flows
  into the raw branch of the decoder (it matches neither magic).

#### 3.2 Receiver sanitization (129-135)

```
sanitised_receiver=$(printf '%s' "$AM_ME" | tr '[:upper:]' '[:lower:]' \
  | sed 's|[^a-z0-9_-]|-|g; s|^-\+||; s|-\+$||')
RECEIVER="${sanitised_receiver:-.unrouted}"
```
Lowercase → one-for-one substitution (**runs not collapsed**) → strip leading run of `-` →
strip trailing run of `-`. Internal hyphen runs survive. Falls back to the literal
`.unrouted` when `AM_ME` is unset **or** sanitizes to empty; the dot prefix makes it
disjoint from every legal handle. Path-traversal `AM_ME` is neutralized by the same rule
(pinned by `inject-bridge.bats`).

#### 3.3 Envelope decode

- **Strict mode** (`DUX_AMQ_VERIFY == "1"`, 138-154): requires `amq-receive-verify` on PATH
  (else stderr + `exit 0`); invokes it with **stdin closed (`</dev/null`)** and
  `AMQ_EXPECTED_RECEIVER="$RECEIVER"`; empty stdout (any rejection) → silent `exit 0`. Sets
  `body_is_b64=1` — the strict path never decodes locally.
- **Skip mode DUX2** (161-167): matched by `[[ "$ENVELOPE" == DUX2$'\t'* ]]`. Split into 8
  vars via `IFS=$'\t' read -r magic _sender _recipient _ts _nonce body_b64 _mac extra`. Accepts
  iff `magic=="DUX2" && -n body_b64 && -z extra`. **Malformed → the whole original string
  becomes the literal body** (167), not a drop.
- **Skip mode DUX1** (169-180, legacy): field order is
  `DUX1 \t sender \t recipient \t ts \t nonce \t mac \t body` — MAC **before** body, the
  opposite of DUX2. Strips five fields with `${rest#*$'\t'}`; if a strip is a no-op (fewer
  than 7 fields) it aborts and sets `rest="$ENVELOPE"` — **the entire original text including
  the `DUX1` prefix becomes the body**. Strictly more permissive than DUX2's handling.
- **Anything else** → `body="$ENVELOPE"` verbatim (183).
- A lone `\003` (Ctrl-C transport byte) is dropped in skip mode (197-199).

#### 3.4 Delivery decision tree

1. `under_dux` = `DUX_PANE` non-empty (209-212). If under dux and `DUX_PID` is set but dead,
   `exit 0` (216-220).
2. **Under dux + skip mode + `amq` on PATH → live re-drain substitution** (222-246):
   `amq drain --root "$ROOT" --me "$RECEIVER" --include-body --json --limit "${DUX_AMQ_DRAIN_LIMIT:-20}"`.
   Here `ROOT="${AM_ROOT:-${AMQ_GLOBAL_ROOT:-/data/state/amq}}"` (229) — **inconsistent with
   line 98**, which has no hardcoded fallback.
   - `.count == 0` → another consumer already drained; silent `exit 0`, **no queue write** (232-236).
   - Invalid JSON shape → stderr + `exit 0`, drop (237-240).
   - Otherwise the decoded body is **discarded and replaced** with
     `"[AMQ] Drained messages (JSON):\n<drain_out>\nAct on these AMQ messages now. Reply over AMQ when needed."`
     and `body_is_b64=0` (241).
   - `amq drain` itself failing → log and **fall through with the original body** (243-244).
3. **Outside dux + `$TMUX` set + `tmux` on PATH** (260-271): decode base64 via
   `openssl base64 -d -A` if `body_is_b64` (`|| exit 0`); target = `${DUX_TMUX_TARGET:-}` or
   current pane; `tmux send-keys [-t TARGET] -- "$send_body" Enter`; unconditional `exit 0`.
4. **Otherwise → file queue** (273-306). Under dux this is always taken (the tmux branch is
   gated on `under_dux == 0`).

#### 3.5 Control-byte stripping (248-253)

`LC_ALL=C tr -d '\000-\010\013\014\015\016-\037\177'` — deletes **{0x00–0x08, 0x0B, 0x0C,
0x0D, 0x0E–0x1F, 0x7F}**, i.e. every C0 control except **TAB (0x09) and LF (0x0A)**, plus
DEL. Bytes ≥0x80 are untouched. `LC_ALL=C` is forced deliberately here.

#### 3.6 Queue write (273-306)

- `QUEUE_ROOT="$HOME/.local/share/dux-amq/inject-queue"` (275) — **hardcoded, no env override**.
  (The Rust drainer at `src/amq_inject.rs:136-146` is XDG-aware: `$XDG_DATA_HOME/dux-amq/inject-queue`
  when set, else `~/.local/share/dux-amq/inject-queue`. **The bridge and the drainer disagree
  when `XDG_DATA_HOME` is set** — see Open questions.)
- `QUEUE_DIR="$QUEUE_ROOT/$RECEIVER"`; `mkdir -p` failure → stderr + `exit 0`.
- Timestamp `ts=$(date -u +%s%N)`; BSD `date` has no `%N` and emits a literal trailing `N`,
  detected via `[[ "$ts" == *N* ]]` and downgraded to `date -u +%s` (285-288).
- `tmp=$(mktemp "$QUEUE_DIR/.inflight.XXXXXX")` (294) — created **inside** the destination
  dir so the rename is same-filesystem/atomic, and the `.inflight.` prefix matches the
  reservation prefix the Rust drainer uses. Failure → `exit 0` with **no stderr message**
  (the only fully silent drop).
- Base64 path: `openssl base64 -d -A >"$tmp"`, `rm -f "$tmp"; exit 0` on failure (295-299).
- **Plain-text path `printf '%s' "$send_body" >"$tmp"` (301) and the final
  `mv -f "$tmp" "$target"` (306) are unguarded** — under `set -e` an ENOSPC/EACCES there
  aborts nonzero, contradicting the file's own documented "always exit 0" contract (67-74).
- Final name: `"$QUEUE_DIR/${ts}-${suffix}.msg"` where `suffix="${tmp##*.inflight.}"` (303-305).

Every reachable `exit` in this file is `exit 0`; there is no `exit 1` anywhere.

---

### 4. `scripts/amq-send-signed` (93) + `scripts/amq-receive-verify` (113) — bit-exact

#### 4.1 Key material

`SECRET_PATH="${AMQ_SECRET_PATH:-$HOME/.local/share/dux-amq/amq-secret}"` — identical in
`amq-send-signed:60`, `amq-receive-verify:13`, `amq-secret-init.sh:24`.

- Read as `SECRET=$(cat "$SECRET_PATH")` (send:65) / `SECRET=$(<"$SECRET_PATH")` (verify:18).
  **Both strip all trailing newlines** — an implicit side effect of `$( )`, not an explicit
  trim. A Rust port reading raw bytes must strip trailing `\n` (only `\n`, not `\r\n`) to
  produce a bit-identical key.
- Empty secret → `exit 1` on both sides (send 66-69, verify 19-22).
- The secret file contains base64 **text**; that text is used **literally as the HMAC key
  bytes** — it is never base64-decoded first.

#### 4.2 Envelope construction (`amq-send-signed`)

```
NONCE=$(openssl rand -hex 12)                        # 12 bytes → 24 lowercase hex chars   :73
TS=$(date -u +%Y-%m-%dT%H:%M:%SZ)                    # UTC, second resolution, literal Z    :74
BODY_B64=$(printf '%s' "$BODY" | openssl base64 -A)  # -A = no line wrapping                :75

PAYLOAD="DUX2|$ME|$TO|$TS|$NONCE|$BODY_B64"          # 6 fields joined by literal '|'       :80
MAC=$(printf '%s' "$PAYLOAD" | openssl dgst -sha256 -hmac "$SECRET" -binary | base64 | tr -d '\n')  :81-83

printf -v ENVELOPE 'DUX2\t%s\t%s\t%s\t%s\t%s\t%s' "$ME" "$TO" "$TS" "$NONCE" "$BODY_B64" "$MAC"     :85-86
```

- **MAC algorithm: HMAC-SHA256**, output base64 (standard alphabet, 44 chars with padding).
- **The MAC is computed over the pipe-joined `PAYLOAD`, never over the tab-joined wire line.**
  `|` is chosen because base64/hex alphabets cannot contain it and handles are `[a-z0-9_-]+`.
- **Wire format = 7 TAB-separated fields:**
  `DUX2 \t sender \t recipient \t ts \t nonce \t body_b64 \t mac`.
  (DUX1's legacy order put `mac` before `body` — see §3.3.)
- Delivery: `--print-only` → `printf '%s\n' "$ENVELOPE"`, `exit 0` (89-90); else
  `exec amq send --to "$TO" --me "$ME" --body "$ENVELOPE"` (93).

**Three-way exit-code split for bad argv** (empirically verified by the sub-agent):
`${2:?--to requires a value}` fires bash's own unset-parameter error and terminates with
**exit 1**; the script's explicit unknown-flag (50) and missing-required-flag (57) checks
`exit 2`; `-h/--help` prints usage to **stderr** and `exit 0`. Do not collapse these.
Note `bats-test-parity-matrix.md` records that this CLI surface is **essentially untested**
(every call site passes `--print-only` with all three flags).

#### 4.3 Verification pipeline (`amq-receive-verify`)

Setup failures **exit 1**: secret unreadable `'[amq-verify] secret not readable at %s\n'` (15);
secret empty `'[amq-verify] secret file is empty\n'` (20).
Everything below is a **soft reject with `exit 0`** so AMQ never retries poison messages.

| # | Check | Cite | Exact stderr |
|---|---|---|---|
| 1 | Input: if `[[ -t 0 ]]` use `$1` only; else `IFS= read -r line \|\| true` from stdin, falling back to `$1` when the line is empty | 24-34 | `'[amq-verify] no envelope on stdin or argv (empty invocation)\n'` |
| 2 | Split: `IFS=$'\t' read -r MAGIC SENDER RECEIVER TS NONCE BODY_B64 MAC extra <<<"$line"` — `extra` catches anything past field 7 | 37 | — |
| 3 | `MAGIC != "DUX2"` | 38-42 | `'[amq-verify] dropping unsigned/unknown-magic message (magic=%q sender=%q)\n'` (**`%q`**) |
| 4 | Any field empty, or `extra` non-empty | 43-47 | `'[amq-verify] dropping malformed DUX2 envelope\n'` |
| 5 | `SENDER`/`RECEIVER` `^[a-z0-9_-]+$`; `NONCE` `^[0-9a-f]{24}$`; `BODY_B64`/`MAC` `^[A-Za-z0-9+/]+={0,2}$` | 48-53 | `'[amq-verify] dropping malformed DUX2 field encoding\n'` |
| 6 | `EXPECTED_RECEIVER="${AMQ_EXPECTED_RECEIVER:-${AM_ME:-}}"`; empty or `!= RECEIVER` | 55-60 | `'[amq-verify] wrong recipient (signed=%q expected=%q)\n'` |
| 7 | `SKEW="${AMQ_AUTH_SKEW_SECONDS:-60}"`, `WINDOW="${AMQ_AUTH_WINDOW_SECONDS:-86400}"`. `parse_utc_epoch` tries GNU `date -u -d "$1" +%s` then BSD `date -j -u -f '%Y-%m-%dT%H:%M:%SZ' "$1" +%s`. `DELTA = NOW - MSG_TS`; reject if `DELTA < -SKEW` or `DELTA > WINDOW` | 62-81 | `'...unparseable timestamp %q\n'` / `'...future-dated message (delta=%ds, ts=%s)\n'` / `'...stale message (age=%ds > %ds, ts=%s)\n'` |
| 8 | Recompute `EXPECT` identically (using `openssl base64 -A` rather than `base64\|tr`), compare with `[[ "$MAC" != "$EXPECT" ]]` — **plain string compare, NOT constant-time** | 83-90 | `'[amq-verify] HMAC mismatch from %s\n'` |
| 9 | Replay: `NONCE_ROOT="${XDG_RUNTIME_DIR:-/tmp}/dux-amq"`, `NONCES="$NONCE_ROOT/seen-nonces.d"`, `mkdir -p` + `chmod 0700` (best-effort). The claim is **`mkdir "$NONCES/$NONCE" 2>/dev/null`** — POSIX mkdir atomicity is the entire replay defense | 92-101 | `'[amq-verify] replay rejected (nonce=%s sender=%s)\n'` |
| 10 | Prune: **`(( RANDOM % 100 == 0 ))`** — a 1-in-100 chance *per invocation* to run `find "$NONCES" -mindepth 1 -maxdepth 1 -type d -mmin "+$((WINDOW/60+1))" -exec rm -rf -- {} +` (default 1441 min) | 103-107 | — |
| 11 | Output: `--emit-base64` → `printf '%s\n' "$BODY_B64"`; else `printf '%s' "$BODY_B64" \| openssl base64 -d -A` (raw bytes, no trailing newline) | 109-113 | — |

`amq-auth.bats` (13 tests) pins the accept path, unsigned drop, replay dedup, MAC mismatch,
wrong-recipient, argv-vs-stdin precedence, and byte-for-byte DUX2 round-tripping of tabs,
Unicode, and trailing newlines.

#### 4.4 `scripts/amq-secret-init.sh` (41)

`mkdir -p "$(dirname "$SECRET_PATH")"` runs at line 25, **before** `umask 077` is set at 38 —
so the containing directory inherits the caller's ambient umask; only the **file** is
force-`chmod 0600` (31, 40). If the file exists: `'amq-secret-init: %s already present (kept)\n'`
to stderr, best-effort re-chmod, `exit 0` — **no check that the existing file is non-empty or
well-formed**, so a truncated secret is silently kept. Fresh creation (38-40):
`umask 077; head -c 32 /dev/urandom | base64 | tr -d '\n' > "$SECRET_PATH"; chmod 0600` —
256 bits, base64, **no trailing newline**. No CLI args; always exit 0.

---

### 5. `scripts/encode-claude-project-dir` (93) — bit-exact

#### 5.1 Rules, in order

1. **Absolute-path gate** (63-66): `[[ "$in" != /* ]]` → stderr
   `'encode-claude-project-dir: absolute path required, got %q\n'`, **`exit 2`**. Relative
   paths are never resolved against CWD.
2. **Trailing-slash strip** (70-72): `[[ "$in" != "/" ]] && in="${in%/}"` — strips exactly
   **one** trailing `/` (shortest-suffix match, so `//` → `/`); literal `"/"` is exempt.
3. **Char substitution** (80-91): a native bash loop over `${in:i:1}`; keep every char matching
   `^[A-Za-z0-9-]$` **verbatim (case preserved)**, replace everything else with a single `-`.
   **Runs are NOT collapsed**: `__` → `--`, `..` → `--`.
4. Symlinks are **never** resolved; `.`/`..` are **not** special-cased. Empty input fails the
   absolute-path gate → `exit 2`.

CLI: `-h|--help` → usage to stderr, `exit 0`; `--` → `shift`; exactly one positional required
else usage + `exit 2`.

#### 5.2 Canonical fixtures (`dux-amq/tests/fixtures/claude-paths.txt`, TAB-separated)

```
/tmp/probe/with-dash                                  -tmp-probe-with-dash
/tmp/probe/with_underscore                            -tmp-probe-with-underscore
/tmp/probe/with.dot                                   -tmp-probe-with-dot
/tmp/probe/.dot-prefix                                -tmp-probe--dot-prefix
/tmp/probe/trailing/                                  -tmp-probe-trailing
/tmp/probe/MixedCase                                  -tmp-probe-MixedCase
/tmp/probe/has space dir                              -tmp-probe-has-space-dir
/tmp/probe/multi__under                               -tmp-probe-multi--under
/tmp/probe/with..dotdot                               -tmp-probe-with--dotdot
/tmp/probe/end_                                       -tmp-probe-end-
/data/state/dux/worktrees/audit02-12-path-encoding    -data-state-dux-worktrees-audit02-12-path-encoding
/home/siavash_kiani_jobzy_fi/Projects/dux-amq-setup   -home-siavash-kiani-jobzy-fi-Projects-dux-amq-setup
```

#### 5.3 Already ported — reuse, do not rewrite

`src/purge_encoding.rs` is a complete, documented Rust implementation
(`encode_claude_project_dir(&Path)` and `encode_str(&str)`) whose unit test loads the **same**
fixture file and asserts byte-for-byte equivalence (`src/purge_encoding.rs:91-125`). The Rust
version returns `Err` for empty and for relative input, matching the bash `exit 2` cases.
The port should lift this module into the shared crate and have both `dux` and
`encode-claude-project-dir` call it.

#### 5.4 The one genuine bit-exactness hazard: locale

The bash char loop uses `${#s}` and `${s:i:1}`, which are **codepoint-based under a UTF-8
locale but byte-based under `LC_ALL=C`/`POSIX`**. The sub-agent verified this empirically with
`s="café"`: under `C.UTF-8`/`en_US.UTF-8` the `é` yields **one** `-`; under `C`/`POSIX` it
yields **two**. The script never sets or checks `LC_CTYPE`. **No fixture exercises a
non-ASCII byte**, so there is no pinned answer. `src/purge_encoding.rs` iterates `chars()`,
i.e. it commits to the UTF-8-locale behavior (one `-` per Unicode scalar) — which is almost
certainly right, since the original probe against real Claude Code ran under a UTF-8 locale.
**Record this as a deliberate decision, not an accident.**

---

### 6. `scripts/finalize-claude-migration.sh` (149)

- **Lock** (32-47): `LOCK_FILE="/tmp/dux-amq-finalize.lock"` (hardcoded). `exec 9>"$LOCK_FILE"`
  then **`flock -n 9`** — non-blocking; failure → `"[finalize] another instance is already running (lock: $LOCK_FILE)"`,
  `exit 1`. `trap 'rm -f "$LOCK_FILE"' EXIT` (47) is **cosmetic only** — release happens when
  fd 9 closes at process exit; the `rm` is housekeeping and can itself race a waiting instance.
- **`ensure_no_claude()`** (49-60): `pgrep -x 'claude(-amq)?'`; on match prints
  `"[finalize] a claude/claude-amq process is running — aborting"` plus `pgrep -af`, `exit 1`.
  Called **4 times** — at top (133) and three times inside `migrate_dir` (78, 87, 104) —
  deliberately to shrink, not eliminate, the TOCTOU window.
- **`migrate_dir(src,dst)`** (62-130), three branches:
  1. `[[ -L "$src" ]]` → already migrated, echo target, `return 0` (pure no-op on re-run).
  2. `[[ ! -e "$src" ]]` → `mkdir -p "$dst"`, `ensure_no_claude`, `ln -s "$dst" "$src"`.
  3. Real migration: `mkdir -p "$dst"`; `rsync -aH "${rsync_delete[@]}" "$src/" "$dst/"` (98)
     where `--delete` is added **only** if `FINALIZE_FORCE_DELETE=1` (93-97, with a stderr
     warning) — the default is strictly additive. Then `ensure_no_claude` again (104);
     `bak="${src}.bak.$(date +%Y%m%d-%H%M%S)"` (**local time**, second resolution);
     `mv "$src" "$bak"` (110); `staged="${src}.new"`; `ln -sfn "$dst" "$staged"` (117);
     **`mv -Tn "$staged" "$src"`** (126) — the atomic swap.
- **`mv -T` does not exist on BSD/macOS `mv`** — it would fail with "illegal option" and abort
  under `set -e`. Combined with `flock` (util-linux), this script is Linux-only end to end.
- Post-migration (138-146): if `$STATE_ROOT/.agents` is absent, create
  `$STATE_ROOT/.agents -> $STATE_ROOT/agents` — a workaround for Claude Code's skills CLI
  emitting *relative* symlinks (`../../.agents/...`).
- The `.bak` is **never auto-deleted**; the final echoes tell the operator to `rm -rf` it after
  verifying. Zero CLI arguments. Idempotent by design.

### 7. `scripts/install-gocryptfs.sh` (111) — opt-in, never called by install.sh

Env: `GOCRYPT_CIPHER_DIR` (default `/data/state.crypt`), `GOCRYPT_CLEAR_DIR` (default
`/data/state`), `GOCRYPT_PASS_FILE` (default `/run/credentials/gocrypt.pass`) (29-31).
Preflight (38-68) requires `gocryptfs` and `mountpoint` on PATH and the passfile readable,
and requires its mode to be **exactly one of the literal strings `400`, `600`, `0400`,
`0600`** from `stat -c '%a'` (GNU) with a `stat -f '%Lp'` (BSD) fallback (60) — a string
allowlist, not a numeric/bitmask check. Init (70-89): create + `chmod 0700` +
`gocryptfs -init -passfile ...` if the cipher dir is absent; **refuse (`exit 1`) if the dir
exists without `gocryptfs.conf`**; no-op if both present. Mount (91-108): `mountpoint -q`
short-circuits to `exit 0`; else `gocryptfs -passfile "$PASS_FILE" -allow_other "$CIPHER_DIR" "$CLEAR_DIR"`
(gocryptfs self-daemonizes; the script never backgrounds it). No CLI args; never checks
`EUID`. **Zero test coverage.**

---

### 8. `dux-amq/install.sh` (565)

#### 8.1 Pins (34-51) — all `${VAR:-default}`, CI must use defaults

| Var | Default | Purpose |
|---|---|---|
| `DUX_TAG` | `v0.4.0` | release tag on `patrickdappollonio/dux` |
| `DUX_SHA256` | `a1c449989e9c4dd5…` | dux **tarball** hash |
| `AMQ_TAG` | `v0.61.0` | tag path segment |
| `AMQ_VERSION` | `0.61.0` | filename segment — **two vars that must be kept in sync by hand** |
| `AMQ_SHA256` | `36edf7f1f08ab12e…` | amq **tarball** hash |
| `AMQ_BINARY_SHA256` | `3b10af9f245b04d2…` | amq **extracted-binary** hash — a *separate* pin |
| `SKILLS_PIN` | `1.5.3` | npm package version |
| `SKILLS_REV` | `ad3f9341…` | git commit for `skills add` |
| `CLAUDE_PEERS_REV` | `640183fa…` | git commit for `louislva/claude-peers-mcp` |
| `DUX_AMQ_VERSION` | `0.1.0` | overlay version; drives the block markers |
| `TIOCSTI_PROC_PATH` | `/proc/sys/dev/tty/legacy_tiocsti` | test override point |

`HERE` is computed with pure parameter expansion + `cd`/`pwd` (25-31), **not** `dirname`, so
preflight still works under `env -i PATH=""`.

#### 8.2 Steps

1. **Preflight** (148-217).
   - `${STATE_ROOT%/*}` (empty → `/`) must exist, else `warn` + `exit 1` (154-159).
   - Required tools, **all missing ones aggregated into one message** (172-182):
     `curl jq sha256sum tar install git rsync awk sed realpath openssl`.
   - `mkdir -p "$STATE_ROOT"/{claude,agents,codex,gemini,dux,amq,worktrees,scripts} "$LOCAL_BIN"` (183).
   - **TIOCSTI tri-state** via `tiocsti_status()` (77-91), reading `$TIOCSTI_PROC_PATH` directly
     (no `sysctl`, which lives in `/sbin`):

     | procfs | rc | Action |
     |---|---|---|
     | file absent | 2 | warn (runtime sysctl won't help), **write** `$STATE_ROOT/dux/.tiocsti-state` = `tiocsti_disabled\n` |
     | content `1` | 0 | `rm -f "$TIOCSTI_FLAG"` (clears a stale sentinel), `ok` |
     | content `0` | 1 | warn (suggests `sudo sysctl -w dev.tty.legacy_tiocsti=1`), **write** the sentinel |
     | garbage / empty | 2 | as "file absent" |

     Only the sentinel's **presence** is consulted at runtime; its content is informational.
2. **dux** (219-230). Guard `! command -v dux` — note this is **any** `dux` on PATH, not
   specifically `$LOCAL_BIN/dux`. `curl -fsSL -o "$TMP/dux.tar.gz" https://github.com/patrickdappollonio/dux/releases/download/${DUX_TAG}/dux-linux-amd64.tar.gz`
   — **fixed filename, Linux/amd64 only, no OS/arch detection**. `verify_sha256` vs `DUX_SHA256`
   (exits 1 on mismatch), `tar -xzf`, `install -m 0755 "$TMP/dux" "$LOCAL_BIN/dux"`. `TMP` is
   `mktemp -d` with `trap 'rm -rf "$TMP"' EXIT`, then `trap - EXIT` after cleanup so it does
   not fire twice. No `-k`/insecure flag anywhere.
3. **AMQ** (232-306). Guard `! command -v amq`. URL
   `https://github.com/avivsinai/agent-message-queue/releases/download/${AMQ_TAG}/amq_${AMQ_VERSION}_linux_amd64.tar.gz`.
   The block is the left side of a pipe into `tee -a "$STATE_ROOT/amq/install.log"`, so
   `verify_sha256`'s `exit 1` only kills the subshell — correctness relies on `pipefail`
   propagating and `set -e` aborting immediately after.
   - **Init marker** `$STATE_ROOT/amq/meta/config.json` (254-267): absent → `amq init --root ... --agents claude,codex,gemini --force`;
     present → skip entirely, preserving in-flight queue state. `chmod 700 "$STATE_ROOT/amq"` runs unconditionally (268).
   - **Binary pinning** (270-306), unconditional every run: source = `$LOCAL_BIN/amq` if
     executable else `command -v amq`; `verify_sha256` vs `AMQ_BINARY_SHA256` (rejects a
     tampered `amq` already on PATH even when the download branch was skipped);
     `install -m 0755 ... "$STATE_ROOT/amq-bin/amq"`; then
     `chmod u+w "$STATE_ROOT/amq/binary.sha256" 2>/dev/null || true`,
     `sha256sum "$AMQ_BIN_PINNED" > "$STATE_ROOT/amq/binary.sha256"`, `chmod 0444`.
4. **Skills** (308-319). Guarded by `command -v npx`, **non-fatal**:
   `npx --yes --ignore-scripts "skills@${SKILLS_PIN}" add "avivsinai/agent-message-queue#${SKILLS_REV}" -g -y`.
   `--ignore-scripts` is a deliberate supply-chain rail.
5. **Claude Peers MCP** (321-372). Guarded by `command -v bun` **and** `command -v claude`;
   every sub-step is `|| warn`. `CLAUDE_PEERS_DIR` defaults from `$STATE_ROOT`, except when
   `$STATE_ROOT` matches `/tmp*`, `/private/tmp*`, or `/var/folders/*`, in which case it is
   redirected to `$HOME/.local/state/claude-peers-mcp` (326-336). Clone-or-fetch, then a
   **commit-pin verification** (354-360) whose three lines are pinned *verbatim* by
   `supply-chain-rails.bats`: `git checkout --detach "$CLAUDE_PEERS_REV"`,
   `peers_head=$(git rev-parse HEAD)`, `[[ "$peers_head" != "$CLAUDE_PEERS_REV" ]]` → skip
   registration. On success: `bun install` (if `package.json`), then
   `claude mcp remove --scope user claude-peers` (errors ignored) followed by
   `claude mcp add --scope user --transport stdio claude-peers --env OPENAI_API_KEY= -- "$BUN_BIN" "$CLAUDE_PEERS_DIR/server.ts"`
   — remove-then-add is itself the idempotency pattern.
6. **Wrappers/scripts** (374-414), a flat sequence of unconditional `install -m 0755`:

   | Source | Destination |
   |---|---|
   | `wrappers/claude-amq` / `codex-amq` / `gemini-amq` | `$LOCAL_BIN/<same>` |
   | `scripts/encode-claude-project-dir` | `$LOCAL_BIN/encode-claude-project-dir` |
   | `scripts/dux-amq-doctor` | `$LOCAL_BIN/dux-amq-doctor` |
   | `scripts/amq-secret-init.sh` | `$LOCAL_BIN/amq-secret-init.sh` |
   | `scripts/amq-send-signed` | `$LOCAL_BIN/amq-send-signed` |
   | `scripts/amq-receive-verify` | `$LOCAL_BIN/amq-receive-verify` |
   | `scripts/dux-amq-inject-bridge` | `$LOCAL_BIN/dux-amq-inject-bridge` |
   | `scripts/finalize-claude-migration.sh` | **`$STATE_ROOT/scripts/finalize-claude-migration.sh`** (not PATH) |

   `install-gocryptfs.sh` is **never installed** — it is run from the repo checkout on demand.
   Finally `"$HERE/scripts/amq-secret-init.sh"` is **executed** (414) — the repo-checkout copy,
   not the just-installed one.
7. **dux config** (416-435). Regenerate guard:
   `[[ ! -e "$STATE_ROOT/dux/config.toml" && ! -L ... ]]` → `DUX_HOME="$STATE_ROOT/dux" dux config regenerate --yes`.
   Checking `-L` as well is what makes a symlinked config survive (exactly the
   `install-idempotency.bats` P0-01 scenario). Then, **every run unconditionally**, one
   `sed -i --follow-symlinks` with 12 `-e` substitutions, each address-scoped like
   `/^\[providers\.claude\]$/,/^\[/ s|^command = "claude"$|command = "claude-amq"|`.
   Idempotency is **emergent**: each substitution matches only the vanilla default, so once
   applied the line no longer matches. `--follow-symlinks` is **GNU-sed-only**.
   Substitutions cover `prompt_for_name`, all three providers' `command`, Claude's
   `resume_args`/`forward_scroll`/`forward_mouse`, Codex's
   `args`/`resume_args`/`resume_by_id_args`/`forward_scroll`/`forward_mouse`, Gemini's
   `forward_scroll`.
8. **`~/.bashrc`** (437-449): `touch`; `strip_block "$HOME/.bashrc" sh`;
   `bashrc_block=$(<"$HERE/config/bashrc-additions.sh")`;
   `printf -v state_root_quoted '%q' "$STATE_ROOT"`;
   `${bashrc_block//REPLACE_AT_INSTALL/$DUX_AMQ_VERSION}` and
   `${bashrc_block//REPLACE_STATE_ROOT/$state_root_quoted}`; `printf '%s\n' >> ~/.bashrc`.
9. **`~/.claude/CLAUDE.md`** (451-467): `mkdir -p`, `touch`; if the file is **non-empty**,
   `install -m 0644 "$HOME/.claude/CLAUDE.md" "$STATE_ROOT/dux/claude-md.$(date +%s).bak"`
   (**second resolution, no uniqueness fallback — two installs in the same second silently
   clobber the earlier backup**); `strip_block ... md`; then append:
   ```
   printf '\n<!-- >>> dux-amq v%s >>> -->\n\n' "$DUX_AMQ_VERSION"
   cat "$HERE/config/claude-md-additions.md"      # verbatim, NO substitution
   printf '\n<!-- <<< dux-amq v%s <<< -->\n' "$DUX_AMQ_VERSION"
   ```
10. **VS Code Remote-SSH** (469-539). Target `$HOME/.vscode-server/data/Machine/settings.json`.
    Missing `~/.vscode-server` → `say` (not `warn`) and return 0. The `command -v jq` guard at
    the top of the function is **dead code** — `jq` is already a hard preflight requirement.
    Reads `vscode/settings-additions.json`, strips `//...` with `sed 's|//.*$||'` (naive,
    line-based, safe only because the template has no `//` inside strings), then
    `jq -c '.["terminal.integrated.commandsToSkipShell"]'`. Missing file → write a fresh
    single-key JSON; existing → `jq '.[k] = ((.[k] // []) + $new) | unique'` into `"$f.tmp"`
    then `mv`. **`unique` also sorts**, so the port must reproduce "concat, sort, dedup",
    not "append if absent".

#### 8.3 Block markers — bit-exact (`strip_block`, 105-146)

`strip_block(file, kind)`: no-op if the file is absent; writes through
`mktemp "${file}.dux-amq.XXXXXX"` then `mv` (atomic replace); unknown `kind` → `warn`,
`rm -f "$tmp"`, `return 1`.

```awk
# kind = sh
/^# >>> dux-amq v[^ ]+ >>>$/     {s=1; next}
/^# <<< dux-amq v[^ ]+ <<<$/     {s=0; next}
/^# === dux \+ AMQ ===$/         {s=1; next}     # legacy, pre-Phase-12
/^# === end dux \+ AMQ ===$/     {s=0; next}
!s

# kind = md
/^<!-- >>> dux-amq v[^ ]+ >>> -->$/          {s=1; next}
/^<!-- <<< dux-amq v[^ ]+ <<< -->$/          {s=0; next}
/^<!-- end dux-amq legacy -->$/              {s=0; next}   # legacy explicit sentinel
/^## Multi-agent environment \(AMQ \+ dux\)$/ {s=1; next}   # legacy heading
s && /^## /                                   {s=0}        # NOTE: no `next`
!s
```

Three details a port must get right:

- The version inside a marker (`v[^ ]+`) is **never compared** to `$DUX_AMQ_VERSION` — *any*
  version strips. That is what makes version bumps actually propagate.
- In the md legacy branch, `s && /^## / {s=0}` has **no `next`**, so the sibling heading line
  itself is **printed** (the audit02 P0-G fix: a user's own `## Notes` after the AMQ stanza
  survives). Marker lines, by contrast, are consumed via `next`.
- Emitted markers must be byte-identical: `# >>> dux-amq v0.1.0 >>>` /
  `# <<< dux-amq v0.1.0 <<<` and `<!-- >>> dux-amq v0.1.0 >>> -->` /
  `<!-- <<< dux-amq v0.1.0 <<< -->`.

**Real idempotency leak, currently untested.** `bashrc-additions.sh` lines 1-7 are preamble
comments (`# shellcheck shell=bash`, four explanatory lines, a blank line) that sit **before**
the begin marker at line 8; the end marker is line 69 (the last line). Because install.sh
appends the file's *entire* contents, **every re-install appends those 7 orphan lines to
`~/.bashrc`, and `strip_block` never removes them** (they are outside the markers).
`install-idempotency.bats` does not catch this — it only asserts
`! grep -Fq -- "/data/state" "$HOME/.bashrc"` and byte-compares `config.toml`. The Rust port
should emit only the marker-delimited region.

#### 8.4 No interactive prompts, no uninstall

`grep -n "read -p"` across both installers returns **zero matches**; every optional path
degrades to a `warn`. The closing lines (549-565) are informational echoes, not `read` calls.

`grep -rniE "uninstall|--remove|--purge"` across both installers, both config dirs, and both
READMEs returns **zero matches**. There is **no uninstall path at all**. `strip_block` is only
ever invoked as a prelude to re-appending, never standalone, and touches only `~/.bashrc` and
`~/.claude/CLAUDE.md` — not `$LOCAL_BIN` binaries, `$STATE_ROOT`, the VS Code merge, or the
Claude Peers MCP registration.

### 9. Root `install.sh` (165) — a different trust model

Confirmed **fully independent**: no reference to `dux-amq/` anywhere; it installs only the
`dux` binary.

| Aspect | Root `install.sh` | `dux-amq/install.sh` |
|---|---|---|
| Source repo | `${DUX_REPO:-SiavZ/dux-amq-setup}` (`install.sh:4`) — **this** repo's releases | `patrickdappollonio/dux` |
| Version | `${DUX_VERSION:-}`, normalized to a `v` prefix; else scrape `"tag_name"` from `https://api.github.com/repos/${REPO}/releases/latest` with `grep -o` + `sed` (**no jq**). No fallback if the API call fails — hard error telling the user to set `DUX_VERSION` | hardcoded `DUX_TAG` pin |
| OS/arch | `detect_os`: `uname -s` → `linux`/`darwin` (error otherwise); `detect_arch`: `x86_64\|amd64→amd64`, `aarch64\|arm64→arm64` (error otherwise); archive = `${BINARY}-${os}-${arch}.tar.gz` | hardcoded `dux-linux-amd64.tar.gz` |
| Checksum | Downloads `SHA256SUMS` **from the same release** and looks the archive up with `awk` matching `name` or GNU `*name` (68-77). **No hardcoded hash anywhere** — trust is "whatever the release currently publishes", materially weaker than an in-script pin | in-script sha256 pins |
| Transport | `http_get` dispatches curl **or wget** (36-56) | curl only |
| Destination | `$DUX_INSTALL_DIR`, else `$HOME/.local/bin` **if it exists and is already on `$PATH`**, else `/usr/local/bin` (101-118) | always `$HOME/.local/bin` |
| sudo | the **only** sudo in either installer: `[ -w "$install_dir" ]` ? `install -m 755` : `sudo install -m 755` (146-152). Non-interactive-hostile — no `sudo -n` probe, so a `curl \| bash` with no TTY and no cached credentials hangs or fails | none |
| Cleanup | `printf -v cleanup_cmd 'rm -rf -- %q' "$tmpdir"`, `trap "$cleanup_cmd" EXIT` (133-137) | per-step `trap`/`trap -` |

`root-installer.bats` (2 tests) pins the exact URLs
(`github.com/SiavZ/dux-amq-setup/releases/download/v1.2.3/dux-linux-amd64.tar.gz` and
`.../SHA256SUMS`) and that a checksum mismatch aborts **before** `tar` runs and before
`$INSTALL_DIR/dux` exists.

### 10. Config fragments

| File | Content | How install.sh consumes it |
|---|---|---|
| `config/bashrc-additions.sh` (69) | PATH-prepend guard; conditional `STATE_ROOT` export; derived `DUX_HOME`/`AMQ_GLOBAL_ROOT`/`AMQ_BIN` exports; the **fail-closed** `_amq_shell_setup_guarded` (26-65): skip silently if `! -x "$AMQ_BIN"`; **fail** if the binary exists but `$AMQ_GLOBAL_ROOT/binary.sha256` is missing (36-40, the audit02 N-3 fail-open fix); fail if `"$AMQ_BIN" -nt "$rec"` (50-54); full `sha256sum` compare (55-61); on match `eval "$("$AMQ_BIN" shell-setup)"`. Commented-out `# export CLAUDE_YOLO=1` (67-68) | Read verbatim into a variable, two `${var//search/replace}` substitutions (`REPLACE_AT_INSTALL` → version, `REPLACE_STATE_ROOT` → `%q`-quoted `$STATE_ROOT`), then appended. **Not** sourced by install.sh |
| `config/claude-md-additions.md` (11) | A `## Multi-agent environment (AMQ + dux)` section: queue root, `AM_ME` derivation, `dux peer list`/`sync-amq`, the router-not-raw-amq policy, `$STATE_ROOT` persistence, the `[task-done]` sentinel | `cat`'d **verbatim, zero substitution**, wrapped only by `printf`-built markers |
| `config/dux-config-changes.toml` (28) | A human-readable diff reference, **not consumed programmatically at all** (its own header says "apply these after first launch"). The real mechanism is the `sed` block at 421-435 | Nothing — must be kept in sync with the sed block by hand |
| `vscode/settings-additions.json` | JSONC with one key, `terminal.integrated.commandsToSkipShell`, listing four `-`-prefixed entries (`-workbench.action.gotoLine`, `-workbench.action.terminal.goToRecentDirectory`, `-workbench.action.files.openFile`, `-workbench.action.quickOpen`) | `sed 's|//.*$||'` → `jq -c` extract → concat+`unique` merge into the VM's settings.json |

**Where the AMQ hash actually lives:** `AMQ_BINARY_SHA256` is a constant in
`dux-amq/install.sh:47`. What `dux-amq-doctor` compares against is the **recorded** value at
`$STATE_ROOT/amq/binary.sha256`, written by install.sh at line 304 as `sha256sum` of the
pinned binary. So doctor answers "does the pinned binary still match what install recorded",
not "does it match the in-script constant". `bashrc-additions.sh` also compares against the
same recorded file.

---

## Proposed dux-amq-rust architecture

### A. Workspace shape

The repo root `Cargo.toml` is a **single package with no `[workspace]` section**, and no
other `Cargo.toml` exists in the tree. Adding `dux-amq-rust/Cargo.toml` inside the `dux`
package directory without declaring a workspace makes it an orphan that `cargo package`
would try to include.

**Recommendation:** add to the root `Cargo.toml`:

```toml
[workspace]
members = ["dux-amq-rust"]
```

A package can also be a workspace root, so `dux` stays where it is. Benefits: one
`Cargo.lock` (so the existing pinned graph is reused and `cargo deny`/`cargo audit` cover
both crates), one `target/`, and `dux` can depend on `dux-amq-rust` as a path dependency to
share `purge_encoding`, the handle sanitizer, and the AMQ registry protocol instead of
duplicating them.

```
Cargo.toml                  # dux package + [workspace] members = ["dux-amq-rust"]
dux-amq-rust/
  Cargo.toml                # package "dux-amq"; one [[bin]] name = "dux-amq"
  src/…                     # see module tree
  tests/…                   # integration tests (parity with the bats suite)
```

### B. Dispatch strategy: one binary, multicall symlinks

Today install.sh does `install -m 0755` of **nine** separate scripts onto `$PATH` plus one
into `$STATE_ROOT/scripts`. Options:

| Option | Verdict |
|---|---|
| Nine `[[bin]]` targets | Rejected. With `opt-level="z"` + LTO + `strip` each binary is still ~2-4 MB; nine copies is ~25-35 MB in the release tarball for what is one code base. |
| Nine copies of one binary | Rejected for the same size reason. |
| **One binary + symlinks, argv[0] multicall** | **Recommended.** `clap` supports this first-class via `Command::multicall(true)` (see `clap::_cookbook::multicall_busybox`), which strips argv[0] to its basename and parses it as the first subcommand. |

Mechanics that make this safe here:
- `std::env::args_os().next()` → `Path::file_name()` gives the symlink name, because
  `exec`/`Command::new` set argv[0] to the path actually used. `src/cli.rs::resolve_doctor_script`
  calls `Command::new(path_from_which)`, so the basename is `dux-amq-doctor` — correct.
- **Two applet names end in `.sh`** (`amq-secret-init.sh`, `finalize-claude-migration.sh`).
  The symlink names must stay byte-identical for compat, so the basename→applet map must
  carry the `.sh` suffix as part of the key. Do not "normalize" it away.
- Every applet must **also** be reachable as an explicit subcommand of the `dux-amq` binary
  (`dux-amq doctor`, `dux-amq bridge`, …) so the tool is usable without symlinks and so
  `dux-amq install` can bootstrap. Per the clap cookbook, that means declaring the applets
  both at the top level and under a `main` applet.
- The installer changes from `install -m 0755 <script> <dest>` to `install -m 0755 dux-amq $LOCAL_BIN/dux-amq`
  plus `ln -sfn dux-amq $LOCAL_BIN/<applet>` for each name. `finalize-claude-migration.sh`
  keeps its `$STATE_ROOT/scripts/` destination as a symlink to `$LOCAL_BIN/dux-amq`.

Applet inventory (10 names):
`claude-amq`, `codex-amq`, `gemini-amq`, `dux-amq-doctor`, `dux-amq-inject-bridge`,
`amq-send-signed`, `amq-receive-verify`, `amq-secret-init.sh`, `encode-claude-project-dir`,
`finalize-claude-migration.sh`.

### C. What stays bash

- **Root `install.sh`** — a `curl | bash` release fetcher. Chicken-and-egg: it exists to put
  a binary on disk. Keep as bash.
- **`dux-amq/install.sh`** — reduce to a thin bootstrap that (a) runs preflight, (b) fetches
  or builds `dux-amq`, then (c) delegates every subsequent step to
  `dux-amq install --state-root … [--skip-peers] [--dry-run]`. That moves the block-marker
  logic, the TIOCSTI tri-state, the config `sed` patches, and the VS Code jq merge into
  testable Rust while keeping the bootstrap honest.
- **`install-gocryptfs.sh`** — opt-in, never invoked by install.sh, zero test coverage, and
  purely a `gocryptfs`/`mountpoint` driver. Lowest port value; defer or leave as bash.

### D. Module tree (every file budgeted under 500 lines)

```
dux-amq-rust/src/
  main.rs                    ~120  argv[0] basename → applet; clap multicall; anyhow → exit code
  lib.rs                      ~70  pub mod re-exports; the shared surface `dux` links against

  common/
    mod.rs                    ~30
    env.rs                   ~190  bash `${VAR:-x}` semantics (empty == absent); StateRoot /
                                   DuxHome / AmqRoot / LocalBin / SecretPath resolution
    handle.rs                ~130  sanitize_handle(), validate(<=64, ^[a-z0-9_-]+$),
                                   `.unrouted` fallback — shared by wrappers + bridge
    shquote.rs                ~90  printf `%q` parity (error messages are pinned byte-for-byte)
    ctrlbytes.rs              ~70  the exact C0-except-TAB/LF + DEL delete set
    proc.rs                  ~230  run-with-timeout; (timeout)/(not found)/(error) sentinels;
                                   exec-replacing-self (rustix::runtime or execvp)
    locks.rs                 ~140  rustix::fs::flock wrappers: blocking-exclusive (registry),
                                   non-blocking (finalize); retry-on-EINTR
    atomic.rs                ~110  mktemp-in-dir + fsync + rename; the `.inflight.` prefix rule

  wrapper/
    mod.rs                    ~90  Provider enum {Claude, Codex, Gemini}; run(provider, argv)
    version_gate.rs          ~140  version_at_least + require_provider_version
                                   (first-semver-substring-wins, `|| true` tolerance)
    identity.rs              ~200  managed-vs-standalone chain, is_dux_worktree, normalization
    registry.rs              ~340  claim_and_register_locked equivalent: agent dir, polymorphic
                                   `.dux-amq-source` marker, meta/config.json merge
    transport.rs             ~190  INJECT_MODE, BRIDGE resolution, WAKE_ARGS, recover-owner gate
    seed.rs                  ~210  Claude-only session-history seeding (rsync/cp fallback)
    launch.rs                ~240  per-provider argv assembly + exec into `amq coop exec`

  bridge/
    mod.rs                    ~90
    startup.rs               ~160  5s backlog poll: kill(0) liveness + .wake.lock/.wake.prepared
                                   generation equality
    envelope.rs              ~210  DUX2 / DUX1 / raw decode, incl. the permissive fallbacks
    drain.rs                 ~180  under-dux `amq drain --json` substitution + count==0 handling
    deliver.rs               ~200  tmux vs queue decision tree
    queue.rs                 ~170  `<ts>-<suffix>.msg` naming, `%s%N` BSD fallback, atomic write

  auth/
    mod.rs                    ~40
    secret.rs                ~120  AMQ_SECRET_PATH; init (32B urandom → base64, 0600, no NL);
                                   read + trailing-\n trim
    sign.rs                  ~150  PAYLOAD "DUX2|me|to|ts|nonce|body_b64"; HMAC-SHA256;
                                   base64 MAC; TAB envelope
    verify.rs                ~300  the 11-stage rejection pipeline (split further if it grows)
    nonce_store.rs           ~130  mkdir-atomic claim + age-based prune

  encode/
    mod.rs                   ~100  Claude project-dir encoder — lift `src/purge_encoding.rs`

  doctor/
    mod.rs                   ~140  section orchestration; always exit 0
    text.rs                  ~190  kv/color rendering, exact `%-30s`/`%-5s` widths
    json.rs                  ~150  dotted-path setter (the `j_set` equivalent)
    anonymize.rs             ~200  $HOME → /HOME; worktree → /WT/branch-N; branch/agent caches
    sec_versions.rs          ~140  incl. the awk-equivalent DUX_AMQ_VERSION scrape
    sec_binary.rs            ~130
    sec_disk.rs              ~150
    sec_amq.rs               ~240
    sec_symlinks.rs          ~110
    sec_kernel.rs             ~90
    sec_sessions_db.rs       ~160  rusqlite instead of the sqlite3 CLI
    sec_runtime.rs           ~120  sysinfo instead of ps
    sec_errors.rs            ~170

  migrate/
    finalize.rs              ~270  flock -n, 4× ensure_no_claude, rsync, backup, atomic swap

  install/
    mod.rs                   ~150  step orchestration
    blocks.rs                ~220  strip_block sh/md + byte-identical marker emission
    tiocsti.rs                ~90  the tri-state truth table + sentinel
    config_patch.rs          ~200  the 12 scoped substitutions (through symlinks)
    vscode.rs                ~140  JSONC strip + concat/sort/dedup merge
    pins.rs                  ~120  the pin table + sha256 verification
```

Total ≈ 7,600 lines across 42 files; largest file ≈ 340. Comfortably under the 500-line rule
with room for doc comments.

### E. Dependencies (and why)

| Crate | Version | Justification |
|---|---|---|
| `clap` | 4.6 (MSRV 1.85) | Multicall/argv[0] dispatch is first-class (`Command::multicall`). **Note the parent `dux` crate deliberately hand-rolls its arg parsing** — adding clap here is a departure; the alternative is a ~150-line hand-rolled dispatcher matching `dux`'s existing style. Flag as a decision. |
| `anyhow` | 1.0 (already in `dux`) | Same error idiom as the parent crate. |
| `serde` / `serde_json` | already in `dux` | Doctor JSON, `.dux-amq-source` marker, `meta/config.json` merge, `amq drain --json`, `.wake.lock`. Replaces every `jq` shell-out. |
| `rustix` | 1.1 (already in `dux`, features `fs process`) | `flock` (`FlockOperation::LockExclusive`/`NonBlocking`), `setsid`, `kill_process`, `getsid`. **Zero new dependency** — `src/peer.rs` already uses `rustix::fs::flock` for the very same `meta/config.lock`, so using it here guarantees identical lock semantics. |
| `hmac` + `sha2` | 0.12 + 0.10 (or 0.13 + 0.11) | HMAC-SHA256 for DUX2 and sha256 for binary integrity. **Pure Rust — no openssl**, which matters because release builds target `x86_64/aarch64-unknown-linux-musl` (`.github/workflows/release.yml`) where linking openssl is a liability. Choosing 0.12/0.10 avoids a duplicate `sha2` (0.10.9 is already in `Cargo.lock` via `termwiz`); 0.13/0.11 is the modern API at the cost of a second `sha2`. `deny.toml` sets `multiple-versions = "warn"`, so either passes. |
| `subtle` | 2.6 | Constant-time MAC comparison. The bash uses `[[ "$MAC" != "$EXPECT" ]]` (not timing-safe); the port should fix this — it changes no observable behavior. |
| `base64` | 0.23 | DUX2 body and the secret. |
| `rand` | 0.8 (already in `dux`) or `getrandom` | 12-byte nonce and the 32-byte secret. |
| `tempfile` | 3.20 (already in `dux`) | `mktemp`-in-directory semantics for every atomic write. |
| `chrono` | 0.4 (already in `dux`) | `%Y-%m-%dT%H:%M:%SZ` format/parse, skew/window arithmetic, backup timestamps. |
| `rusqlite` | 0.39 bundled (already in `dux`) | Doctor's sessions-DB section without an external `sqlite3` CLI — this **removes** the `sqlite3-cli-missing` degraded state. |
| `sysinfo` | 0.35 (already in `dux`) | Doctor runtime PID/RSS/uptime without shelling to `ps` (which is currently un-timeout-guarded at `dux-amq-doctor:700-705`). |
| `which` | 8.0 | PATH resolution for `flock`, `jq`-replacement, `amq`, `tmux`, `rsync`, provider binaries. |
| `libc` | 0.2 (already transitively) | Only if `rustix` lacks something needed for `execvp`. |
| dev: `assert_cmd` 2.2, `predicates` 3.1, `tempfile`, `serial_test` (already in `dux`) | | Black-box CLI tests replacing the bats `run` idiom. `insta`/`insta-cmd` is optional for doctor text-output snapshots. |

All of these are MIT/Apache-2.0 and MSRV ≤ 1.85, so they clear `deny.toml`'s license
allowlist and the crates.io-only source policy (no git deps). Everything except `clap`,
`hmac`, `sha2`, `subtle`, `base64`, and `which` is **already in the lock file**.

### F. Toolchain prerequisite

`rust-toolchain.toml` pins **1.88.0** while stable is **1.98.0** (`00-baseline.md`). Every
clippy lint added in 1.89–1.98 is currently invisible. Bump **before** writing the new crate,
so 42 new modules are linted by the compiler CI will eventually use — otherwise the bump
later produces a second, larger cleanup. `cargo clippy --all-targets --all-features -- -D warnings`
is a CI gate.

### G. Test strategy

`bats-test-parity-matrix.md` is the acceptance list. Its hazard §4 is the binding constraint:

- **~60 argv assertions** work only because `tests/fakes/amq` shadows the real binary on PATH
  and the wrapper does a literal `exec amq coop exec …`. If the Rust wrapper still `exec`s an
  external `amq`, **the existing fakes and their `AMQ_FAKE_ARGV_FILE` mechanism keep working
  unchanged** — which is a strong argument for keeping the subprocess boundary rather than
  reimplementing `amq` in-process. Additionally expose a pure
  `wrapper::launch::build_argv(...) -> Vec<OsString>` so the same expectations can be asserted
  as fast unit tests.
- **12 sourced-function tests across 3 files have no Rust analogue** (`is_dux_worktree`,
  `tiocsti_status`, `strip_block`). Re-express each as `pub fn` + `#[test]` carrying the same
  input/output pairs.
- **Tests asserting file content, not behavior** should be deleted or re-anchored: the
  "byte-identical `is_dux_worktree` across three wrappers" test is meaningless once there is
  one shared impl (replace with one unit test); the four `supply-chain-rails.bats` greps
  target literal bash/YAML text (re-anchor to the new build tooling — but **do** keep the
  double-tar reproducibility `cmp`).
- The `encode-claude-project-dir` fixture file must stay at
  `dux-amq/tests/fixtures/claude-paths.txt` — both `src/purge_encoding.rs`'s unit test and
  `encoder-fixtures.bats` load it by that path.
- Keep `src/peer.rs::rust_and_wrapper_claims_serialize_on_config_lock` green by pointing it at
  the new `claude-amq` symlink.

---

## Bash-semantics hazards

A naive port silently changes behavior at each of these points.

1. **`${VAR:-x}` fires on set-but-empty.** Every default in every wrapper uses colon-dash.
   `std::env::var("X")` returning `Ok("")` must be treated as absent. A `fn env_or(name, default)`
   in `common/env.rs` should be the only way env is read.
2. **`set -e` is defused in specific, load-bearing places.** `$(cmd || true)` in
   `require_provider_version` (`claude-amq:39`) means a provider whose `--version` exits
   nonzero **still gets its output parsed**. `seed_session_history || true`. Conversely,
   `amq wake recover-owner --strict` (`claude-amq:382`) is **unguarded** and its nonzero exit
   **kills the wrapper with that exact code** — that is a feature, pinned by a test.
3. **`set -e` does not cross a pipeline's left side.** In `dux-amq/install.sh:241-251` the AMQ
   download block is `{ … } | tee -a`, so `verify_sha256`'s `exit 1` only kills the subshell;
   correctness depends on `pipefail` + the outer `set -e`. A Rust early-return is fine, but a
   test asserting the abort point must account for the ordering.
4. **`exec` replaces the process; the backgrounded bridge is not reparented cleanly.**
   `DUX_AMQ_STARTUP_OWNER_PID="$$" "$BRIDGE" &` then `exec` means the child's parent PID stays
   numerically valid but its spawning shell is gone. No `wait`, no `setsid`, no `disown`. If
   the new image exits inside the bridge's 5 s window, the bridge is orphaned and may deliver a
   stale "startup backlog recovery" into a dead session's queue. **Decide explicitly**: keep
   fire-and-forget (bug-compatible), or `setsid` + double-fork, or run it as a thread before
   `execvp`. Untested today.
5. **No traps anywhere in the wrappers.** A Rust port that adds RAII cleanup on
   Ctrl-C is a *behavior change* (bash leaves a half-created `agent_dir`). Probably a
   desirable one — but note it.
6. **`export -f` has no Rust equivalent.** The registration critical section runs in a *fresh
   bash* that inherits the function via `BASH_FUNC_*`. Inline it under a native `flock`.
7. **`flock(1)` pathname form vs `flock(2)` fd form.** `flock -x <file> <cmd>` locks a fd the
   tool opens itself; the lock lives for the child's lifetime. In Rust the `File` must be kept
   alive for the whole critical section (`src/peer.rs`'s `AmqRegistryLock` + `Drop` is the
   pattern to copy). Related: in `finalize-claude-migration.sh`, `rm -f "$LOCK_FILE"` does
   **not** release the lock (fd close does) — deleting the file can race a waiting instance.
8. **`printf %q` shell-escaping is pinned in error messages.** Used for the unparseable-version
   output, the unrecognized `DUX_AMQ_INJECT_MODE` warning, and four `amq-receive-verify`
   rejections. Rust `{:?}` escapes differently. `common/shquote.rs` must reproduce bash's
   `%q` rules, or the tests must be re-baselined deliberately.
9. **Locale-dependent character iteration** in `encode-claude-project-dir` — one `-` per
   codepoint under UTF-8, per **byte** under `LC_ALL=C`. Untested. `chars()` is the right
   choice; make it a recorded decision.
10. **`unique` in jq sorts.** The VS Code merge is concat → **sort** → dedup, not
    append-if-absent. A "smarter" merge produces a different file.
11. **`s && /^## ` in `strip_block`'s md branch has no `next`.** The sibling heading is
    *printed*, unlike the marker lines which are consumed. Getting this wrong re-introduces
    the audit02 P0-G data-loss bug.
12. **GNU-vs-BSD tool divergence is handled inconsistently.** Handled: `date +%s%N`
    (`inject-bridge:285-288`), `date -d` vs `date -j -f` (`amq-receive-verify:65-68`),
    `stat -c` vs `stat -f` (`install-gocryptfs.sh:60`). **Not** handled: `mv -Tn`
    (`finalize-claude-migration.sh:126`, GNU-only, hard-fails on macOS) and
    `sed -i --follow-symlinks` (`install.sh:421`, GNU-only). CLAUDE.md says target Unix
    without `cfg!(windows)` branches, but the bats suite does run on macOS — a Rust port
    naturally removes these divergences, which will **change** behavior on macOS from
    "hard-fail" to "works".
13. **Naive `-p`/`--print` scanning** in the claude/gemini oneshot check matches the flag
    anywhere in argv, including as another option's value. Untested; preserve or fix
    deliberately.
14. **Two exit-code families in `amq-send-signed`.** `${2:?msg}` → bash's own error and
    **exit 1**; explicit checks → **exit 2**. Do not unify.
15. **`dux-amq-inject-bridge` documents "always exit 0" but does not enforce it.** The
    plain-text body write (301) and the final `mv -f` (306) are unguarded under `set -e`. The
    Rust port should honor the documented contract (always 0) rather than the accidental one.
16. **`config.json.tmp.XXXXXX` is not cleaned on a failed final `mv`** — asymmetric with the
    `.owner.tmp` path, which is. Fix, and note it as a divergence.
17. **The `timeout` shell function degrades to unbounded** when neither `timeout` nor
    `gtimeout` exists (`dux-amq-doctor:64-73`). A Rust implementation always enforces the
    deadline — a behavior change, and a good one.
18. **`$HOME` substring replacement in the anonymizer is not path-boundary aware** — any
    occurrence of the `$HOME` string anywhere is rewritten to `/HOME`.
19. **`mkdir -p "$ROOT/meta" "$ROOT/agents"` runs before the `flock`/`jq` checks** — an
    aborted wrapper run still leaves those directories behind.
20. **`date +%s` backup collisions** in the CLAUDE.md snapshot: two installs in the same
    second silently clobber the earlier `.bak`.

---

## Migration/compat

The Rust binaries must be **drop-in** on hosts already running the bash version. Concretely:

**Must be read and preserved unchanged (on-disk state):**

| Path | Format | Who else touches it |
|---|---|---|
| `$STATE_ROOT/amq/meta/config.json` | JSON, jq-merged | `amq` binary, Rust `dux` (`src/peer.rs` reconcile) |
| `$STATE_ROOT/amq/meta/config.lock` | empty lock file, `flock(2)` exclusive | **`src/peer.rs:722-731` takes the same lock** — mandatory to keep |
| `$STATE_ROOT/amq/agents/<handle>/.dux-amq-source` | **symlink** (standalone) or **JSON `{store_id,session_id,wake_pid?}`** (managed) | `src/peer.rs:930-1010`; a symlink is classified `Legacy` and never reclaimed |
| `$STATE_ROOT/amq/agents/<handle>/.wake.lock` / `.wake.prepared` | JSON with `.owner.pid`, `.generation` | written by `amq`; read by the wrapper and the bridge |
| `$STATE_ROOT/amq/binary.sha256` | `sha256sum` output, **mode 0444** | written by install.sh; read by doctor **and by `~/.bashrc`'s fail-closed guard** |
| `$STATE_ROOT/amq-bin/amq` | the pinned binary | hash-guarded on every interactive shell |
| `$STATE_ROOT/dux/.tiocsti-state` | sentinel; content informational, **presence** is the signal | wrappers |
| `$STATE_ROOT/dux/config.toml` | TOML, possibly a **symlink** | `dux` itself; the installer patches through the symlink |
| `$STATE_ROOT/dux/sessions.sqlite3` | SQLite WAL | `dux`; doctor opens it **read-only** |
| `$STATE_ROOT/dux/dux.lock` | first whitespace field of line 1 = PID | doctor runtime section |
| `$STATE_ROOT/scripts/finalize-claude-migration.sh` | executable | operator runs it manually |
| `$HOME/.local/share/dux-amq/amq-secret` | base64 text, **no trailing newline**, mode 0600 | sign + verify; **rotating it invalidates every in-flight envelope** |
| `$HOME/.local/share/dux-amq/inject-queue/<receiver>/<ts>-<suffix>.msg` | raw body bytes | **the Rust drainer `src/amq_inject.rs` consumes these** |
| `${XDG_RUNTIME_DIR:-/tmp}/dux-amq/seen-nonces.d/<nonce>/` | empty dirs as replay markers | verify |
| `$HOME/.claude/projects/<encoded>` | Claude Code's own dirs | the encoder must stay bit-exact |
| `~/.bashrc` and `~/.claude/CLAUDE.md` | marker-delimited blocks | `strip_block` must match the existing markers exactly |

**Wire/CLI contracts that cannot drift:**

- The **DUX2 envelope** (7 TAB fields, MAC over the `|`-joined payload) and **DUX1** decode
  compatibility — an in-flight signed message written by bash must verify under Rust and
  vice-versa.
- The **inject-queue filename shape** `<ts>-<suffix>.msg` and the `.inflight.` reservation
  prefix — `src/amq_inject.rs` filters on both.
- **`dux doctor`'s contract**: `DUX_AMQ_DOCTOR_BIN` → `which dux-amq-doctor` → `<exe>/../dux-amq/scripts/…`;
  `--json`/`--anonymize`; exit 0; a JSON **object** at the root.
- **`amq coop exec` argv** — `--no-init --root … --me … --require-wake -y [--wake-inject-mode raw | --wake-inject-via <path>] <provider> -- <flags> <user args>`.
- **`amq wake --me <handle> --root <root>`** — `src/peer.rs::amq_wake_command_matches`
  (1308-1320) scans running processes for a command containing `amq`, the literal `wake`, and
  the `--me`/`--root` pairs. If the port ever spawns wake itself, that argv shape is load-bearing.
- **Env the dux PTY exports** (`src/model.rs:644-700`, `src/pty.rs:1217-1225`):
  `DUX_PANE=1`, `CLAUDE_AMQ_YOLO`/`CODEX_AMQ_YOLO`, `DUX_AMQ_VERIFY`, `DUX_SYSTEM_PROMPT`,
  `DUX_STORE_ID` — the wrapper's read-side must be unchanged.

**Rollout sequence (recommended):**

1. Bump `rust-toolchain.toml` to current stable; land green.
2. Add the workspace member and the shared `common/` + `encode/` modules; make `dux` depend on
   them so `purge_encoding` has exactly one home.
3. Port leaf-first, in ascending blast radius:
   `encode-claude-project-dir` → `amq-secret-init.sh` → `amq-send-signed` → `amq-receive-verify`
   → `dux-amq-doctor` → `dux-amq-inject-bridge` → the three wrappers → `finalize-claude-migration.sh`
   → the installer steps. Each step ships a symlink that replaces exactly one bash script, so
   a regression can be reverted by re-pointing one symlink.
4. Keep the bats suite running against whichever half is still bash; retire a `.bats` file only
   when its Rust replacement asserts the same list from `bats-test-parity-matrix.md`.
5. Ship `dux-amq` in the existing `release.yml` matrix (musl for Linux, native for both macOS
   arches) alongside `dux`, with the same attestation/SBOM/SHA256SUMS treatment.
6. Update `SECURITY.md` and `docs/operations/threat-model.md` in the same PR — CLAUDE.md
   mandates it and this port touches T2, T3, T7, and T14 surface. Notably: replacing the
   non-constant-time MAC compare strengthens T2/T14, and dropping the `jq`/`openssl`/`sqlite3`
   shell-outs shrinks the TCB.

**What operators must be told:**

- Nothing in `~/.bashrc`, `~/.claude/CLAUDE.md`, `$STATE_ROOT`, or the AMQ queue needs manual
  migration; re-running the installer is sufficient and idempotent.
- The `~/.bashrc` preamble-accumulation bug (§8.3) means long-lived hosts may have several
  copies of the 7 orphan comment lines. The Rust installer should offer to strip them, or the
  release notes should say to delete them by hand.
- There is **no uninstall path today** (§8.4). The port is the natural place to add
  `dux-amq uninstall`, which is also the only safe way to remove the symlink farm.

---

## Open questions

1. **`clap` or hand-rolled?** `dux` deliberately hand-parses argv (`src/cli.rs`) and has no
   clap dependency. Multicall dispatch is ~150 lines by hand. Which way does the project want
   to go — consistency with `dux`, or clap's multicall/help/completions?
2. **Symlink farm or copies?** Symlinks are ~30 MB smaller but change install.sh's
   `install -m 0755` contract and are visible to operators (`ls -l ~/.local/bin` shows
   `claude-amq -> dux-amq`). Any objection?
3. **`hmac` 0.12 + `sha2` 0.10 (no duplicate in the lock) or 0.13 + 0.11 (modern API, one extra
   `sha2` copy)?** `deny.toml` warns rather than denies on duplicates.
4. **The backgrounded bridge (hazard 4).** Keep fire-and-forget bug-compatibility, or
   `setsid`/double-fork it, or run it as a thread before `execvp`? It is untested today, so
   there is no test to break either way.
5. **`XDG_DATA_HOME` disagreement.** `dux-amq-inject-bridge:275` hardcodes
   `$HOME/.local/share/dux-amq/inject-queue`, but the Rust drainer (`src/amq_inject.rs:136-146`)
   prefers `$XDG_DATA_HOME/dux-amq/inject-queue` when that variable is set. On a host with
   `XDG_DATA_HOME` exported they write and read **different directories**. Is this a known bug
   to fix in the port (align on XDG-aware), or is the bridge's hardcoding intentional?
6. **Non-ASCII path encoding (hazard 9).** Confirm the intended semantics is one `-` per
   Unicode scalar (what `src/purge_encoding.rs` already does) rather than per byte, and add a
   fixture. Ideally re-probe a live Claude Code with a non-ASCII path to settle it empirically.
7. **Constant-time MAC compare.** Switching to `subtle::ConstantTimeEq` changes no observable
   behavior but does change the security posture claim in `SECURITY.md` T2. Land it as part of
   the port, or as a separate reviewable change?
8. **`sqlite3` CLI removal.** Using `rusqlite` deletes the `sessions_db.integrity == "sqlite3-cli-missing"`
   state entirely. Is any downstream consumer matching on that literal?
9. **Doctor JSON type inconsistencies** (`disk.top_dirs` string-or-array, `runtime.dux_pid`
   forced to 0, `amq.agents[].depth` as a string). Fix them (cleaner schema, may break an
   unknown `jq` consumer) or reproduce them faithfully?
10. **`recent_errors` unredacted-field leak** (`--json` without `--anonymize` emits raw log
    records). Tighten to the `{timestamp, message}` allowlist unconditionally, or preserve?
11. **Which installer steps actually move to Rust?** Proposal is preflight+bootstrap stays
    bash, everything after moves to `dux-amq install`. Does the project want the download and
    pin-verification in Rust too (better: real TLS + streaming sha256), or kept in curl?
12. **`install-gocryptfs.sh`** — port, keep as bash, or drop? Zero test coverage, opt-in,
    never invoked by install.sh.
13. **macOS behavior change.** The port naturally removes the GNU-only `mv -T` and
    `sed --follow-symlinks` hard-failures. Is macOS a supported target for these tools, or
    only for running the test suite?
14. **`dux-config-changes.toml` drift.** It is documentation that must be hand-synced with the
    12 `sed` substitutions. Should the Rust port make it the machine-readable source of truth
    and generate the patch from it?

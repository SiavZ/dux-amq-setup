# Bats test-parity matrix — acceptance criteria for the bash→Rust port

Source: research sub-agent under parent D, 2026-08-31. All 14 `.bats` files read
in full (2,938 lines), plus `tests/lib/setup.bash` (51) and all 5 `tests/fakes/`
files. **123 tests total**, verified by `grep -c '^@test'` per file:
amq-auth 13 · doctor 7 · encoder-fixtures 8 · finalize-migration 5 ·
inject-bridge 22 · install-idempotency 3 · ownership 7 · root-installer 2 ·
smoke 3 · strip-block 3 · supply-chain-rails 4 · tiocsti-detect 6 ·
wrappers-p1 20 · wrappers 20.

**Every behavior below must survive the port.** Anything not reproduced is a
regression, not a simplification.

## 1. Harness spec

### `tests/lib/setup.bash`

- `setup_isolated_home()` (L25-33): `TEST_HOME="$(mktemp -d -t dux-amq-test.XXXXXX)"`,
  exported; `export HOME="$TEST_HOME"` (redirects `~` for every test, so no test
  touches real dotfiles); `export PATH="$BATS_TEST_DIRNAME/fakes:$BATS_TEST_DIRNAME/../scripts:$PATH"`
  — prepends **both** the fakes dir and the real `dux-amq/scripts` dir, so
  `encode-claude-project-dir`/`amq-send-signed`/`amq-receive-verify`/
  `dux-amq-inject-bridge`/`amq-secret-init.sh` resolve to real scripts while
  `amq`/`claude`/`codex`/`gemini`/`flock` resolve to fakes (fakes dir wins ties).
- `teardown_isolated_home()` (L37-42): `rm -rf "$TEST_HOME"` guarded by
  `[[ -n "${TEST_HOME:-}" && -d "$TEST_HOME" ]]`, then `unset TEST_HOME` — safe twice.
- `touch_epoch(epoch, path)` (L44-51): portable mtime-setter — GNU `touch -d "@$epoch"`,
  falling back to BSD `date -r "$epoch" +%Y%m%d%H%M.%S` | `touch -t`.

### Fakes (`tests/fakes/`)

| Fake | Records to | Record format | stdout | Exit | Notes |
|---|---|---|---|---|---|
| `amq` (31 ln) | `$AMQ_FAKE_ARGV_FILE` (append), only if non-empty | `ARGV\n<arg0>\n<arg1>\n...\nEND\n`, one arg per line, per invocation | nothing | `42` if `AMQ_FAKE_FAIL_RECOVER_OWNER=1` **and** `$1=="wake"` **and** `$2=="recover-owner"`; else `0` | argv logging independent of fail-injection |
| `claude` (6 ln) | — | — | `2.1.207 (Claude Code)\n` iff `$1 == "--version"` | **1** for any non-`--version` invocation (see hazard 8); `0` for `--version` | — |
| `codex` (6 ln) | — | — | `codex-cli 0.144.1\n` iff `--version` | same quirk | — |
| `gemini` (6 ln) | — | — | `0.50.0\n` iff `--version` | same quirk | — |
| `flock` (48 ln) | — | — | passthrough | delegates | **Test-only macOS shim.** (1) if `/usr/bin/flock` or `/bin/flock` exists (Linux CI), `exec`s it; (2) else `flock [-n] <fd>` form + `ruby` → Ruby `File#flock` on `/dev/fd/<fd>`; (3) else `flock -x <path> <cmd...>` + `ruby` → Ruby open 0600 + `LOCK_EX` + `system(*ARGV)`, `exit 75` if unlockable; (4) **no ruby**: `mkdir "$lock.test-lock"` spin loop, 500 × 10 ms = 5 s, `trap 'rmdir' EXIT`; `64` on malformed invocation, `75` on timeout |

**Rust harness implication**: a faithful rig needs (a) an argv-recording stub with
the exact `ARGV\n...\nEND\n` framing many tests `grep -Fxq` against literal lines,
(b) version-string stubs, (c) a real cross-platform advisory-lock crate in the test
double, or an accepted-risk mkdir-spinlock equivalent for CI without a native primitive.

## 2. The matrix

### `amq-auth.bats` (13) — `amq-secret-init.sh`, `amq-send-signed`, `amq-receive-verify`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| secret is 32-byte base64, mode 0600 (L45) | fresh `$HOME` | exists at `$HOME/.local/share/dux-amq/amq-secret`; `43 ≤ wc -c ≤ 46`; mode `"600"` | 256-bit base64 secret, mode 0600 |
| drops unsigned messages (L58) | plain text stdin | `status -eq 0`; stdout empty or `^\[amq-verify\]`; stderr has `"dropping unsigned"` | unsigned input silently dropped (exit 0), never echoed |
| accepts well-signed, emits body (L70) | envelope via `--print-only` | `status -eq 0`; `output == "hello"` | valid DUX2 envelope's exact body on stdout |
| rejects replay (nonce dedup) (L79) | same envelope twice | 1st `output=="hi"`; 2nd `*"replay rejected"*`; 3rd stdout-only `-z` | repeated nonce rejected on 2nd delivery, body never re-emitted |
| rejects MAC mismatch (L96) | field 6 replaced with `RVZJTA==` | `*"HMAC mismatch"*`; stdout empty | tampered MAC rejected, body withheld |
| accepts envelope as argv `$1` (L118) | stdin closed `</dev/null` | `output == "via-argv"` | argv[1] accepted when stdin closed (AMQ v0.34.0 `--inject-via`) |
| drops unsigned argv-mode (L127) | unsigned argv | `*"dropping unsigned"*`; stdout empty | argv-mode applies same unsigned rule |
| argv-mode replay rejection (L136) | same envelope twice via argv | 1st `"argv-dup"`; 2nd `*"replay rejected"*` | argv-mode shares nonce store with stdin-mode |
| prefers stdin over argv (L149) | distinct stdin + argv envelopes | `output == "from-stdin"` | stdin wins when both present |
| falls back to argv when stdin empty (L159) | stdin from `<(:)` | `output == "stdin-empty-argv-wins"` | empty/failed stdin falls through to argv |
| round-trips tabs/Unicode/trailing newlines (L169) | body `$'line one\t中\nline two\n'` | `cmp` succeeds | base64 body field preserves bytes exactly |
| rejects wrong recipient (L180) | `--to bob`, verify as `AM_ME=mallory` | `*"wrong recipient"*`; stdout empty | recipient binding enforced |
| concurrent replay: exactly one wins (L190) | two backgrounded verifies of one envelope | exactly one output `=="one-owner"`; stderr has `"replay rejected"` | nonce dedup is race-free |

### `doctor.bats` (7) — `dux-amq-doctor`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| all expected sections (L78) | `seed_amq_state` (3 agents, 4 inbox files) | contains `"== Versions =="`, `"== Binary integrity =="`, `"== Persistent disk =="`, `"== AMQ =="`, `"== Symlinks =="`, `"== Kernel =="`, `"== Sessions DB =="`, `"== Runtime =="`, `"== Recent errors"` | exactly these 9 headers always render, even on partial install |
| binary integrity behind timeout (L94) | fake `timeout`/`sha256sum` shims | `*"sha256 matches"*`; `grep -F "5 sha256sum $amq_bin" "$timeout_log"` | must invoke `timeout 5 sha256sum <pinned-binary>` |
| `--anonymize` redacts (L129) | seeded AMQ + 2 worktrees + `dux.log` JSON line naming a worktree | excludes real `$HOME`, `"alpha"`,`"bravo"`,`"charlie"`; includes `"agent-1"`,`"agent-2"`,`"agent-3"` | redacts `$HOME` paths + numbers agent handles, including inside scraped log lines |
| `--json` valid (L160) | seeded | `jq -e .`; `jq -er '.versions,.binary_integrity,.amq,.symlinks,.kernel,.sessions_db,.runtime,.recent_errors \| type'` | one valid JSON object, all 8 top-level keys present and typed |
| `--json --anonymize` (L172) | seeded + worktree | `jq -e .`; excludes real `$HOME`, `'"alpha"'`,`'"bravo"'`; includes `'agent-1'` | JSON mode anonymizes structured values too |
| sqlite read-only (L184, skips w/o `sqlite3`) | DB with table + `pragma user_version=77` | before/after sha256, `.schema`, `user_version` all identical | strictly read-only — zero byte/schema/pragma mutation |
| exits 0 when AMQ uninitialized (L217) | no `meta/config.json` | `status -eq 0`; `"== AMQ =="` + (`"uninitialized"` or `"no $AMQ_GLOBAL_ROOT/meta/config.json"`); `jq -er '.amq.initialized == false'` | never fails merely because AMQ isn't initialized |

### `encoder-fixtures.bats` (8) — `encode-claude-project-dir` + sourced `is_dux_worktree`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| matches recorded fixtures (L35) | `tests/fixtures/claude-paths.txt`, 12 TAB-separated pairs | per-line `actual == expected`; `count >= 6` | output matches every probe pair from a live Claude Code install; fixture retains ≥6 cases |
| rejects relative path (L64) | `"relative/path"` | `status -eq 2`; `*"absolute path required"*` | non-absolute rejected, exit 2, exact phrase |
| strips single trailing slash (L73) | `"/foo/bar/"` | `out == "-foo-bar"` | exactly one trailing `/` stripped |
| preserves case (L79) | `"/Foo/BarBaz"` | `out == "-Foo-BarBaz"` | never lowercased |
| rejects sibling `/worktrees-evil` (L117) | fn sourced from `claude-amq` via `awk`; cwd `.../worktrees-evil/x` | `status -ne 0` | containment check rejects prefix-matching siblings (audit01 P0-5) |
| accepts `/worktrees/x` (L129) | same | `status -eq 0` | genuine paths accepted |
| rejects nonexistent `DUX_HOME` (L140) | `DUX_HOME=".../nope"` | `status -ne 0` | `realpath` failure rejects, no crash/false-accept |
| **byte-identical across 3 wrappers** (L148) | `awk`-extract fn body from all 3 | `[[ "$c" == "$x" ]]`, `[[ "$c" == "$g" ]]` | **structural, not behavioral** — see hazard 6, delete rather than port |

### `finalize-migration.bats` (5) — `finalize-claude-migration.sh`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| aborts when claude running (L102) | fake `pgrep` exits 0 | `status -ne 0`; `grep -qi 'claude.*running'`; `$HOME/.claude` still real dir w/ marker; `[ ! -L ]` | refuses + leaves source untouched while any claude process alive |
| idempotent on finalized tree (L126) | `pgrep` exits 1; run twice | both `status -eq 0`; `-L` on `.claude` and `.agents`; 2nd run's `rsync.log` unchanged; `grep -q 'already a symlink'` | 2nd run is a true no-op — no rsync call, exact message |
| preserves pre-existing dest files (L158) | pre-seeded `$STATE_ROOT/claude/preserved.txt` | survives with `"keep me"`; `! grep -- '--delete' rsync.log` | default rsync never passes `--delete` |
| honors `FINALIZE_FORCE_DELETE=1` (L186) | env set | `grep -- '--delete' rsync.log` | opt-in adds `--delete` |
| parallel: one wins, one fails fast (L207) | test holds `flock -n 8` on lock file | 1st `status -ne 0` + `grep -qi 'another instance'`; after release 2nd `status -eq 0` | non-blocking flock; loser fails fast, no hang/corruption |

### `inject-bridge.bats` (22) — `dux-amq-inject-bridge`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| tmux send-keys when not under dux (L128) | `$TMUX` set, no `$DUX_PANE` | tmux log `grep -Fxq` `"send-keys"`,`"hello-world"`,`"Enter"`; no queue file | outside dux, deliver via `tmux send-keys ... Enter` |
| uses `DUX_TMUX_TARGET` (L144) | `="mywin:0.1"` | tmux log has `"-t"`,`"mywin:0.1"` | passed as `-t <target>` |
| (strict) drops unsigned (L157) | `DUX_AMQ_VERIFY=1`, spoofed text | `[ ! -s "$TMUX_LOG" ]`; no queue file | strict drops before any delivery |
| (strict) drops MAC mismatch (L171) | tampered MAC | `[ ! -s "$TMUX_LOG" ]` | strict drops bad MAC |
| falls back to `.unrouted` (L190) | no `$TMUX`, no `$AM_ME` | exactly one `inject-queue/.unrouted/*.msg` w/ `"queued-msg"`; no flat top-level `*.msg` | missing identity → `.unrouted/` subdir, never queue root |
| `_unrouted` is a legit receiver (L206) | `AM_ME="_unrouted"` | one `inject-queue/_unrouted/*.msg`; none under `.unrouted/` | literal handle must not collide with fallback bucket |
| empty argv silent (L222) | `""` | `status -eq 0`; no queue file | unconditional no-op |
| prefers queue over tmux w/ `DUX_PANE` (L230) | tmux available + `DUX_PANE=1` | `[ ! -s "$TMUX_LOG" ]`; one `bob/*.msg` w/ `"under-dux"` | `DUX_PANE` forces file-queue path |
| drops under-dux wake when `DUX_PID` dead (L249) | `DUX_PANE=1`, `DUX_PID="999999999"` | no tmux call; no queue file | dead PID drops entirely (shutdown race) |
| auto-drains AMQ w/ `AM_ROOT` (L265) | fake `amq drain` returns 1 msg; `DUX_PANE=1` | drain argv has `"drain"`,`"--root"`,`"$AM_ROOT"`,`"--me"`,`"bob"`,`"--json"`; file has `"[AMQ] Drained messages (JSON):"`,`"drained-body"`,`"Act on these AMQ messages now"`; `! grep -q $'\003'` | converts wake into real `amq drain --include-body --json`, strips control bytes |
| ignores empty JSON drain (L296) | drain returns `count:0` | `status -eq 0`; no queue file | zero-message drain queues nothing |
| drains downtime mail after exact wake (L310) | **no argv**; `DUX_AMQ_STARTUP_OWNER_PID="$$"`; `.wake.lock`/`.wake.prepared` seeded w/ matching generation | drain argv has `"drain"`; one file w/ `"drained-body"` | startup-backlog mode polls for matching generation then drains+queues |
| strict signed bypasses drain (L335) | `DUX_AMQ_VERIFY=1`, `AM_ROOT` set, body w/ tab+newline | `[ ! -s "$AMQ_DRAIN_ARGV_LOG" ]`; `cmp` exact bytes | strict signed delivery must NOT auto-drain; queues body byte-for-byte |
| drops raw ctrl-c under dux (L359) | `$'\003'` w/ `DUX_PANE=1` | no queue file | raw Ctrl-C transport signal is a pure no-op |
| keys queue on sanitised `AM_ME` (L368) | `AM_ME="bob"` | one `bob/*.msg` | subdir equals clean handle |
| sanitises `AM_ME` (L384) | `="Feature/Login.v2"` | one `inject-queue/feature-login-v2/*.msg`; `[ ! -d ".../Feature/Login.v2" ]` | lowercase + `[^a-z0-9_-]→-`, mirroring wrapper normalization |
| rejects traversal `AM_ME` (L404) | `="../../etc"` | nothing outside `inject-queue/`; `[ ! -e "$HOME/.local/share/etc" ]` | traversal neutralized |
| (skip) delivers unsigned via tmux (L420) | default | tmux log has `"send-keys"`,`"hello-from-legacy-amq-send"`,`"Enter"` | default skip mode delivers legacy unsigned as-is |
| (skip) unwraps DUX2 w/o MAC check (L432) | MAC `+"TAMPERED"` | tmux log has `"unwrap-me"` | skip mode decodes body, never checks MAC |
| (skip) malformed DUX2 → raw (L450) | `$'DUX2\talice'` (2 of 7 fields) | tmux log has `"DUX2"` and `"alice"` | too-few-fields falls back to raw delivery |
| (skip) empty argv silent (L465) | — | `[ ! -s "$TMUX_LOG" ]` | matches strict mode |
| (skip) preserves internal TABs (L474) | `$'line1\tline2'` | tmux log has `"line1"` and `"line2"` | TABs survive base64 round-trip |

### `install-idempotency.bats` (3) — `dux-amq/install.sh` + `bashrc-additions.sh`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| P0-01 hermetic rerun preserves config (L63, Linux-only) | fake `dux` exiting 97 if invoked w/ `config`; hand-authored `config.toml` w/ comments/unknown keys/macros/custom providers; `AMQ_BINARY_SHA256` pinned | `status -eq 0`; config still a symlink; `cmp` byte-identical; `.bashrc` has no `"/data/state"`; sourcing `.bashrc` reproduces `STATE_ROOT\|DUX_HOME\|AMQ_GLOBAL_ROOT` | fresh install never invokes `dux config` when config exists; preserves every user byte |
| P0-F/P0-01 second install preserves AMQ + config (L143, needs real `/data`+`dux`+`amq`) | real install ×2; sentinel in AMQ `config.json`; user edits in `config.toml` | `grep -q "preserved-by-idempotent-install"`; `cmp` config | 2nd install must not `amq init --force` or overwrite user TOML |
| N-3 guard refuses when `binary.sha256` removed (L212) | real install then `rm -f "$STATE_ROOT/amq/binary.sha256"` | sourcing w/ `set -e` gives `status -ne 0` | guard fails **closed**, not silently skipping |

### `ownership.bats` (7) — shared registration/locking across all 3 wrappers

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| atomic store+session marker, no wake PID (L23) | loop 3 providers w/ `DUX_STORE_ID`/`DUX_SESSION_ID`/`DUX_AMQ_HANDLE` | `jq -e '.store_id=="store-a" and .session_id==$session and (has("wake_pid")\|not)'`; dir mode `"700"` | marker is JSON w/ exactly store_id/session_id (no legacy `wake_pid`), dir 0700 |
| managed handle beats inherited `AM_ME` (L40) | `AM_ME=wrong` + `DUX_AMQ_HANDLE=right` | marker at `.../right/...`; `.../wrong` absent | managed tri-var bundle takes priority |
| fails closed w/o flock (L49) | `DUX_AMQ_FLOCK=definitely-not-a-flock-command` | `status -ne 0`; `*"mandatory flock utility is unavailable"*`; agent dir never created | hard-fail before any registration write |
| partial managed env fails closed (L57) | only 2 of 3 vars | `*"must be set together"*` | tri-var bundle is all-or-nothing |
| managed handle validated verbatim (L64) | `DUX_AMQ_HANDLE='Bad/Handle'` | `*"invalid or overlong AMQ handle"*`; no `bad-handle` dir | rejected outright, never silently normalized |
| foreign JSON owner preserved (L73) | marker w/ different store/session | `status -ne 0`; marker unchanged (`store-b`/`foreign`) | never overwrite a foreign managed owner |
| concurrent wrappers: valid config, exact owners (L85) | 3 providers launched simultaneously | `.agents == ["claude-agent","codex-agent","gemini-agent"]`; each marker correct | flock-serialized registration is race-free cross-provider |

### `root-installer.bats` (2) — top-level `install.sh`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| downloads from releasing repo (L74) | fake `uname`/`curl`/`tar`; matching checksum | `-x "$INSTALL_DIR/dux"`; curl log has `"github.com/SiavZ/dux-amq-setup/releases/download/v1.2.3/dux-linux-amd64.tar.gz"` + matching `SHA256SUMS` URL | exact release URL built from `$DUX_VERSION`; working binary extracted |
| rejects checksum mismatch before extraction (L93) | wrong checksum | `status -ne 0`; `*"Checksum mismatch"*`; `tar` never invoked; no binary | abort before extraction, exact message |

### `smoke.bats` (3) — the harness itself

| Test (line) | Assertions | Pinned requirement |
|---|---|---|
| fresh `$HOME` under /tmp (L17) | `-n`/`-d "$TEST_HOME"`; `"$HOME" = "$TEST_HOME"`; under `/tmp/*`, `/var/folders/*`, or `$TMPDIR*` | isolated home is real tmp dir and becomes `$HOME` |
| prepends fakes to PATH (L27) | `$PATH` starts with `"$BATS_TEST_DIRNAME/fakes":` | fakes dir first |
| teardown idempotent (L34) | dir gone; `TEST_HOME` unset; 2nd call no error | safe twice |

### `strip-block.bats` (3) — sourced `strip_block` from `install.sh`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| P0-G legacy md preserves user content (L38) | fixture w/ legacy heading then user's `## My personal notes` | `grep -q "DO NOT DELETE"`; `grep -q "## My personal notes"`; `! grep -q "old content..."` | legacy block removal must not delete to EOF (the audit02 P0-G bug) |
| P0-G versioned markers stripped (L53) | `<!-- >>> dux-amq v0.0.9 >>> -->` … `<<<` | old block gone; `"keep me"` survives | versioned blocks fully removed |
| P0-G explicit end-sentinel resets (L70) | `<!-- end dux-amq legacy -->` | legacy stanza gone; text after sentinel survives | sentinel resets the "still stripping" state |

### `supply-chain-rails.bats` (4) — **all static text assertions, no runtime behavior**

| Test (line) | Assertions | Pinned requirement |
|---|---|---|
| Claude Peers pinned commit (L7) | 40-hex `CLAUDE_PEERS_REV=` default; literal `git -C "$CLAUDE_PEERS_DIR" checkout --detach "$CLAUDE_PEERS_REV"`; literal `peers_head=$(git -C "$CLAUDE_PEERS_DIR" rev-parse HEAD`; literal `[[ "$peers_head" != "$CLAUDE_PEERS_REV" ]]` | install.sh text pins an exact commit and verifies checkout |
| every workflow `cargo install` versioned (L19) | every matched line contains `"--version "` | no unpinned `cargo install` in CI |
| release packaging reproducible (L31) | literal `SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)`, `--mtime="@${SOURCE_DATE_EPOCH}"`, `package "${{ matrix.archive }}.first"`; **plus a real double-tar `cmp`** | reproducible-build flags pinned + independently verified |
| RustSec exceptions carry rationale + date (L56) | `deny.toml` + 2 workflows each contain `'RUSTSEC-2025-0141'`, `'RUSTSEC-2024-0384'`, `'2026-08-01'`, and `unmaintained\|no patched\|no upstream replacement` | same IDs, review date, rationale in all three files |

### `tiocsti-detect.bats` (6) — sourced `tiocsti_status` from `install.sh`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| file absent → rc=2 (L56) | file removed | `status -eq 2` | missing procfs/sysctl ⇒ 2 |
| file=1 → rc=0 (L62) | `1` | `status -eq 0` | usable |
| file=0 → rc=1 (L68) | `0` | `status -eq 1` | compiled in, disabled |
| garbage → rc=2 (L74) | `banana` | `status -eq 2` | unrecognized ⇒ 2 |
| empty → rc=2 (L80) | empty | `status -eq 2` | empty ⇒ 2 |
| install.sh writes/removes sentinel (L89) | greps install.sh text | `grep -c "printf 'tiocsti_disabled" >= 1`; `grep -c 'rm -f "\$TIOCSTI_FLAG"' >= 1` | **structural** — write site and clear site both present |

### `wrappers-p1.bats` (20) — P1 hygiene bundle

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| P1-B require managed raw wake (L58) | 3 providers, default env | argv has `"--require-wake"`,`"-y"`,`"--wake-inject-mode"`,`"raw"`; **not** `"--no-wake"` | managed wake required by default (raw mode) |
| P1-B bridge mode delegated (L72) | `DUX_AMQ_INJECT_MODE=via` on codex | argv has `"--require-wake"`,`"--wake-inject-via"`, bridge abs path | `via` passes bridge path, still managed |
| P1-B recover dead exact owner (L80) | `.wake.lock` seeded w/ `owner.pid` | 2nd run's first two `amq` calls are exactly `wake` then `coop` (awk over consecutive ARGV blocks); `recover-owner` present | stale owner PID triggers `amq wake recover-owner` before `coop exec`, all 3 providers |
| P1-B fail closed on refused recovery (L97) | `AMQ_FAKE_FAIL_RECOVER_OWNER=1` | `status -eq 42`; only one ARGV block (`wake`), no `coop` | propagates exact exit code; never reaches launch |
| P1-B ownerless legacy left for coop (L108) | `.wake.lock` w/ `wake_mode` but no `.owner.pid` | 2nd run's single ARGV block is `coop`, no `wake`; `-y` present | legacy ownerless lock NOT recovered — left for coop's `-y` migration |
| P1-F dynamic handles registered (L122) | pre-existing `config.json` w/ 3 agents | `.agents \| index("claude-pane")` etc. non-null | dynamic handles appended to shared list |
| P1-C preflight reports ALL missing tools (L144) | `PATH` = empty dir | `status -ne 0`; `*"missing required tools:"*`; has `"curl"`,`"jq"`,`"openssl"` | aggregate all missing, don't bail on first |
| P1-C preflight lists realpath+openssl (L173) | same | output has `"realpath"`,`"openssl"` | those two named when missing |
| P1-D seed reports warning count (L233, skips w/o git/rsync/root) | real worktrees + one `chmod 000` file | `*"rsync warnings"*` | partial seed must say so |
| P1-D seed plain count on clean rsync (L252) | clean worktrees | `*"seeded "*" past sessions"*`; no `"rsync warnings"` | clean seed keeps original message |
| P1-E guard refuses newer binary (L294) | binary mtime +120 s vs record | `status -ne 0`; `*"newer than recorded hash"*` | mtime-newer refuses, exact banner |
| P1-E guard accepts newer record (L324) | record newer | output lacks `"newer than recorded hash"` | guard must not fire when record is newer |
| P1-F refuse handle from different `$PWD` (L357) | register from `$TEST_HOME`, rerun from `$TEST_HOME/elsewhere` | `status -ne 0`; `*"identity collision"*`; marker unchanged | handle owned by one dir refuses another, marker untouched |
| P1-F same handle same `$PWD` idempotent (L385) | twice from same dir | both `status -eq 0` | identical relaunch never flagged |
| P1-F stale legacy preserved as foreign (L397) | marker = symlink to nonexistent path, different cwd | `status -ne 0`; `*"identity collision"*`; symlink target unchanged | broken symlink still blocks, preserved not repaired |
| P1-F codex enforces collision (L408) | codex variant | same | identical for codex |
| P1-F gemini enforces collision (L419) | gemini variant | same | identical for gemini |
| P1-F simultaneous: one owner, one launcher (L430) | two concurrent claude-amq from different dirs | exit codes exactly `"0:1"` or `"1:0"`; exactly one `coop` in argv log; marker points at winner | exactly one winner, never both |
| version floors per provider (L471) | fake old versions `2.1.162`/`0.38.9`/`0.39.0` + unparseable | each `status -ne 0`; exact `"upgrade to >= 2.1.163"`, `"upgrade to >= 0.39.0"`, `"upgrade to >= 0.39.1"`; unparseable → `"version could not be parsed"` | floors and wording pinned per provider |
| derives state from custom `STATE_ROOT` (L504) | only `STATE_ROOT` set | marker at `"$STATE_ROOT/amq/agents/custom-root/.dux-amq-source"`, symlinked to `$TEST_HOME` | all paths key off `STATE_ROOT` alone |

### `wrappers.bats` (20) — YOLO defaults, seeding, `DUX_SYSTEM_PROMPT`

| Test (line) | Setup | Assertions | Pinned requirement |
|---|---|---|---|
| claude no YOLO by default (L87) | default | argv lacks `"--dangerously-skip-permissions"` | YOLO opt-in, never default |
| claude YOLO via `CLAUDE_AMQ_YOLO=1` (L93) | set | argv has flag | new-name opt-in |
| claude YOLO via legacy `CLAUDE_YOLO=1` (L99) | set | argv has flag | legacy name still works |
| `CLAUDE_AMQ_SAFE` warns only (L105) | set | `"CLAUDE_AMQ_SAFE is deprecated"`; argv still lacks flag | deprecated var only warns |
| claude loads peers channel by default (L114) | default | argv has `"--dangerously-load-development-channels"`,`"server:claude-peers"` | on by default |
| peers channel disableable (L121) | `CLAUDE_PEERS_DISABLE=1` | both absent | fully suppressible |
| codex no sandbox-bypass by default (L128) | default | both `--dangerously-bypass-approvals-and-sandbox` and `--dangerously-bypass-hook-trust` absent | default-denies both |
| codex YOLO → sandbox-bypass only (L135) | `CODEX_AMQ_YOLO=1` | sandbox-bypass present; hook-trust absent | YOLO controls only sandbox-bypass |
| codex legacy `CLAUDE_YOLO=1` (L142) | set | same | shared legacy var drives codex |
| codex hook-trust needs own opt-in (L149) | `CODEX_AMQ_BYPASS_HOOK_TRUST=1`, YOLO unset | sandbox absent; hook-trust present; `"hook trust review bypass enabled"` | independent opt-in |
| claude does NOT seed by default (L196) | real worktrees | `[ ! -e "$CHILD_SESS_DIR/sample.jsonl" ]` | seeding off by default |
| claude seeds w/ `CLAUDE_AMQ_SEED_FROM_PARENT=1` (L207) | set | `-f "$CHILD_SESS_DIR/sample.jsonl"` | opt-in triggers seeding |
| claude `--append-system-prompt` (L253) | `DUX_SYSTEM_PROMPT="be concise"` | argv has `"--append-system-prompt"` then `"be concise"` as **consecutive** entries | flag+value adjacency |
| unset → no flag (L263) | unset | absent | — |
| empty → no flag (L270) | `""` | absent | empty treated as unset |
| multi-line survives exec (L276) | `$'line one\nline two'` | argv has flag; `"line one"` in log | newlines survive argv boundary |
| codex warns and drops (L293) | set | flag+value absent; output exactly `"codex-amq: DUX_SYSTEM_PROMPT set but codex has no system-prompt flag..."` | warn-and-drop, never forward |
| codex silent when unset (L303) | unset | output lacks `"DUX_SYSTEM_PROMPT"` | no spurious diagnostic |
| gemini warns and drops (L311) | set | absent; `"gemini-amq: DUX_SYSTEM_PROMPT set but gemini has no equivalent flag..."` | pinned verbatim |
| gemini silent when unset (L320) | unset | output lacks var name | same guarantee |

## 3. Coverage map + untested behaviors

| Script | Tests | Coverage |
|---|---|---|
| `wrappers/claude-amq` | ownership (7), wrappers (~12), wrappers-p1 (most of 20) | Heavy — version gate, YOLO, Peers channel, system-prompt, seeding, identity collision, flock-missing, managed identity, concurrent registration |
| `wrappers/codex-amq` | shared provider loops + codex-specific | Heavy |
| `wrappers/gemini-amq` | shared loops + its warn-drop | Good but thinner — no YOLO surface exists (correctly untested) |
| `scripts/amq-secret-init.sh` | 1 direct + fixture setup | Thin — **idempotent re-run never tested** (every test starts fresh) |
| `scripts/amq-send-signed` | ~25 call sites, always `--print-only` | **Its own CLI essentially untested**: missing `--to`/`--me`/`--body`, unknown-arg, missing/unreadable secret, and the real `exec amq send` hand-off |
| `scripts/amq-receive-verify` | 12 of 13 | Heavy |
| `scripts/dux-amq-inject-bridge` | 22 | Heavy in isolation |
| `scripts/dux-amq-doctor` | 7 | All 9 headers checked; deep assertions only for Versions/Binary-integrity/AMQ/Sessions-DB/JSON/anonymize. **`Persistent disk`, `Symlinks`, `Kernel`, `Runtime` have no assertion beyond header presence** |
| `scripts/finalize-claude-migration.sh` | 5 | Covers all 5 criteria; real-rsync-failure never exercised (fake always exits 0) |
| `scripts/encode-claude-project-dir` | 12 fixtures + 3 units | Good ASCII/case/slash; **no non-ASCII, no bare `/`, no embedded newline** |
| `scripts/install-gocryptfs.sh` | **Zero** (`grep -rl gocryptfs tests/` empty) | None |
| `dux-amq/install.sh` | strip-block (3, fn extraction), tiocsti (6), wrappers-p1 P1-C (2), install-idempotency (3, env-gated) | Thin vs 565 lines. **Binary download/pinning, Claude Peers checkout execution, npx/bun/claude auto-detection, `configure_vscode_remote` (L488) have zero coverage** |
| `install.sh` (root) | 2 | Linux/curl/checksum happy path + mismatch abort. **darwin/`detect_os`/`detect_arch`, `wget` fallback, unsupported OS/arch untested** |
| `config/bashrc-additions.sh` | N-3 + P1-E (2) | Failure paths well covered; **the successful `eval "$("$AMQ_BIN" shell-setup)"` happy path never exercised** |

### Confirmed untested behaviors (verified by grep/read, not inferred)

1. **The backgrounded `DUX_AMQ_STARTUP_OWNER_PID="$$" "$BRIDGE" &` launch** (claude-amq:390 / codex-amq:235 / gemini-amq:223) is never exercised through any wrapper test. No test invokes a wrapper under `DUX_PANE` and checks that the wrapper doesn't block, that the bridge is actually spawned, or how it interacts with the wrapper's own `exec`.
2. **No signal/trap behavior is tested anywhere** — consistent with the wrappers having zero `trap` statements.
3. **Oneshot `exec` short-circuits completely untested.** No test invokes `claude-amq -p`, `--print`, `--bare`, `gemini-amq -p`/`--print`, or `codex-amq exec ...`.
4. **`%q` printf escaping untested at character level.** The `"version could not be parsed"` branch uses `%q` to embed `$output` but only substring prose is asserted.
5. **`config.json.tmp.XXXXXX` cleanup on failed final `mv`** — no test simulates EPERM/ENOSPC inside `claim_and_register_locked`.
6. **Identity-resolution priorities #2–#4 effectively untested end-to-end.** Every test sets `AM_ME` or the managed bundle. `is_dux_worktree`-driven identity (`basename "$PWD"`) is tested only at raw-function level; the git-branch fallback (`git symbolic-ref`) and the final `claude-$$`/`codex-$$`/`gemini-$$` PID fallback are **never exercised at all**.
7. Doctor's `Persistent disk`, `Symlinks`, `Kernel`, `Runtime` sections — header presence only.
8. `install-gocryptfs.sh` — nothing (preflight, passfile-permission refusal, already-mounted no-op, `mountpoint` detection).
9. `amq-send-signed`'s own CLI surface.
10. `dux-amq-inject-bridge`'s "amq drain returned invalid JSON" branch — only `count==0` and success shapes covered.
11. install.sh's binary download/pinning, Peers checkout **execution** (only grepped), npx/bun/claude auto-detection, `configure_vscode_remote`.
12. Root installer's darwin path, `wget` fallback, unsupported OS/arch errors.
13. `bashrc-additions.sh`'s successful `eval` happy path.

## 4. Hazards for the port

1. **PATH-shimmed fakes assume subprocess dispatch.** Every wrapper test works only because `tests/fakes/amq` precedes the real binary and the wrapper does a literal `exec amq coop exec ...`. If the Rust port reimplements that in-process, the `AMQ_FAKE_ARGV_FILE` mechanism has nothing to hook — **~60 argv-assertion tests** (ownership, wrappers, wrappers-p1) must be re-targeted at an in-process seam (e.g. a testable "build launch argv" function) rather than ported as black-box subprocess tests.
2. **`run` merges stdout+stderr into `$output`.** Dozens of assertions don't distinguish streams; some tests explicitly re-run with `2>&1 1>/dev/null` to isolate stderr. A Rust port must decide stream-by-stream where each message goes, since `assert_cmd` won't merge unless configured to.
3. **Exact wording is pinned dozens of times** — `"mandatory flock utility is unavailable"`, `"must be set together"`, `"invalid or overlong AMQ handle"`, `"identity collision"`, `"upgrade to >= X.Y.Z"`, `"version could not be parsed"`, `"CLAUDE_AMQ_SAFE is deprecated"`, both provider warn-drop sentences, `"hook trust review bypass enabled"`, `"seeded N past sessions"`/`"rsync warnings"`, `"another instance"`, `"already a symlink"`, doctor's `"sha256 matches"`/`"uninitialized"`. Several are built with `%q` shell-escaping whose exact shape a Rust `format!` will not naturally match.
4. **`$BATS_TEST_TMPDIR`/`$BATS_TEST_DIRNAME` are bats-specific** — a Rust suite needs `tempfile`; any bats file kept to drive the compiled binary keeps bats in the toolchain.
5. **Sourced-function tests have no Rust analogue — 3 files, 12 tests:**
   - `encoder-fixtures.bats` `source_is_dux_worktree()` (L100-115) awk-extracts `is_dux_worktree() {...}` from `claude-amq` and sources it (tests L117, L129, L140, plus L148 string-diffing all three).
   - `tiocsti-detect.bats` (setup L32-37) awk-extracts `tiocsti_status() {...}` from `install.sh`; 5 tests call it directly.
   - `strip-block.bats` extracts every helper up to `# 1. preflight` (stripping `set -euo pipefail` so it doesn't hijack bats' options); 3 tests call `strip_block`.
   None exercise a compiled artifact. The port must expose equivalent logic as `pub fn` with native `#[test]`s carrying the same input/output pairs, or keep a tiny script/subcommand. **These bats files cannot be reused unmodified.**
6. **Tests asserting file *content* not behavior — do not port as-is:**
   - `encoder-fixtures.bats` L148 (byte-identical across 3 wrappers) is meaningless once there's one shared Rust impl — **delete**, replace with one unit test.
   - **All four `supply-chain-rails.bats` tests** are `grep` assertions against literal bash/YAML source text that won't exist in the same form — re-anchor to the new build/release tooling. (The real double-tar `cmp` reproducibility check *is* worth porting; the surrounding greps are not.)
   - `tiocsti-detect.bats` L89 is a `grep -c` count against install.sh text.
   - By contrast `install-idempotency.bats` L136 (`! grep -Fq -- "/data/state" "$HOME/.bashrc"`) is **behavioral** (checks the installer's resolved output) — port normally.
7. **The `flock` fake changes lock semantics on macOS.** Without native flock and without ruby, it degrades to a mkdir spin-loop with a 5 s ceiling. `ownership.bats` L85 and `wrappers-p1.bats` L430 then exercise the spinlock's correctness rather than a true kernel advisory lock — they can pass while masking a race real `flock(2)` would prevent, and flake under load from the 5 s ceiling.
8. **`tests/fakes/{claude,codex,gemini}` exit 1 for any non-`--version` invocation.** `if [[ "${1:-}" == "--version" ]]; then printf ...; fi` with nothing after it means the script's exit status *is* the failed `[[ ]]` test — **exit 1**, not 0. Currently inert (fakes are only invoked for the version check), but do not port them assuming "prints nothing, exits 0".
9. **Environment-gated tests silently skip.** `install-idempotency.bats` tests 2-3 skip unless `/data` exists and both `dux` and `amq` are on PATH — on a dev laptop they report skipped, masking a broken idempotency guarantee.
10. **`wrappers-p1.bats` P1-D seed tests skip under root** (`chmod 000` is a no-op for root) — containerized CI running as root silently skips the one test proving rsync-warning surfacing works.

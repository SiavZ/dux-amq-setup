# Threat Model — long-form companion

This document is the long-form companion to the STRIDE table in
[`/SECURITY.md`](../../SECURITY.md). For each row T1–T20 we capture
the concrete attack scenario, the mitigation in code (with
file:line references taken from `docs/audits/audit02.md`), the
residual risk after mitigation, and the detection mechanism — what
shows up in `dux.log` or `dux-amq doctor` output when the threat
fires.

The audit reports in `docs/audits/` are point-in-time snapshots.
This file and `SECURITY.md` are the living artifacts and must be
updated whenever new attack surface is added.

---

## T1 — Malicious repo executes through permission, sandbox, or hook-trust bypasses

**Attack scenario.** An operator clones a third-party repository
and opens a `dux` pane. The repo contains a `.claude/`
configuration, a poisoned README, or a doc string that prompt-injects
the running Claude session. Because the wrapper previously passed
`--dangerously-skip-permissions` to `claude` and
`--dangerously-bypass-approvals-and-sandbox` and
`--dangerously-bypass-hook-trust` to `codex` by
**default**, the injected payload runs arbitrary commands inside
the operator's Linux account: exfiltrating `~/.claude/.credentials.json`,
reading `~/.ssh/id_*`, or launching reverse shells. This is the
worst-case configuration for the entire 2025–2026 CVE class
(CVE-2025-59536, CVE-2026-21852, CVE-2026-25723, CVE-2026-33068,
CVE-2026-35020/35021/35022).

**Mitigation in code.** Permission and sandbox bypasses are off by
default. An operator who knowingly accepts the risk enables the
per-session `yolo_permissions` setting. Dux maps that setting to
`CLAUDE_AMQ_YOLO=1` or `CODEX_AMQ_YOLO=1` for wrapper providers and to
OpenCode's native `--auto` launch argument; OpenCode receives no such
argument when the setting is false. Codex hook trust review is a
separate control and remains enabled even in YOLO mode. Disabling that
review requires the explicit `CODEX_AMQ_BYPASS_HOOK_TRUST=1` opt-in in
`dux-amq/wrappers/codex-amq`. The wrappers also fail closed below the
reviewed provider-CLI floors (Claude 2.1.163, Codex 0.39.0, Gemini
0.39.1), including when a version string cannot be parsed.

**Residual risk.** Operators who set the YOLO or hook-trust-bypass env
vars globally (e.g. in `~/.bashrc`) re-create the affected part of the
original posture. Likewise,
prompt injection that targets the upstream provider's own
sandbox-bypass primitives is out of our control.

**Detection.** Each wrapper prints a warning when its dangerous opt-in
is active. The Codex warning distinguishes sandbox bypass from hook
trust review bypass so the operator can see which control was disabled.
For OpenCode, the enabled YOLO checkbox remains visible in session
settings and the PTY launch debug log includes `--auto`. An unsupported
or unknown wrapper-provider version is refused with the required minimum
in the error message.

---

## T2 — Compromised AMQ peer spoofs `--me <other>` and injects text

**Status: accepted-risk in single-user-VM mode.** The Phase 08
HMAC mitigation is preserved as opt-in but no longer the default.
The full reasoning lives below; the short version is that the
mitigation defends against an attacker that doesn't exist in dux's
declared trust model.

**Attack scenario.** Two panes share `$STATE_ROOT/amq`. Pane `bob`
is compromised (e.g. via T1). It runs
`amq send --me alice <victim> "rm -rf $HOME"`. The victim pane,
configured with `--inject-mode raw`, auto-types the payload into
its underlying CLI as if it came from `alice`. The receiver has no
way to verify the sender; AMQ wrappers
(`claude-amq`, `codex-amq`, `gemini-amq`) trust whatever
`--me` claims.

**Why the original Phase 08 HMAC mitigation does not actually defend
this surface.** Phase 08 added an HMAC-signed envelope: each
`amq send` (via `dux-amq/scripts/amq-send-signed`) reads a per-VM
secret from `$AMQ_SECRET_PATH` (default
`$HOME/.local/share/dux-amq/amq-secret`, mode 0600) and signs the
payload + a nonce. The current DUX2 format binds the sender, recipient,
UTC timestamp, 96-bit nonce, and base64 body. Receivers bind the signed
recipient to their actual handle, preserve body bytes, and use atomic
directory creation so simultaneous replay checks have one winner.
Implementation-wise this works as designed.

But the trust model in [SECURITY.md](../../SECURITY.md) explicitly
states: *"dux runs as a single-user, single-Linux-account TUI. All
panes spawned by dux share the same `$HOME`, the same filesystem
permissions, and the same environment. There is no in-VM isolation
between panes. One compromised pane = one compromised user account."*

Inside that model, every "peer" is a process running as the same
Linux user. Same-UID processes can:

- `cat $HOME/.local/share/dux-amq/amq-secret` and forge envelopes
  with valid MACs (the secret is mode 0600 by the same UID).
- `ptrace` the signing process and read the secret from memory.
- `LD_PRELOAD` the signer to substitute the body before signing.

Per Linus Torvalds's canonical position ([Debian thread, 2014](https://linux.debian.devel.narkive.com/66QPZz2A/using-sgid-binaries-to-defend-against-ld-preload-ptrace)),
*"there is a complete lack of a security boundary between processes
of the same user."* The MIT 6.828 OS-security course
([2008 lecture](https://pdos.csail.mit.edu/6.828/2008/lec/l-security.html))
makes the same point. T2's mitigation defends a boundary that
Linux itself does not enforce.

A second, concrete cost surfaced in production: enforcing strict
verification at the receiver silently dropped every legacy
`amq send` body that hadn't been wrapped through `amq-send-signed`.
Most senders don't go through the signed path — the skill teaches
plain `amq send` and the upstream AMQ binary has no signing
support — so the mitigation broke the steady-state flow without
adding a meaningful defense.

**Mitigation in code (current).**

- The bridge defaults to **skip mode**: it byte-safely decodes a
  `DUX2\t...` envelope when present, retains DUX1 compatibility for
  already queued messages, and treats plain bodies as raw. No HMAC check.
- Strict mode is opt-in via `[amq.inject].verify_envelope = true`
  in dux's `config.toml`. dux exports `DUX_AMQ_VERIFY=1` to
  spawned PTYs at bootstrap; the bridge calls
  `amq-receive-verify`; unsigned, misaddressed, replayed, stale, and
  MAC-mismatched envelopes are dropped silently. Reserved for environments that genuinely
  cross a trust boundary — proxying wakes across hosts, mixed-trust
  agents under the same UID via setuid shims, etc.
- The `amq-send-signed` and `amq-receive-verify` tooling is kept
  in the overlay and is exercised by the bats suite.
- The per-VM secret at `$AMQ_SECRET_PATH` is still initialised by
  `amq-secret-init.sh` so opt-in works out of the box. To rotate,
  `rm` the file and restart every pane.

**Residual risk.** In skip mode (the default), any peer process
running as the same Linux user can write to another peer's AMQ
inbox with a forged `--me`. This is concretely no worse than what
the same peer could already do via `ptrace`/`LD_PRELOAD`/direct
filesystem access against the signer in strict mode — the
boundary doesn't exist in either case. In strict mode, the residual
risk is the original Phase 08 risk: attackers with read access to
`$AMQ_SECRET_PATH` can still forge.

**Detection.** When strict mode is active, rejected envelopes are
written to `$AMQ_GLOBAL_ROOT/agents/<me>/.wake.log` by
`amq-receive-verify`'s stderr. dux's main JSON log records every
delivered wake under `target: "dux::amq_inject"` for the
post-bridge half of the path; the bridge itself stays silent on
the happy path so AMQ's `--inject-via` retry contract is preserved.

**Reverting accepted-risk status.** If a future deployment lands
in a context where `same-UID` does become a meaningful boundary
(e.g. a setuid-segregated multi-tenant variant of dux), this
section must be updated and `verify_envelope = true` shipped as
the default. `[amq.inject].verify_envelope` was named
deliberately so the policy flip is a one-line config change.

---

## T3 — Tampered `amq` binary `eval`'d on every shell

**Attack scenario.** `bashrc-additions.sh` runs
`eval "$($AMQ_BIN shell-init)"` on every interactive shell. If an
attacker swaps `~/.local/bin/amq` for a malicious binary, every
new shell executes attacker-controlled output as code with the
operator's permissions — silent persistence.

**Mitigation in code.** Phase 02 pins the sha256 of the trusted
binary in the bashrc fragment
(`bashrc-additions.sh:17-32`). Before the `eval`, the fragment
recomputes the binary's hash and refuses to run if the hash
doesn't match the recorded value. Phase 02 also closes the
fail-open hole from P1-E: a missing `binary.sha256` record now
fails closed with a red banner instead of silently allowing the
eval.

**Residual risk.** A root attacker can edit both the binary and
the recorded sha simultaneously. Operators on multi-user hosts
should additionally `chmod 0444` the wrapper and own it by root.

**Detection.** A mismatch prints
`AMQ binary integrity check failed — refusing to load` on every
new shell, and the same line appears in `dux.log` when `dux-amq
doctor` runs. The doctor's `amq.binary` section displays the
expected vs actual sha pair.

---

## T4 — Spot-VM preemption mid-sqlite write

**Attack scenario.** dux runs on a GCE spot VM. The VM is
preempted while `src/storage.rs` is mid-transaction on
`sessions.sqlite3`. Because the database opened with the default
rollback journal and no `synchronous=NORMAL`/WAL settings
(`storage.rs:22`), the operator returns to find a session row
attached to the empty-string project (`storage.rs:209`),
half-written agent rows, or a corrupt header. Loss of session
metadata also breaks auto-resume (T12).

**Mitigation in code.** Phase 14 sets
`PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;` on every
open and runs `PRAGMA integrity_check` once per launch. A
periodic `.backup` copies the DB to
`sessions.sqlite3.bak` so a failed `integrity_check` can be
recovered without losing more than the last backup interval.

**Residual risk.** A torn write that occurs *after* the integrity
check on launch but before the next backup still loses the
intervening transactions. Filesystem-level corruption (failing
disk) is outside the scope of WAL.

**Detection.** `dux.log` records
`sqlite integrity_check: ok` or the failing pragma output on every
launch. `dux-amq doctor` surfaces the same line and the age of the
most recent `.bak`. A failing `integrity_check` is the operator's
cue to restore from `.bak`.

---

## T5 — Plaintext API tokens / chat on persistent disk after VM destroyed

**Attack scenario.** The operator destroys a spot VM but the
attached persistent disk (`/data/state`) survives, gets snapshotted,
or is re-attached to a different VM. Chat JSONLs under
`/data/state/claude/projects/`, `/data/state/codex/`, and the
files in `~/.claude/.credentials.json` (symlinked into `/data/state/`)
contain PII and live API tokens, all in plaintext.

**Mitigation in code.** Phase 25 documents and tools an
operator-driven encryption playbook in
`docs/operations/encryption-at-rest.md`: the recommended pattern is
`gocryptfs` over `/data/state` for portable VMs and LUKS for
dedicated hosts. The playbook covers passphrase rotation and
recovery, and the installer detects the encryption posture and
warns when `$STATE_ROOT` is plaintext.

**Residual risk.** Anyone with the passphrase can decrypt. We do
not encrypt secrets at the application layer — the upstream
provider CLIs require plaintext credential files at runtime.
Snapshots taken *after* the disk is mounted-and-decrypted on a
running VM are still cleartext from the cloud's perspective.

**Detection.** `dux-amq doctor`'s `encryption` line reports
`encrypted (gocryptfs)`, `encrypted (luks)`, or `plaintext — at
risk`. The installer prints the same warning at first run.

---

## T6 — Right-to-erasure: per-customer chat history can't be deleted

**Attack scenario.** A customer requests deletion of their data
under GDPR Art. 17. The operator runs the existing
`reset_agent_data` (`src/cli.rs:464`), which removes worktrees,
sqlite, and `dux.log`. It does **not** touch
`~/.claude/projects/<encoded>/*.jsonl` or
`/data/state/{codex,gemini}/`. Every prompt and response with
potential PII survives the delete.

**Mitigation in code.** `dux session purge --hard <target>` resolves a UUID or
immutable agent handle, and accepts a branch only when it maps to exactly one
row. For isolated sessions it cascades through the managed worktree, encoded
provider-history directories, the exact-owner AMQ inbox, session-scoped log
redaction, and finally the SQLite row. Recursive targets are resolved through
symlinks and must remain strict descendants of their category root. Whole
worktree/root removal also refuses any target that is an ancestor or descendant
of a registered project; contained-file cleanup is deliberately outside that
guard. Any failed step retains the SQLite identity for retry, and bulk purge
retains rows whose full target inventory cannot be planned.

Shared sessions never remove the registered checkout. Per-session provider
history cannot be honestly attributed because sibling Dux sessions and non-Dux
conversations use the same provider directory. The default plan therefore
reports an explicit `INCOMPLETE` item and retains the row, while still erasing
provably owned AMQ and log records. The operator may explicitly accept residual
provider data, or confirm a workspace-wide purge that covers every Dux session
on that canonical path. The latter deletes the shared provider directory and
therefore also deletes non-Dux conversations stored under the same workspace.
`PURGE ALL` is not workspace-wide provider-history consent: bulk purge retains
shared provider history and its recovery rows while reporting them incomplete.

`dux config reset --all` loads config, active rows, tombstones, durable store
identity, and the complete registered-project inventory before its first
destructive step. It aborts on any incomplete/corrupt inventory, frees every
exactly-owned AMQ inbox before deleting SQLite, and never removes a shared
session's registered checkout. The error names the failed file or row; repair
or restore it and retry. If identity cannot be restored, associated AMQ and
filesystem data requires manual ownership verification before metadata removal.

**Residual risk.** Backups (sqlite `.bak`, OS-level snapshots, disk encryption
snapshots) still contain the data and must be purged out-of-band. A shared
per-session purge that accepts residual data intentionally leaves provider
transcripts and reports that fact in its purge summary.

**Detection.** Each successful purge logs `session purged
session_id=<id> files=<n> bytes=<m>` at INFO. `dux-amq doctor
--anonymize` reports the count of purges in the last 24 h, which
the operator can use as a GDPR audit trail.

---

## T7 — Cross-store or normalized wrapper identity collision

**Attack scenario.** Two DUX_HOME stores share one AMQ root and independently
allocate the same normalized handle, or a standalone wrapper/path marker
already occupies that physical `agents/<handle>` key. An unlocked
read-modify-write can also lose one writer's `config.json` registration. Either
case can redirect delivery or let one store prune another store's identity.

**Mitigation in code.** Every DUX_HOME has a durable `store-id`. Dux persists
the session UUID and handle before provider launch, then Rust and all three
wrappers use the same mandatory `flock` on `meta/config.lock` for complete
owner-marker and registry updates. The atomic marker binds `store_id` and
`session_id`; reconciliation prunes only missing/deleted rows owned by its own
store. Foreign, standalone, malformed, ownerless, and ambiguous legacy keys
are never reclaimed. Creation/backfill instead allocates a bounded `-2`,
`-3`, … suffix while the lock is held. Wrappers use AMQ's guarded
`wake recover-owner` before launch so a dead exact-owner claim cannot make a
restart permanently fail; AMQ refuses that operation while the owner is live.

**Residual risk.** This is coordination, not an authorization boundary:
same-UID code can edit the shared root or lock it indefinitely. That remains
inside the declared single-user VM threat model. A provider launched outside
an AMQ wrapper has no owner-bound wake, so deletion can reserve/remove its
registry identity but has no notifier process to manage.

**Detection.** Missing lock support and owner mismatches fail closed with an
explicit wrapper or `dux::peer` error. `amq doctor --ops` reports managed-wake
health. A recycled legacy wake PID that no longer identifies `amq wake` is
logged and left untouched.

---

## T8 — Log injection via PTY content into `dux.log`

**Attack scenario.** Producers feed unfiltered byte streams into
`logger.rs:84-92`: `String::from_utf8_lossy(&output.stderr)` from
`src/git.rs`, GitHub PR titles via `gh pr view`,
`/proc/<pid>/comm` from `pty.rs:521-525`, arbitrary user paths.
A hostile branch name, PR title, or process name with embedded
ANSI/OSC/DCS bytes lands verbatim in `dux.log`. When the operator
runs `tail dux.log` or `less dux.log`, those bytes execute as
terminal escapes: OSC 0/2 rewrites the terminal title, OSC 8
drops a covering hyperlink, OSC 52 paste-injects clipboard, DCS
sequences can corrupt subsequent rendering. Same incident class
as Rails CVE-2025-55193.

**Mitigation in code.** Phase 03 introduces
`sanitize_for_terminal(s: &str) -> String` (lives in
`src/sanitizer.rs`) which strips
`[\x00-\x08\x0b-\x1f\x7f\x1b]`. Every `logger::*` call and every
`set_error`/`set_info` status-line writer (`src/app/workers.rs`,
`src/app/sessions.rs`, `src/app/input.rs`) now routes through
the sanitizer. The 17 `git.rs` `anyhow!` sites listed in P0-C
are wrapped at the consumer side.

**Residual risk.** Bytes that escape the regex (legitimate UTF-8
that happens to look adversarial when mis-rendered) can still
confuse a viewer that interprets the file as something other than
plain text. Operators who `cat dux.log` into a tool that
re-escapes are on their own.

**Detection.** Any sanitized character logs a debug counter
`sanitizer: stripped <n> control bytes from <field>`. A spike in
that counter is the signal that something upstream is producing
hostile content. `doctor` does not currently surface this; tracked
for a future iteration.

---

## T9 — Resource exhaustion: no PTY/memory caps

**Attack scenario.** A user (or a buggy auto-resume — see T12)
spawns 100 panes. Each pane is ~1 MB of grid + ~100 MB of chat
process RSS. Within a minute the host OOMs. There is no per-pane
memory cap and no PTY-count cap.

**Mitigation in code.** Phase 16 adds a `[limits]` config block:
`max_panes` (default 32), `max_companion_terminals` (default 8),
`max_total_scrollback_mb` (default 256). The agent-creation path
(`src/app/sessions.rs::create_agent`) consults the caps and
refuses with a status-line error when exceeded. A disk watchdog
refuses new agents when free space drops below 5%.

**Residual risk.** A fork-bomb inside an existing pane (`while :;
do bash & done`) is invisible to dux's pane counter — that's an
OS-level concern. Per-pane RSS is not bounded; we count panes,
not megabytes.

**Detection.** `dux.log` records
`limits: refused new agent — max_panes reached (32)` at WARN.
`doctor` reports current pane count vs cap and free disk space.

---

## T10 — DoS via AMQ inbox flood

**Attack scenario.** A compromised pane (or a buggy script)
sends 10 000 small messages to a victim pane in a tight loop.
Each message is a small file under
`$STATE_ROOT/amq/<victim>/inbox/`. Even at a few hundred bytes
each, 10 000+ files exhaust inodes on default ext4 small-inode
filesystems and slow every directory listing.

**Mitigation in code.** Phase 16 adds a local inode-usage
watchdog: when the AMQ root exceeds 80% of available inodes the
TUI drops a status-line warning and `doctor` flags the
filesystem. Rate limiting per sender is tracked upstream with
the AMQ project; we coordinate via the shared issue tracker
referenced in Phase 16's plan.

**Residual risk.** The local watchdog is reactive, not
preventive — a fast attacker still exhausts inodes between
checks. A real fix requires upstream rate limiting in `amq`
itself.

**Detection.** `dux.log` logs
`amq: inbox <handle> reached <n> messages` at WARN every 1 000
messages. `doctor`'s `amq queue depth` and `oldest message age`
fields surface the flood.

---

## T11 — Symlink swap of `~/.claude` → attacker dir

**Attack scenario.** `~/.claude` is a symlink to
`/data/state/claude` on persistent-disk VMs. An attacker with
write access to `$HOME` (e.g. via T1) replaces the symlink with
one pointing into an attacker-controlled directory containing
forged `.credentials.json`, forged `projects/`, and a poisoned
`skills/` tree. On the next dux launch every spawned `claude`
pane reads attacker-controlled credentials and skills.

**Mitigation in code.** **Planned, not yet implemented.** The
audit lists this as `future`. The intended mitigation is a
launch-time check in `dux-amq doctor` and the dux startup path
that resolves `~/.claude` and refuses to launch (or warns
loudly) if the resolved target is not the recorded canonical
path. Until then, `SECURITY.md` documents this as a known gap.

**Residual risk.** Until the check ships, this threat is
unmitigated. Operators on shared hosts should
`chattr +i ~/.claude` after install.

**Detection.** Once shipped, `dux-amq doctor` will emit
`~/.claude symlink: <expected> → <actual>` and a red status when
they diverge.

---

## T12 — Auto-resume thundering herd on spot-VM reboot

**Attack scenario.** A spot VM is preempted with 50 active dux
sessions. On reboot, `auto_resume_all_sessions`
(`src/app/mod.rs:1380-1410`) iterates sequentially but unbounded:
all 50 sessions try to spawn PTYs and complete TLS handshakes to
the upstream API at once. The result is API rate-limit responses,
exhausted file descriptors, and OOM during the resume burst —
which itself triggers another preempt-resume cycle.

**Mitigation in code.** Phase 15 introduces a bounded scheduler:
`auto_resume_concurrency` (default 4) caps the number of
concurrent resumes via a semaphore. Sessions whose worktree mtime
exceeds `auto_resume_max_age_days` (default 14) are skipped — a
cold session is resumed lazily on operator focus instead of
during the burst.

**Residual risk.** A correctly tuned cap still spends bursts of
CPU when the user has many fresh sessions. The
`auto_resume_max_age_days` default is a heuristic; operators
running long-lived sessions may need to raise it.

**Detection.** `dux.log` records
`auto_resume: scheduling <n> sessions, concurrency=<k>` at INFO
on launch, then per-session `auto_resume: <id> started/skipped/failed`.
`doctor` shows the most recent auto-resume burst summary.

---

## T13 — Watch-rule regex evaluated on attacker-influenced PTY output

**Attack scenario.** Phase 26 introduces user-configurable watch
rules under `[[providers.<name>.watch]]` in `config.toml`. Each
rule pairs a regex against the agent's terminal output with an
action — `send_text` (Phase 1) writes bytes back into the agent's
PTY when the regex matches, and `wait_until_capture` (Phase 2)
does the same after waiting until a parsed time captured from the
matched text. Both variants ultimately write attacker-influenceable
bytes back into the agent. Two distinct abuse paths follow:

1. **Regex DoS.** An attacker (a malicious project the agent is
   editing, or an upstream prompt-injection that gets the model to
   print specific text) crafts a payload that triggers pathological
   regex behavior, freezing the UI thread on every render tick.
2. **Spurious-fire.** An attacker crafts output that *legitimately*
   matches the user's rule, causing dux to write the rule's
   `text` (e.g. `"please continue"`) back into the agent. For the
   shipped default this only resumes a Claude conversation, but a
   user with a custom rule (e.g. an "auto-yes" pattern) could be
   tricked into auto-confirming dangerous actions.

**Mitigation in code** (`src/watch/`, `src/app/mod.rs`).

- *Linear-time matching.* Rules compile via the `regex` crate's
  NFA engine, which is guaranteed linear in input length —
  catastrophic backtracking is not possible regardless of pattern
  shape.
- *Pattern complexity cap.* `RegexBuilder::size_limit(64 KiB)` and
  `dfa_size_limit(64 KiB)` reject oversized programs at load time;
  the bad rule is logged and skipped, leaving the rest of the
  config intact.
- *Rule cap per provider.* `MAX_RULES_PER_PROVIDER = 32` so a
  malicious config (or a supply-chain compromise of the canonical
  template) cannot DoS via thousands of rules.
- *Per-rule fire budget.* Every rule has a `budget.max_attempts`
  (default 5; `0` means explicitly unlimited). When exhausted, the
  rule disarms itself for the rest of the session and emits a status
  warning. Spurious-fire attacks therefore cap at `max_attempts` rule
  firings unless the operator deliberately marks that rule unlimited.
- *Cooldown between repeats.* `cooldown_ms` and the
  `baseline_match_count` dedup mean stale matches still visible in
  scrollback do not re-arm the rule. A single attacker payload
  cannot fire more than once per occurrence.
- *Opt-in defaults.* The canonical config template ships every
  example rule **commented out**. A fresh install has zero active
  watch rules; users explicitly uncomment to enable.
- *Active-pane suppression.* `App::tick_watch_engines` skips
  effects when the user is interactively typing in the matched
  session, so an auto-action does not arrive in the middle of the
  user's prompt.
- *Manual disarm.* `WatchEngine::disarm` (Phase 3 palette command)
  lets users immediately silence a rule that misfires.

**Residual risk.** A user who configures a permissive rule (e.g.
`text = "yes"`, `pattern = "Continue\\?"`) opts into the spurious-
fire risk for that rule's `max_attempts` budget, or unbounded risk if
they set `max_attempts = 0`. We document this
in the canonical-config comment block above the example. Watch
rules **never** evaluate during oneshot mode (commit-message
generation), only during interactive PTY sessions in
`SessionState::Live`.

**Detection.** `dux.log` records
`watch rule load error` at WARN whenever a rule fails to compile
(regex too big, malformed, etc.) and
`watch send_text failed` at WARN if a PTY write fails after a
rule fires. The status line surfaces every rule fire
(`watch rule "X": fired (attempt N/M)` or `N/unlimited`) and budget exhaustion
(`watch rule "X": budget exhausted; disarming`).

---

## T14 — Malicious file in `~/.local/share/dux-amq/inject-queue/` injects unauthorised text into a dux session

**Attack scenario.** dux's drainer (`crate::amq_inject` and
`crate::app::inject_runtime`) reads files from a per-receiver
queue under `~/.local/share/dux-amq/inject-queue/<receiver>/<ts>.msg`
and types each body into the matching session's PTY. An attacker
with same-UID write access to the queue dir — i.e. anyone running
as the dux operator (per the trust model T2 already concedes) —
can drop a hand-crafted `.msg` file. The drainer would type it
into whichever session matches the parent directory name, with
`\r` to submit. Concretely: drop
`inject-queue/payment-ms-engineer/666.msg` containing
`yes, run rm -rf` and the message would land in the
payment-ms-engineer session as if it had been routed through AMQ.

This is a derivative of T2 — same trust boundary — but lands in
the agent's input field rather than the AMQ inbox, so it bypasses
any agent-side filtering on AMQ message metadata.

**Mitigation in code.** The drainer rejects:

- Files larger than `[amq.inject].max_message_bytes` (default 64
  KiB). A legitimate wake notification is ~150 bytes; the cap stops
  a forged multi-megabyte body.
- Symlinks. `fs::symlink_metadata` is checked before `read_to_string`,
  so symlink swaps under TOCTOU don't escape the queue root.
- Receiver subdirectories that don't match the wrapper's
  sanitisation regex (`[a-z0-9_-]+`, no `..`, no leading dash).
  Anything else is logged at WARN and skipped.
- Inflight files left behind by a crashed prior dux instance are
  reclaimed at startup (renamed back to `.msg`); bridge-format
  `mktemp .inflight.XXXXXX` files (no `.msg` suffix) are skipped
  on purpose so a concurrent in-progress write isn't corrupted. If a
  producer has recreated the original `.msg`, the stranded inflight is
  moved to a unique `.expired/` quarantine name. Claims themselves use
  a no-replace rename, so a quarantine failure cannot turn a later scan
  into an overwrite of either body.

The bridge runs `amq-receive-verify` (HMAC + freshness + replay)
ahead of writing the queue file *only* when strict mode is opted
into (see T2). In skip mode (the default), unsigned bodies pass
through. This is consistent with T2's accepted-risk reasoning:
defending the queue against same-UID writers requires defending
the agent's PTY against same-UID writers, which is not a boundary
Linux gives us.

**Residual risk.** Same-UID code can already type into the
session's PTY directly via `ioctl(TIOCSTI)` (where supported) or
by writing to `/proc/<pid>/fd/0`, so the queue is not adding new
attack surface beyond what the OS already provides at this trust
level. The size cap and symlink check exist primarily to keep the
drainer's *own* failure modes bounded — operator error and
filesystem hiccups — rather than to harden against a hostile peer.

**Detection.** Rejections log at WARN under
`target: "dux::amq_inject"` with `path` and `reason` fields.
Successful deliveries log at INFO with a body preview. The
status line surfaces "no session matches receiver X" warnings
(rate-limited to once per minute per receiver) when a queued file
can't be routed.

---

## T15 — Tampered `agent_sessions.session_settings` blob escalates a session into autonomous mode

**Vector.** An attacker (or a buggy version of dux itself) writes a
malformed JSON value into `agent_sessions.session_settings`, or
crafts one that explicitly enables `yolo_permissions: true` /
`mode: worker` / `auto_clear_on_task_done: true` for a session the
operator never opted in. On the next dux launch — or the next time
that session re-spawns — those settings would normally drive a
provider bypass (`CLAUDE_AMQ_YOLO=1` or OpenCode's `--auto`), AMQ
postscript injection (asking the agent to emit `[task-done]`), and
the built-in auto-clear watch rule.

The attacker model is the same as T1 / T14: same-UID code with
write access to `~/.dux/sessions.sqlite3`. The novelty is that the
sqlite blob is now load-bearing for autonomous behaviour, which
makes the parser the primary attack surface.

**Mitigation in code.** Asymmetric-default policy at the parse
boundary:

- `SessionSettings::parse_or_default(raw)` (in `src/model.rs`)
  returns `Self::default()` for `None`, empty string, or any blob
  that fails `serde_json::from_str`. The fallback emits a `warn!`
  with `target: "dux::session_settings"` carrying `err` and the
  raw (sanitisable) input so post-hoc forensics can see what was
  rejected.
- `SessionSettings::default()` is the safe everything-off shape:
  `mode = Attended` (no postscript, no auto-clear),
  `yolo_permissions = false` (no CLI flag, no env var),
  `watch_rule_arm = {}` (no overrides),
  `auto_clear_on_task_done = false` (asymmetric: even Worker mode
  requires the operator to tick the box explicitly), and
  `verify_envelope_override = None` (inherit the global config
  default).
- Every consumer reads through this filter:
  `src/app/workers.rs` and `src/app/sessions.rs` call
  `session.settings.to_pty_env(...)` at PTY spawn, and
  `src/app/inject_runtime.rs::deliver_inject_body` /
  `apply_inject_postscript` consult `session.settings.mode` for the
  postscript decision. None of those paths read the raw column text
  directly.

A successful tamper that produces *valid* JSON enabling autonomous
behaviour requires the same write access an attacker already needs
to type into the PTY directly (T14 residual). The defence here is
limited to keeping the *parser* a deterministic, fail-safe
chokepoint so a malformed blob can never produce surprising
defaults — not to defending against a same-UID attacker who has
lawful access to the database.

**Residual risk.** Same-UID code with write access to
`sessions.sqlite3` can plant valid JSON enabling YOLO. The
mitigation is operator awareness via the modal (the operator can
inspect any session's settings at a glance) and the existing
T1/T14 chain. A future hardening would be SQLCipher with an
operator-derived key, currently tracked under T5
(encryption-at-rest playbook).

**Detection.** Malformed-blob rejections log at WARN under
`dux::session_settings` with the raw input. Settings-driven
decisions (env var set, postscript appended, built-in rule
attached) log at DEBUG under the same target so an operator
running with `RUST_LOG=dux::session_settings=debug` can audit
which session enabled what.

---

## T16 — Shared-workspace provider mutates the registered checkout

**Attack scenario.** Shared-workspace mode deliberately launches a provider in
the user's registered checkout rather than a Dux-owned worktree. A surprising
default change could expose an existing installation to writes it previously
expected to be isolated. If Dux later confused that checkout with a managed
worktree, automatic branch, cleanup, or link operations could also mutate the
real repository.

**Mitigation in code.** Workspace consent is represented by section presence:
an existing config with no `[workspace]` table resolves to `worktree`, while a
fresh canonical config writes `default_mode = "shared"` explicitly. A
per-project override can restore isolation. Before registration, creation, and
reconnect, shared paths are canonicalized and rejected when they resolve under
the Dux state or worktree roots. The creation modal identifies shared mode and
shows the real checkout path. Persisted `shared_workspace` state, rather than
path equality or the current project default, gates lifecycle behavior: shared
sessions set neither worktree nor branch ownership, do not create the repository
link. Shared auto-resume remains opt-in; when enabled it selects only the exact
captured provider UUID and never a latest/recency selector. A missing or invalid
UUID starts fresh and is captured before it can become resumable. Fork is an
explicit isolation boundary and always creates a worktree.

**Residual risk.** Running a provider in a real checkout grants it the same file
permissions as the operator and is the purpose of shared mode; Dux is not a
sandbox. Shared writers use one index and staging area: one writer can stage,
unstage, commit, discard, or overwrite another's changes and can switch the
branch beneath every sibling. Another `DUX_HOME` and unmanaged same-UID
processes cannot be observed reliably. Operators who need independent changes
must use worktree mode or Fork.

**Detection.** The create modal and completion status identify shared sessions,
and starting a second writer visible to the current store requires an explicit
confirmation. A persistent header warning is derived from live session state
on every render and is deliberately labeled `CURRENT STORE ONLY`; it does not
claim visibility into other stores or unmanaged processes. The database retains
`shared_workspace = 1`, and eligibility failures are shown before provider
launch. Branch and PR status are derived from the checkout's live HEAD so Dux
does not present a stale per-session branch as authoritative.

---

## T17 — Orphan cleaner removes user work

**Attack scenario.** A broad directory scan could mistake a crash residue,
user-created worktree, main checkout, or worktree still represented by a
soft-deleted session for an orphan and remove uncommitted work or its branch.

**Mitigation in code.** Cleanup is a command-palette action and never runs at
startup or when workspace mode changes. Before offering any item it loads the
complete config and protected-project inventory, durable store identity, all
session rows including tombstones, and every registered project's
machine-readable `git worktree list`; any failure aborts before removal. A
candidate must be a Git-registered non-main worktree whose canonical path is a
strict descendant of `worktrees_root` and has no matching row. Dirty/untracked
state is displayed, every item has its own confirmation, and branch deletion is
off by default. Execution revalidates the complete inventory and calls the
central protected-workspace guard before `git worktree remove`.

**Residual risk.** Under the single-user, single-UID VM model, the operator or a
same-UID process can change a worktree after inventory and before Git removes
it. The per-item warning reports the last inventoried dirty state; operators
must review it before confirmation.

**Detection.** Inventory and fail-closed errors use the
`dux::orphan_worktrees` tracing target. The modal displays the exact sanitized
path, branch, and dirty status and returns to the remaining candidate list after
each removal.

---

## T18 — Provider-history recovery assigns or copies the wrong conversation

**Attack scenario.** Startup recovery and fresh Codex capture read JSONL files
outside `$STATE_ROOT`, under `~/.claude/projects` and `~/.codex/sessions`. A
forged transcript could claim another worktree CWD so Dux associates one
agent's conversation with another. A symlink or special file could redirect a
Claude copy, an existing destination could be overwritten, or a large provider
tree could exhaust memory, CPU, or inodes. Concurrent fresh Codex launches in
one shared CWD could also race and swap their newly-created rollout UUIDs.

**Mitigation in code.** `src/resume_recovery.rs` reads provider originals and
never moves, edits, or deletes them. Recovery accepts only regular JSONLs with
valid UUIDs and an absolute recorded CWD that is exactly
`<historical-worktrees-root>/<registered-project-name>/<agent-dir>`. It checks
the session's stored project path against the registered project, normalizes the
old basename with the same immutable agent-handle rules used at creation, and
requires the provider/project/handle match to be unique. Ambiguous matches are
reported and left unmapped. Scans have file-count and per-line size bounds.

For Claude, only matched `<uuid>.jsonl` files and their matching `<uuid>/`
companion directories are copied. Sources and recursive children must be plain
files/directories; symlinks and special files fail closed. Each destination is
built under a unique temporary name, flushed, and installed with a no-replace
atomic rename, so an existing history is never overwritten and originals are
retained. For Codex, fresh capture snapshots all existing rollout UUIDs before
launch and accepts one new UUID only when its first `session_meta` record has
the expected canonical CWD. Uncaptured launches are serialized per canonical
CWD; zero or multiple candidates time out or fail closed and block another
uncaptured launch in that CWD rather than guessing. All diagnostics sanitize
provider-controlled fields under the `dux::resume_recovery` tracing target.

**Residual risk.** The documented trust model grants same-UID processes access
to both provider roots. Such a process can race filesystem names or forge one
otherwise-valid transcript during the capture window; Dux is not a security
boundary against a fully compromised Unix account. Ancestor symlink replacement
under `~/.claude` remains the broader T11 gap. The generous scan bounds limit,
but do not eliminate, startup I/O from a very large legitimate history.

**Detection.** Startup logs the number of recovered mappings, copied artifacts,
and refused candidates. Capture timeout, ambiguity, persistence failure, and
blocked-CWD events emit warnings and a status-line warning; SQLite retains the
exact provider UUID used for future resumes.

---

## T19 — Selected NTL provider can send prompts off-host or act as the operator

**Attack scenario.** An operator selects the built-in NTL provider. The
third-party CLI can send prompts and workspace context to its service, and
agent mode can request file writes or commands with the same Unix permissions
as Dux.

**Mitigation in code.** NTL is only a default configuration entry: Dux neither
installs it nor launches it until the operator selects it. Interactive sessions
invoke the official executable as `ntl --agent`; one-shot commit-message work
uses `ntl --chat --no-color -p <prompt>` so it cannot inherit agent mode from
the CLI's persisted preferences. Dux has no NTL adapter, private API access, or
credential handling, and declares no unsupported resume behavior.

**Residual risk.** Dux is not a sandbox. Once selected, NTL and its remote
service receive whatever the official CLI sends and any approved agent action
runs as the operator. NTL's binary, service, authentication, approvals, and
data handling remain upstream responsibilities.

**Detection.** NTL appears by name in the provider selector and generated
configuration. It is never selected silently; the session header shows the
active provider, and removing or overriding `[providers.ntl]` disables it.

---

## T20 — jcode provider self-updates, swarms, or reaches fleet messaging via inherited MCP

**Attack scenario.** An operator selects the built-in jcode provider. The
third-party CLI auto-updates its own binary by default on release builds,
spawns headless sub-agent swarms and a shared background `serve` daemon that
outlives individual panes, and loads the operator's user-scope MCP servers —
including fleet-messaging servers such as claude-peers — giving the agent a
path to message other agents under the operator's identity.

**Mitigation in code.** jcode is only a default configuration entry: Dux
neither installs nor launches it until selected. The `jcode-amq` wrapper fails
closed below a reviewed version floor and probes the version with
`--no-update` so the probe itself cannot swap the binary. Every wrapped launch
has `--no-update` force-injected (and `resume_by_id_args` repeats it, since
resume args replace base args), pinning the pane's binary for the PTY's
lifetime. Subcommand invocations (`run`, `usage`, `telemetry`, ...) bypass
identity and wake claims, so oneshot work cannot register or hold an AMQ
handle. Wrapped sessions use the same owner-bound co-op wake, flock-guarded
registration, and inject-bridge verification path as the other wrappers.

The inject-bridge can deliver AMQ wakes through each provider's own push
channel instead of typing into the PTY: claude panes via the claude-peers
channel (`dux peer send --transport claude-peers`, live pane resolved by
process ancestry) and jcode panes via the daemon's client protocol
(`jcode debug client:message`). This is **opt-in** (`DUX_AMQ_NATIVE_DELIVERY=1`)
and requires jcode's `display.debug_socket = true`; a stock install keeps the
file-queue/drainer path. When enabled, readiness is checked before the inbox
is drained so a held message is retried rather than lost, an unreachable
daemon falls back to the queue, and every jcode delivery attempt is logged to
`~/.local/state/inject-bridge-jcode.log`.

**Residual risk.** Dux is not a sandbox. The `serve` daemon, swarm sub-agents,
and inherited MCP servers run with the operator's Unix permissions and
identity; enabling jcode's debug socket for native delivery exposes a
same-UID control surface (message submission, session listing) that the
single-user-VM trust model already accepts under T2; killing a pane does not stop the shared daemon, and a swarm's
resource use is bounded only by jcode itself and by host-level limits. jcode's
binary, backends, authentication, and data handling remain upstream
responsibilities.

**Detection.** jcode appears by name in the provider selector, generated
configuration, and the session header. The wake watcher and `serve` daemon are
visible in the process table (`amq wake --me <handle>`, `jcode ... serve`);
removing or overriding `[providers.jcode]` disables the provider.

---

## Maintenance

When you add or change attack surface in this codebase, you must
update both `SECURITY.md` (the table) and this file (the
paragraph). PRs that touch the surface listed above without
updating these documents are blocked at review.

The IDs `T1`–`T20` are stable references; new threats append at
the end (`T21`, `T22`, …) rather than reshuffling. Retired
threats are kept in the table with a `~~strikethrough~~` and a
note pointing to the PR that retired them. Threats that move to
**accepted-risk in single-user-VM mode** keep their original ID,
get a `Status:` line at the top of their long-form section, and
remain referenced from `SECURITY.md`'s "Accepted risks" list.

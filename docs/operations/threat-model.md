# Threat Model — long-form companion

This document is the long-form companion to the STRIDE table in
[`/SECURITY.md`](../../SECURITY.md). For each row T1–T14 we capture
the concrete attack scenario, the mitigation in code (with
file:line references taken from `docs/audits/audit02.md`), the
residual risk after mitigation, and the detection mechanism — what
shows up in `dux.log` or `dux-amq doctor` output when the threat
fires.

The audit reports in `docs/audits/` are point-in-time snapshots.
This file and `SECURITY.md` are the living artifacts and must be
updated whenever new attack surface is added.

---

## T1 — Malicious repo executes via `--dangerously-skip-permissions`

**Attack scenario.** An operator clones a third-party repository
and opens a `dux` pane. The repo contains a `.claude/`
configuration, a poisoned README, or a doc string that prompt-injects
the running Claude session. Because the wrapper previously passed
`--dangerously-skip-permissions` to `claude` and
`--dangerously-bypass-approvals-and-sandbox` to `codex` by
**default**, the injected payload runs arbitrary commands inside
the operator's Linux account: exfiltrating `~/.claude/.credentials.json`,
reading `~/.ssh/id_*`, or launching reverse shells. This is the
worst-case configuration for the entire 2025–2026 CVE class
(CVE-2025-59536, CVE-2026-21852, CVE-2026-25723, CVE-2026-33068,
CVE-2026-35020/35021/35022).

**Mitigation in code.** Phase 01 inverts the default in
`dux-amq/wrappers/claude-amq:83-85` and
`dux-amq/wrappers/codex-amq:27`. The wrappers now ship without the
dangerous flag; an operator who knowingly accepts the risk opts
in via `CLAUDE_AMQ_YOLO=1` or `CODEX_AMQ_YOLO=1`.

**Residual risk.** Operators who set the YOLO env var globally
(e.g. in `~/.bashrc`) re-create the original posture. Likewise,
prompt injection that targets the upstream provider's own
sandbox-bypass primitives is out of our control.

**Detection.** `dux-amq doctor` prints the active wrapper flags;
the line `claude flags: …` will show `--dangerously-skip-permissions`
when YOLO is on. `dux.log` records every PTY spawn argv at INFO via
the sanitized logger (Phase 03), so a post-mortem `grep skip-permissions
dux.log` reveals when the dangerous flag was active.

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
payload + a monotonic nonce. Receivers verify via
`amq-receive-verify` and reject replays. Implementation-wise this
works as designed.

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

- The bridge defaults to **skip mode**: it transparently unwraps a
  `DUX1\t...` envelope when present (so legacy `amq-send-signed`
  callers still interop) and treats plain bodies as raw. No HMAC
  check.
- Strict mode is opt-in via `[amq.inject].verify_envelope = true`
  in dux's `config.toml`. dux exports `DUX_AMQ_VERIFY=1` to
  spawned PTYs at bootstrap; the bridge calls
  `amq-receive-verify`; unsigned/replayed/MAC-mismatched envelopes
  are dropped silently. Reserved for environments that genuinely
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
written to `~/.local/share/dux-amq/wake-<me>.log` by
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

**Mitigation in code.** Phase 10 ships
`dux session purge --hard <id>`. The command cascades to
`~/.claude/projects/<encoded>/<session-id>.jsonl`,
`/data/state/codex/<id>/`, `/data/state/gemini/<id>/`,
the per-pane AMQ inbox `$STATE_ROOT/amq/<branch>/`, the sqlite
session row, the worktree directory, and the `dux.log` lines tagged
with `session_id=<id>`. The last item depends on Phase 09's
migration to `tracing` for structured fields.

**Residual risk.** Backups (sqlite `.bak`, OS-level snapshots,
disk encryption snapshots) still contain the data and must be
purged out-of-band. The `purge` command logs the manual
follow-up steps.

**Detection.** Each successful purge logs `session purged
session_id=<id> files=<n> bytes=<m>` at INFO. `dux-amq doctor
--anonymize` reports the count of purges in the last 24 h, which
the operator can use as a GDPR audit trail.

---

## T7 — Wrapper identity collision (`feat/foo` ≡ `feat-foo`)

**Attack scenario.** The wrappers normalize identities with
`sed 's/[^a-z0-9_-]/-/g'` (`claude-amq:75`, `codex-amq:19`).
Branches `feat/foo` and `feat-foo` both collapse to the handle
`feat-foo`. An attacker creates a branch
`feat-bob` while a legitimate `feat/bob` already exists — or vice
versa — and AMQ messages addressed to `bob` (or to `feat/bob`'s
handle) silently land in the attacker's inbox. The recipient has
no way to tell.

**Mitigation in code.** Phase 22 makes the wrapper detect the
collision: before launch it lists existing AMQ identities and, if
the normalized handle already exists for a different branch, the
wrapper exits with `error: identity 'feat-bob' is already
registered to branch 'feat/bob' — choose a different branch
name`. The check runs in
`dux-amq/wrappers/claude-amq` and `codex-amq` before any
`amq init` or `amq wake` call.

**Residual risk.** If two branches with conflicting names are
created on different VMs and only later synced, the conflict only
surfaces when both panes start. The wrapper detects on second
launch but the first launcher has already registered.

**Detection.** Collision attempts log `wrapper: identity
collision <handle> ↔ <branch>` at ERROR. The pane refuses to
start, so the operator sees the error before any AMQ traffic
flows.

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
  (default 5). When exhausted, the rule disarms itself for the
  rest of the session and emits a status warning. Spurious-fire
  attacks therefore cap at `max_attempts` rule firings, not an
  unbounded loop.
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
fire risk for that rule's `max_attempts` budget. We document this
in the canonical-config comment block above the example. Watch
rules **never** evaluate during oneshot mode (commit-message
generation), only during interactive PTY sessions in
`SessionState::Live`.

**Detection.** `dux.log` records
`watch rule load error` at WARN whenever a rule fails to compile
(regex too big, malformed, etc.) and
`watch send_text failed` at WARN if a PTY write fails after a
rule fires. The status line surfaces every rule fire
(`watch rule "X": fired (attempt N/M)`) and budget exhaustion
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
  on purpose so a concurrent in-progress write isn't corrupted.

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

## Maintenance

When you add or change attack surface in this codebase, you must
update both `SECURITY.md` (the table) and this file (the
paragraph). PRs that touch the surface listed above without
updating these documents are blocked at review.

The IDs `T1`–`T14` are stable references; new threats append at
the end (`T15`, `T16`, …) rather than reshuffling. Retired
threats are kept in the table with a `~~strikethrough~~` and a
note pointing to the PR that retired them. Threats that move to
**accepted-risk in single-user-VM mode** keep their original ID,
get a `Status:` line at the top of their long-form section, and
remain referenced from `SECURITY.md`'s "Accepted risks" list.

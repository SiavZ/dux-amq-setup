# Security Policy

This document is the **living** security posture of `dux-amq-setup`,
the Rust TUI (`dux`), and the AMQ integration shipped under
`dux-amq/`. Audit reports under `docs/audits/` are point-in-time
snapshots; this file is what stays current.

## Reporting a vulnerability

Please email `siavash@kiani.fi` with details and, if possible, a
proof-of-concept. Do **not** open a public GitHub issue for
security-relevant findings until a fix is available. We aim to
acknowledge reports within 72 hours.

## Scope

- The `dux` TUI binary built from this repository.
- The `dux-amq` installer (`dux-amq/install.sh`),
  `bashrc-additions.sh`, and the agent wrappers
  (`claude-amq`, `codex-amq`, `gemini-amq`).
- The skills and seed material installed under
  `~/.claude/skills/` and `$STATE_ROOT/`.
- The configuration and on-disk state under
  `$STATE_ROOT` (default `/data/state` on persistent-disk VMs,
  `~/.config/dux` on Linux laptops, `~/.dux` on macOS).

Out of scope: the upstream `claude`, `codex`, `gemini`, and `amq`
binaries themselves; cloud-provider IAM; the host kernel.

## Trust model summary

`dux` runs as a **single-user, single-Linux-account** TUI. All panes
spawned by `dux` share the same `$HOME`, the same filesystem
permissions, and the same environment. There is no in-VM isolation
between panes. One compromised pane = one compromised user account.

That assumption is **load-bearing** for every other mitigation in
this document. Do not deploy `dux` in a multi-tenant context where
multiple humans share the same Linux account.

## STRIDE threat model

STRIDE (Spoofing, Tampering, Repudiation, Information disclosure,
Denial of service, Elevation of privilege) covers the threats we
actively mitigate. For long-form scenarios, mitigation pointers
into source, residual risk, and detection signals, see
[`docs/operations/threat-model.md`](docs/operations/threat-model.md).

| #   | Threat                                                                                                  | STRIDE | Asset                  | Mitigation                                                            | Phase        |
|-----|---------------------------------------------------------------------------------------------------------|--------|------------------------|-----------------------------------------------------------------------|--------------|
| T1  | Malicious repo executes through permission, sandbox, or hook-trust bypasses                              | T,E    | Host shell, API tokens | Default-deny YOLO; opt-in via `CLAUDE_AMQ_YOLO=1` / `CODEX_AMQ_YOLO=1` env propagated by `SessionSettings.yolo_permissions` (per-session, default `false`); a missing or corrupt `agent_sessions.session_settings` blob loads `SessionSettings::default()` (asymmetric default) so a tampered DB cannot escalate a session into YOLO. Codex hook trust review is preserved independently unless the operator explicitly sets `CODEX_AMQ_BYPASS_HOOK_TRUST=1`. Wrappers fail closed below reviewed provider-version floors or when the version cannot be parsed. | 01, hardened in audit03 phases 01/B/P1 |
| T2  | Compromised AMQ peer spoofs `--me <other>` and injects text                                             | S,T    | Sibling panes          | **Accepted-risk in single-user-VM mode** (see "Accepted risks" below). Strict verification is opt-in via `[amq.inject].verify_envelope = true`; DUX2 HMAC envelopes bind sender, recipient, timestamp, nonce, and byte-safe body, with atomic replay claims. The per-VM secret remains same-UID-readable, so this rail is for genuine cross-trust-boundary deployments | 08, accepted in single-user-VM rev; strict rail hardened audit03 P1 |
| T3  | Tampered `amq` binary `eval`'d on every shell                                                           | T,E    | TCB                    | sha256-pinned binary + bashrc hash guard fails closed                 | 02           |
| T4  | Spot-VM preemption mid-sqlite write                                                                     | T,D    | sessions.sqlite3       | WAL journal + integrity check + periodic `.bak`                       | 14           |
| T5  | Plaintext API tokens / chat on persistent disk after VM destroyed                                       | I      | Tokens, PII            | gocryptfs / LUKS playbook in `docs/operations/encryption-at-rest.md`  | 25           |
| T6  | Right-to-erasure: per-customer chat history can't be deleted                                            | I,N    | Chat JSONLs            | `dux session purge --hard <id>` cascades to JSONLs + AMQ + sqlite; recursive targets must resolve as strict descendants of their category roots, and the session row is retained for retry after any earlier execution failure. `purge-all` reports malformed rows, continues valid cascades, and performs an explicit row-only purge for identities whose targets cannot be safely planned | 10, hardened in audit03 phase B |
| T7  | Cross-store or normalized wrapper identity collision                                                    | S,T    | AMQ identity/inbox     | Stable per-DUX_HOME `store-id`; atomic `{store_id,session_id}` owner records; whole-root suffix allocation and wrapper/Rust reconciliation share mandatory `flock` on `meta/config.lock`; foreign/ambiguous legacy owners are preserved, never pruned or reclaimed | 22, shared-workspace phase 2 |
| T8  | Log injection via PTY content into `dux.log`                                                            | T      | Operator trust         | `sanitize_for_terminal` strips C0/C1 control bytes                    | 03           |
| T9  | Resource exhaustion: no PTY/memory caps                                                                 | D      | Host RAM/disk          | `[limits]` config (`max_panes`, scrollback) + disk watchdog           | 16           |
| T10 | DoS via AMQ inbox flood                                                                                 | D      | Filesystem             | Rate-limit (upstream coordination); local inode monitoring            | 16, upstream |
| T11 | Symlink swap of `~/.claude` → attacker dir                                                              | T,E    | API tokens             | Symlink target check on launch (planned)                              | future       |
| T12 | Auto-resume thundering herd on spot-VM reboot                                                           | D      | Host CPU, API quota    | Bounded scheduler (`auto_resume_concurrency`, default 4) + staleness skip | 15        |
| T13 | Watch-rule regex evaluated on attacker-influenced PTY output (DoS / unintended action firing)           | T,D    | UI thread, child PTY   | Linear-time `regex` crate; per-pattern `size_limit` (64 KiB); rule cap (32/provider); per-rule `budget`/`cooldown_ms` by default (`max_attempts = 0` is explicit unlimited); opt-in (commented defaults); manual disarm | 26 |
| T14 | Malicious file in `~/.local/share/dux-amq/inject-queue/` injects unauthorised text into a dux session   | T,E    | Agent input            | In opt-in strict mode the bridge verifies HMAC, recipient, freshness, and atomic replay claim before writing. In the default same-UID mode this is accepted under T2. The drainer rejects symlinks, oversized files, and invalid receiver dirs; `.unrouted` is disjoint from legal handles; claims use no-replace renames and collisions quarantine the stranded body; delivery honors `InputTarget::Agent` | this PR; hardened audit03 P0/P1 |
| T15 | Tampered `agent_sessions.session_settings` blob escalates a session into autonomous mode                | T,E    | Agent input, Host shell | Asymmetric default policy: `SessionSettings::parse_or_default()` returns `Self::default()` (Attended mode, `yolo_permissions=false`, no auto-clear, no overrides) for NULL or malformed blobs and logs a warning at `target: "dux::session_settings"`. Every consumer (PTY env, watch engine, AMQ postscript injection) reads `settings` only after this filter, so a corrupt DB row degrades to safe defaults rather than enabling autonomous behaviour. | audit03 phase 01 |
| T16 | Shared-workspace provider unexpectedly mutates the registered checkout or Dux treats it as a managed worktree | T,E | Source checkout | Legacy configs without `[workspace]` stay isolated; fresh configs state the shared default explicitly; the create modal labels shared mode and shows the real path. Shared sessions persist `shared_workspace`, never own/create/remove a worktree or branch, and are rejected under Dux-managed roots. Auto-resume is opt-in and uses only an exact captured provider UUID; it never uses a latest/recency selector. A second current-store writer requires confirmation and produces a persistent derived warning; Fork always isolates. | shared-workspace phases 4/6 + exact-resume |
| T17 | Orphan cleaner mistakes user data or a retained session worktree for disposable residue | T,D | Worktrees, branches | Cleaner is palette-only and never automatic; inventory fails closed across config/store identity/all rows including tombstones/every project's `git worktree list`; candidates must be strict canonical descendants of the managed root, exclude the main checkout, show dirty state, require per-item confirmation, preserve branches by default, and re-enter the protected-workspace guard at removal. | shared-workspace phase 6 |
| T18 | Forged or raced provider history causes cross-agent conversation assignment, clobbering, or recovery-scan exhaustion | S,T,I,D | Claude/Codex histories, agent identity | Recovery accepts regular JSONLs with valid provider UUIDs and an absolute recorded CWD exactly two components below Dux's historical worktree root; it independently matches registered project ownership plus the immutable normalized agent handle and refuses non-unique matches. Claude copies only matched JSONLs/companion dirs, rejects symlinks and special files, retains originals, and installs fsynced temporary copies with no-replace atomic renames. Codex fresh capture snapshots existing UUIDs, matches canonical CWD, serializes uncaptured launches per CWD, and blocks after timeout/ambiguity rather than guessing. File-count and JSON-line-size bounds cap scans. | exact-resume |

## What's in scope vs. accepted

**In scope** — we mitigate or document:

- Single-user, single-VM compromise via prompt injection.
- Supply-chain integrity of `dux`, `amq`, and the skills package
  (sha256 pins, GitHub Actions pinned by SHA, SBOM, attestation).
- Local data-at-rest exposure on persistent disks that may outlive
  the VM (operator-driven encryption playbook).
- AMQ peer-spoofing within a single VM.
- Right-to-erasure for chat history that may contain PII.

**Accepted risks** — not mitigated, with rationale:

- **Multi-tenant isolation.** dux is a single-user-on-a-VM product.
  Two humans sharing one Linux account is out of scope.
- **Cloud-provider IAM compromise.** If your GCP/AWS service
  account is hijacked, the attacker controls the disk and the VM
  before any dux-level mitigation can help.
- **Side-channel leakage from upstream CLIs.** Bugs in `claude`,
  `codex`, or `gemini` themselves (e.g. the CVE-2025/26 series) are
  the upstream vendor's responsibility. Wrappers enforce reviewed
  minimum-safe versions, but operators own upgrades above those floors;
  dux does not re-implement provider sandboxing.
- **TIOCSTI on legacy kernels.** Mitigation requires
  `dev.tty.legacy_tiocsti=0`; we document the sysctl in Phase 13
  but cannot guarantee its presence on every host kernel.
- **AMQ peer spoofing within the same Linux user account (T2,
  reclassified).** Originally Phase 08 mitigated this with an
  HMAC envelope and a per-VM secret at
  `~/.local/share/dux-amq/amq-secret` (mode 0600). On reflection
  the mitigation does not match the trust model declared above:
  every "peer" is a process running as the same Linux user with
  full access to `$HOME`, including the secret file. A peer can
  `cat` the secret directly, `ptrace` the signer, or `LD_PRELOAD`
  into it — none of which the HMAC check defends against. Per
  Linus Torvalds ("there is a complete lack of a security
  boundary between processes of the same user") and the MIT
  6.828 OS-security course, same-UID is not a defensible
  boundary on Linux. We've therefore made strict verification
  **opt-in** (`[amq.inject].verify_envelope = true`) and left it
  off by default. Operators that genuinely cross a trust boundary
  with their wake notifications — proxying across hosts, or
  running mixed-trust agents under the same UID via setuid
  shims — should flip the switch and accept the legacy-message
  drop semantics. The HMAC tooling
  (`amq-send-signed`/`amq-receive-verify`) remains in the
  overlay and is exercised by the bats suite.

## Verification

The project ships a self-check tool: `dux-amq doctor` (Phase 20).
Run it after every install or upgrade:

```bash
dux-amq doctor | grep -E '(integrity|tiocsti|amq.binary|encryption)'
```

`doctor` reports:

- `binary integrity` — sha256 of `~/.local/bin/amq` against the
  recorded pin in `bashrc-additions.sh`.
- `tiocsti` — value of `dev.tty.legacy_tiocsti`.
- `sqlite integrity_check` — `PRAGMA integrity_check` on
  `sessions.sqlite3`.
- `encryption at rest` — whether the configured `$STATE_ROOT` is on
  an encrypted mount.
- AMQ queue depth, oldest-message age, `~/.claude` symlink target,
  free disk space, currently-running dux PID/uptime/RSS.

Pass `--anonymize` to redact paths and identities before sharing
the output for support.

## Update cadence

This file is **a living document**. Every audit
(`docs/audits/audit01.md`, `audit02.md`, …) must extend the STRIDE
table with new threats discovered, and stale rows must be either
re-validated or removed in the same PR that supersedes them.

When introducing a new attack surface — a new MCP integration, a
new network egress, a new file write outside `$STATE_ROOT`, a new
provider CLI — update the STRIDE table **in the same PR** that
introduces the surface. PRs that add attack surface without
updating this file are blocked at review.

A future CI check (`scripts/validate-threat-model.sh`) is planned
to compare the phase references in this table against the phase
files under `docs/plans/audits/` and warn when a phase claims to
mitigate a threat that is not listed here. Tracked as a
post-audit02 follow-up.

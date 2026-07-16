# Plan: Per-agent conversation resume in shared-workspace mode
_Frozen spec — derived from Codex's read-only design pass (2026-07-14)._

## Goal
In shared-workspace mode, N dux agents share one CWD (the project checkout). Provider resume selectors (`claude --continue`, `codex resume --last`) pick a conversation by **recency/CWD**, not by dux agent identity, so shared agents cannot resume their own prior conversation — today `should_resume_session` deliberately returns `false` for shared sessions, so they always launch fresh, and each agent's real history is stranded under its **old per-worktree** encoded project dir. Fix: give every agent a durable **provider→session-UUID** mapping, resume shared sessions **only** by that exact UUID (never a latest/recency selector), capture the UUID at launch, and one-time-migrate the already-stranded histories so existing agents resume their real conversations. Per-worktree behavior is unchanged.

## Approach

### 1. Persist provider→session UUID (schema migration 0006)
- Add field to `AgentSession` (`src/model.rs`): `provider_session_ids: BTreeMap<String, String>` (key = provider name e.g. `"claude"`/`"codex"`, value = opaque provider session UUID). A map because `started_providers` already lets one agent switch providers.
- Migration `0006`: `ALTER TABLE agent_sessions ADD COLUMN provider_session_ids TEXT NOT NULL DEFAULT '{}';` — follow the crash-atomic migration pattern already used in `src/storage.rs`. Serialize the map as JSON exactly like `started_providers`/`session_settings`. Thread the new field through every `AgentSession` constructor, the upsert, the load, and all test fixtures. Bump schema version 5→6.
- SQLite is the single source of truth; no sidecar file is authoritative (a short-lived capture temp file is fine internally).

### 2. Explicit launch intent (replace the resume boolean)
- Introduce `enum SessionLaunch { Fresh, LegacyLatest, ResumeId(String) }`.
- Selection rules:
  1. UUID exists for this provider **and** provider has targeted-resume configured → `ResumeId(uuid)`.
  2. Shared session, no UUID → `Fresh` (NEVER a latest selector).
  3. Per-worktree, no UUID, legacy resume supported → `LegacyLatest` (current `--continue`/`resume --last`).
  4. Otherwise → `Fresh`.
- Replace the `resume: bool` param threaded through `should_resume_session`, `spawn_pty_for_session`, `spawn_pty_for_auto_resume`, `continue_reconnect`, and the `AutoResumeSpawnOnMain` path with `SessionLaunch`.

### 3. Config: targeted resume-by-id args
- Add optional `resume_by_id_args: Option<Vec<String>>` to the provider config (`src/config.rs`), rendered in the canonical config with comments.
- Token substitution: replace the exact literal token `{session_id}` (no shell expansion, no other tokens) with the UUID when building argv.
- Defaults in the canonical config:
  - Claude: `["--settings", '{"ultracode":true}', "--effort", "high", "--resume", "{session_id}"]` (preserve all non-selector flags).
  - Codex: `["resume", "{session_id}"]`.
- Keep existing `resume_args` as the legacy per-worktree fallback (`LegacyLatest`). Do NOT add Claude `--fork-session` to the targeted path (it mints a new identity).

### 4. Capture the UUID at launch (provider-specific, immediate — not at detach)
- **Claude**: generate a fresh UUID in dux before a `Fresh` launch, persist it into `provider_session_ids["claude"]`, and pass `--session-id <uuid>` in the launch argv (Claude Code supports `--session-id`). This removes all inference for future sessions.
- **Codex**: no fresh-session-id flag exists. Snapshot the set of known `~/.codex/sessions/**/rollout-*.jsonl` UUIDs before launch, then after launch poll for the newly-created rollout whose first `session_meta` record has the expected canonical `cwd` and whose UUID was absent from the snapshot; persist `payload.id` immediately. **Serialize fresh/uncaptured Codex launches per canonical CWD** until each new JSONL is identified (targeted resumes may stay concurrent). On capture timeout or multiple candidates: do not guess, do not launch another uncaptured Codex agent in that CWD until resolved.
- Do NOT rely on `DUX_*` env vars as transcript markers (providers don't write them into transcripts); do NOT inject fake prompt markers.

### 5. Resume by UUID
- Claude: `claude … --resume <uuid>`. Claude's installed `--resume` still locates the transcript via the **current encoded project dir** (with a fallback across *currently registered* git worktrees — removed legacy worktrees are NOT reliably discoverable). Therefore the target JSONL **must be present in the shared CWD's encoded Claude project dir** — see step 6 migration.
- Codex: `codex resume <uuid>` — Codex sessions are global, no file move needed.
- Targeted UUID selection makes N agents in one CWD safe: recency no longer participates.

### 6. One-time recovery backfill for stranded histories (application-level, idempotent)
- Runs **before** shared auto-resume scheduling on startup; idempotent (skip any `(session, provider)` that already has a UUID). The SQL migration only adds the column — recovery is code, not SQL.
- For every `(agent_session, started_provider)` missing a UUID:
  1. Scan provider JSONLs and read the recorded original `cwd` from the transcript — do NOT reverse Claude's lossy encoded dir name.
  2. Accept only old paths under dux's historical worktree root.
  3. Normalize the old CWD basename with the **same agent-handle rules** and match to the immutable `agent_handle`.
  4. Independently verify project ownership (old worktree's project parent / registered project path / git origin).
  5. If the handle/project match is **not unique**, leave unmapped and report candidates — never guess.
- **Claude** matched agents: group old JSONLs by agent; copy all matched JSONLs **plus each matching `<uuid>/` companion dir** (tool results) into the shared project's encoded dir using **no-clobber/atomic** copies, **retaining originals**; do not copy unrelated project memory/metadata; persist the UUID of the conversation with the latest valid event timestamp (fallback to file mtime).
- **Codex** matched agents: read each rollout's `session_meta.payload.{id,cwd}`, match old unique worktree CWD → agent, persist most-recently-active matching UUID; copy no files.
- Only recover records whose stored CWD is an old per-agent worktree. A fresh session already created in the shared CWD is NOT retroactively assignable among N agents.

### 7. Failure handling & back-compat
- Per-worktree sessions with no captured UUID keep legacy `--continue`/`resume --last`.
- Shared sessions **never** fall back to a latest selector; missing/invalid UUID → `Fresh` then capture.
- Preserve `resume_wait_timeout_ms`: a targeted resume producing no visible output within the timeout is killed and retried **fresh once**, exactly like today.
- Do not erase an old UUID until a fresh replacement UUID is captured.
- Providers without `resume_by_id_args` retain current per-worktree behavior; shared sessions start fresh with a clear status-line warning.

## Key decisions & tradeoffs
- **UUID map on the session row** (not a sidecar): one durable lifecycle, matches `started_providers`.
- **Claude `--session-id` (proactive) vs Codex poll-and-match (reactive)**: Claude supports assigning the id; Codex does not, so its capture is inherently a serialized filesystem race we must bound.
- **Copy, don't move, Claude histories**: originals retained so per-worktree/legacy paths and rollback stay intact.
- **Refuse ambiguous recovery**: report candidates rather than mis-assign a conversation to the wrong agent.

## Risks / open questions
- Exact `~/.codex/sessions` rollout layout and `session_meta` shape — Codex must confirm from the installed CLI / existing wrapper code, not assume.
- Claude `--session-id` / `--resume` flag names must be verified against the installed `claude --help` before wiring defaults.
- The recovery scan touches `~/.claude/projects` and `~/.codex/sessions` — must be strictly read + no-clobber-copy, never mutate/delete originals.

## Out of scope
- Changing the shared-workspace model itself (still one CWD per project).
- Claude `--fork-session` semantics.
- Any provider beyond claude/codex getting targeted resume (gemini/opencode keep current behavior).
- UI redesign; only a status-line warning string is added where noted.

## Proof
`cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test`
Plus new tests: v5→v6 migration, provider-map JSON round-trip, two shared agents yielding different UUID argv in one CWD, shared mode never emitting a latest selector, ambiguous-recovery refusal, Claude artifact copy, Codex capture serialization, targeted-resume-timeout→fresh fallback.

# Audit03 Phase 01 — Session settings modal

A per-session settings modal that consolidates every per-agent knob
(YOLO permissions, context mode, watch-rule arm/disarm, auto-clear,
AMQ verify) into one keybinding-accessible surface, persisted to
sqlite, with a strict "asymmetric default" policy: anything that
could disrupt operator-managed work must be opt-in per session.

This document is the **implementing LLM's working spec**. Walk
through phases in order. Each phase has a verification gate; do not
proceed until the gate is green. The document is designed to be
checkpointable — you can resume at any phase, identify the first
failing gate, and continue from there.

---

## 0 · Status, goal, and non-goals

**Status**: implemented and landed. Phases 1–7 split as in §10.

**Verification gate summary** (run at the tip of the implementing
branch):

- **2.A** (storage migration test): `tests/storage_migrations.rs::migrate_v2_to_v3_adds_session_settings_column` passes.
- **3.A** (round-trip): `tests/storage_integration.rs` covers
  default round-trip, full-payload round-trip, NULL→default, and
  malformed-blob→default.
- **4.A** (modal save-and-apply): `src/app/sessions.rs` test
  module covers open/save/no-changes-summary/focus-navigation.
- **5.A** (per-session env): `tests/pty_integration.rs::spawn_with_env_propagates_per_session_vars`
  + `spawn_with_env_falls_back_to_global_verify_envelope` pass with
  a real shell child.
- **6.A** (auto-clear E2E):
  `tests/watch_engine_integration.rs::auto_clear_rule_fires_on_task_done_sentinel_through_pty`
  + `auto_clear_rule_provider_clear_command_dispatch` pass.
- **7.A** (binding wired): `Action::SessionSettings` registered
  with default `Ctrl-Shift-S`, scoped Global, palette name
  `session-settings`. Help-entry coverage test passes.

Global gates (`cargo fmt --check`, `cargo clippy --all-targets
--all-features -- -D warnings`, `cargo test --quiet`, `bats
dux-amq/tests/`) all green.

**Goal**: a single `PromptState::SessionSettings` modal that lets
the operator toggle per-session settings, persisted in
`agent_sessions.session_settings` (new JSON column). All
operator-disturbing automation (auto-clear context, YOLO, AMQ
verify) keys off this storage.

**Non-goals (this phase)**:
- Inferring mode/intent from agent behaviour. The decision is
  always explicit, set by the operator.
- Per-message-thread overrides. Per-session is sufficient for v1.
- Cross-provider semantics for non-Claude CLIs beyond Codex/Gemini.
- A "config presets" library. v1 ships with hard-coded defaults.
- Touching `claude-amq` / `codex-amq` / `gemini-amq` wrappers
  beyond confirming they read the env vars dux now sets per-PTY.

**Production posture**: every phase has tests, every settings
consumer has a default-deny fallback when sqlite is unavailable
or the JSON blob fails to parse. No unsafe global state. Schema
migrations are additive and forward-compatible.

---

## 1 · Surface (operator-facing UX)

### 1.1 Keybinding

- New `Action::SessionSettings`, palette name `session-settings`,
  default keybinding **`Ctrl-Shift-S`** (verified free in
  `src/keybindings.rs` BINDING_DEFS).
- Available scope: `BindingScope::Global` when a session is
  selected in the left pane. Disabled when no session is selected
  (status line: "Select a session first").

### 1.2 Modal layout (ratatui)

```
╭─ Session settings — front-end-qa ─────────────────────╮
│                                                        │
│  Title                                                 │
│  ┌──────────────────────────────────────────────────┐  │
│  │ front-end-qa-orchestrator                        │  │
│  └──────────────────────────────────────────────────┘  │
│                                                        │
│  Context mode                                          │
│    ( ) attended       — operator-managed, never cleared│
│    ( ) orchestrator   — coordinates peers, persists    │
│    ( ) worker         — stateless, auto-clears on done │
│                                                        │
│  Permissions                                           │
│    [ ] YOLO (--dangerously-skip-permissions)           │
│         (needs respawn)                                │
│                                                        │
│  Watch rules                                           │
│    [x] Auto-resume on rate-limit         (live)        │
│    [x] Auto-resume on 5-hour limit       (live)        │
│    [x] Nudge on "tomorrow" deferral      (live)        │
│    [ ] Auto-clear after task done        (live, worker)│
│                                                        │
│  AMQ inject verify_envelope                            │
│    (•) default (config global)                         │
│    ( ) strict (force HMAC)                             │
│    ( ) skip (force unsigned)                           │
│         (needs respawn)                                │
│                                                        │
├────────────────────────────────────────────────────────┤
│ ↑↓ navigate  Space toggle  Tab next  Enter save  Esc cancel │
╰────────────────────────────────────────────────────────╯
```

### 1.3 Apply-timing labels (UI inline tags)

- `(live)` — applied on next tick. Mode, watch-rule arm, auto-clear.
- `(needs respawn)` — applied on next session spawn. YOLO, AMQ
  verify. Modal labels these explicitly so operator isn't surprised.

### 1.4 Save semantics

- `Enter` saves all changes atomically: in-memory `AgentSession`
  update + immediate `session_store.upsert_session()` call.
- `Esc` discards. No partial saves.
- Save is **synchronous**; any sqlite error surfaces in the status
  line and the modal stays open so the operator can retry.

### 1.5 Existing rename/disarm consolidation

- The standalone rename modal stays, but the new modal also exposes
  the title field at the top.
- The standalone watch-rules palette modal stays (it lists rules
  across all sessions); per-session arm/disarm in the new modal is
  a per-session view of the same state.

---

## 2 · Storage schema

### 2.1 Migration `0003_session_settings.sql`

**File**: `src/storage/migrations/0003_session_settings.sql`

```sql
-- Audit03 Phase 01: per-session settings blob.
-- Nullable; readers fall back to SessionSettings::default() when NULL.
-- Additive only — old rows continue to work with NULL.
alter table agent_sessions add column session_settings text;
```

### 2.2 Migration registration

**File**: `src/storage.rs`

Append to the `MIGRATIONS` slice (find the existing pair `(2,
include_str!("storage/migrations/0002_session_state_v2.sql"))`):

```rust
const MIGRATIONS: &[(u32, &str)] = &[
    (1, include_str!("storage/migrations/0001_initial_schema.sql")),
    (2, include_str!("storage/migrations/0002_session_state_v2.sql")),
    (3, include_str!("storage/migrations/0003_session_settings.sql")),
];
```

### 2.3 Migration test

**File**: `tests/storage_migrations.rs`

Add:

```rust
#[test]
fn migrate_v2_to_v3_adds_session_settings_column() {
    // Run migrations 1+2 only against fresh DB.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.sqlite3");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(include_str!(
            "../src/storage/migrations/0001_initial_schema.sql"
        ))
        .unwrap();
        conn.execute_batch(include_str!(
            "../src/storage/migrations/0002_session_state_v2.sql"
        ))
        .unwrap();
        conn.pragma_update(None, "user_version", 2).unwrap();
    }
    // Open via SessionStore — runs remaining migrations.
    let _store = SessionStore::open(&path).unwrap();
    // Confirm the new column exists and is nullable.
    let conn = rusqlite::Connection::open(&path).unwrap();
    let cols: Vec<String> = conn
        .prepare("pragma table_info(agent_sessions)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert!(cols.contains(&"session_settings".to_string()));
    let user_version: u32 = conn
        .query_row("pragma user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(user_version, 3);
}
```

**Verification gate 2.A**: `cargo test --test storage_migrations` green
including the new test. Existing tests still pass.

---

## 3 · `SessionSettings` Rust type

**File**: `src/model.rs` (alongside `AgentSession`).

```rust
/// Per-session settings, persisted as JSON in
/// `agent_sessions.session_settings`. Designed for forward
/// compatibility: unknown fields are ignored on deserialize, missing
/// fields use defaults. Adding a new knob does NOT require a schema
/// migration — only this struct grows.
///
/// All fields default to "do nothing" / "operator-managed" semantics.
/// The asymmetric-risk policy: an empty/missing/corrupt blob must
/// never enable autonomous behaviour that could disrupt the operator.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields = false)]
pub struct SessionSettings {
    /// Context mode. Drives auto-clear policy and AMQ sentinel
    /// injection. See `ContextMode` for the variants and asymmetric
    /// defaults.
    pub mode: ContextMode,

    /// `--dangerously-skip-permissions` (claude) /
    /// `--sandbox-bypass` (codex) for this session. When true, dux
    /// sets `CLAUDE_AMQ_YOLO=1` (and codex equivalent) in the PTY
    /// child env at spawn time; the wrappers translate that into the
    /// CLI flag. Default `false`.
    pub yolo_permissions: bool,

    /// Per-rule arm/disarm overrides keyed by rule index in the
    /// provider's `[providers.<X>.watch]` array. Absence = use the
    /// rule's config-default arm state (always armed today).
    /// Persisted so disarm survives restart.
    pub watch_rule_arm: HashMap<usize, bool>,

    /// Built-in auto-clear-after-task-done rule, only meaningful for
    /// `mode == ContextMode::Worker`. Default `false` even for
    /// workers; operator opts in explicitly.
    pub auto_clear_on_task_done: bool,

    /// Per-session override for `[amq.inject].verify_envelope`. None
    /// = inherit global. Some(true) = strict for this session.
    /// Some(false) = skip for this session.
    pub verify_envelope_override: Option<bool>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContextMode {
    /// Operator-managed session. **Never** auto-cleared, never sees
    /// the task-done sentinel injected into AMQ wakes. The default.
    #[default]
    Attended,
    /// Coordinator that talks to peers. Persistent context. Like
    /// Attended for clearing semantics, but reserved as a label for
    /// future per-mode behaviours (e.g. orchestrator-only watch
    /// rules).
    Orchestrator,
    /// Stateless processor. Receives task instructions via AMQ,
    /// emits a sentinel when done, gets context cleared. The bridge
    /// appends a sentinel-required postscript to AMQ wakes for this
    /// session.
    Worker,
}

impl SessionSettings {
    /// Parse from the sqlite `session_settings` column value.
    /// `None` → returns `Self::default()`. Malformed JSON → logs
    /// warning and returns `Self::default()` (asymmetric-fail-safe).
    pub fn parse_or_default(raw: Option<&str>) -> Self {
        let Some(raw) = raw else { return Self::default() };
        match serde_json::from_str(raw) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(
                    target: "dux::session_settings",
                    err = %err,
                    raw = %raw,
                    "session_settings JSON malformed; falling back to default",
                );
                Self::default()
            }
        }
    }

    /// Serialise for storage. Always returns valid JSON.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("SessionSettings serialises")
    }
}
```

**`AgentSession` extension** (same file):

```rust
pub struct AgentSession {
    // ... existing fields ...
    pub settings: SessionSettings,
}
```

The `settings` field is reconstructed from `session_settings` at row
load time and serialised back on every `upsert_session()`.

**File**: `src/storage.rs` — load/save paths

The existing `upsert_session` and row-load functions need a new
column. Pattern:

- On INSERT/UPDATE: bind `session.settings.to_json()` to the
  `session_settings` parameter.
- On SELECT: read the optional column, call
  `SessionSettings::parse_or_default(...)`, attach to the
  `AgentSession`.

**Verification gate 3.A**: round-trip test in
`tests/storage_integration.rs`:

```rust
#[test]
fn session_settings_round_trip_through_sqlite() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::open(&dir.path().join("t.sqlite3")).unwrap();
    let mut session = make_session("s1", "claude", "/tmp/wt");
    session.settings = SessionSettings {
        mode: ContextMode::Worker,
        yolo_permissions: true,
        watch_rule_arm: [(0usize, false), (2, true)].into_iter().collect(),
        auto_clear_on_task_done: true,
        verify_envelope_override: Some(true),
    };
    store.upsert_session(&session).unwrap();
    let loaded = store.load_sessions().unwrap();
    let s = loaded.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(s.settings, session.settings);
}

#[test]
fn session_settings_default_when_column_null() {
    // Insert a row directly with NULL session_settings, verify load
    // returns SessionSettings::default().
}

#[test]
fn session_settings_default_when_blob_malformed() {
    // Insert a row with garbage JSON, verify load returns default
    // and a warning is logged (use a `tracing` test subscriber).
}
```

---

## 4 · `PromptState::SessionSettings` modal

### 4.1 New variant

**File**: `src/app/mod.rs`, near other `PromptState` variants:

```rust
SessionSettings(SessionSettingsPrompt),
```

```rust
/// State for the per-session settings modal. Built from the live
/// `AgentSession.settings` when the modal opens; the modal mutates
/// a copy and only writes back on save (Enter).
pub(crate) struct SessionSettingsPrompt {
    pub session_id: String,
    pub session_label: String,
    pub provider: ProviderKind,
    pub draft: SessionSettings,
    pub draft_title: TextInput,
    pub focus: SettingsFocus,
    /// Static metadata about the watch rules available for this
    /// session's provider — populated once from the runtime engine
    /// (or, when no engine is attached, from the provider config).
    pub rules: Vec<WatchRuleSummary>,
}

#[derive(Clone, Debug)]
pub(crate) struct WatchRuleSummary {
    pub idx: usize,
    pub label: String,
    pub config_armed_default: bool,
}

/// Cursor position within the modal. `Tab`/`Shift-Tab` cycle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SettingsFocus {
    Title,
    ModeAttended,
    ModeOrchestrator,
    ModeWorker,
    Yolo,
    WatchRule(usize),
    AutoClearOnDone,
    VerifyDefault,
    VerifyStrict,
    VerifySkip,
    SaveButton,
    CancelButton,
}
```

### 4.2 Open trigger

**File**: `src/app/sessions.rs` (add method to existing `impl App`):

```rust
pub(crate) fn open_session_settings(&mut self) -> Result<()> {
    let Some(session) = self.selected_session().cloned() else {
        self.set_warning("Select a session first.");
        return Ok(());
    };
    let rules = self.collect_watch_rule_summaries(&session);
    self.ui.input_target = InputTarget::None;
    self.ui.fullscreen_overlay = FullscreenOverlay::None;
    self.ui.prompt = PromptState::SessionSettings(SessionSettingsPrompt {
        session_id: session.id.clone(),
        session_label: session
            .title
            .clone()
            .unwrap_or_else(|| session.branch_name.clone()),
        provider: ProviderKind::from_str(&session.provider).unwrap_or_default(),
        draft: session.settings.clone(),
        draft_title: TextInput::with_text(
            session
                .title
                .clone()
                .unwrap_or_else(|| session.branch_name.clone()),
        )
        .with_char_map(crate::git::agent_name_char_map),
        focus: SettingsFocus::Title,
        rules,
    });
    Ok(())
}

fn collect_watch_rule_summaries(&self, session: &AgentSession) -> Vec<WatchRuleSummary> {
    // Prefer the live engine's snapshot when attached; fall back to
    // provider config for sessions whose engine isn't loaded yet
    // (detached, etc.).
    if let Some(engine) = self.runtime.watch_engines.get(&session.id) {
        return engine
            .rules_snapshot()
            .into_iter()
            .map(|s| WatchRuleSummary {
                idx: s.idx,
                label: s.label,
                config_armed_default: !s.is_disarmed_in_config,
            })
            .collect();
    }
    self.config
        .providers
        .commands
        .get(&session.provider)
        .map(|cfg| {
            cfg.watch
                .iter()
                .enumerate()
                .map(|(idx, rule)| WatchRuleSummary {
                    idx,
                    label: rule.label.clone().unwrap_or_else(|| format!("rule {idx}")),
                    config_armed_default: true,
                })
                .collect()
        })
        .unwrap_or_default()
}
```

### 4.3 Input handler

**File**: `src/app/input.rs`, near the existing `PromptState::WatchRules` block:

```rust
if let PromptState::SessionSettings(prompt) = &mut self.ui.prompt {
    let action = palette_action.or(dialog_action).or(global_action);

    // Title text input gets all keys when focused.
    if prompt.focus == SettingsFocus::Title {
        match action {
            Some(Action::CloseOverlay) => {
                self.ui.prompt = PromptState::None;
                return Ok(false);
            }
            Some(Action::Confirm) => {
                self.save_session_settings()?;
                return Ok(false);
            }
            Some(Action::FocusNext) => {
                prompt.focus = SettingsFocus::ModeAttended;
                return Ok(false);
            }
            _ => {
                prompt.draft_title.handle_key(key);
                return Ok(false);
            }
        }
    }

    match action {
        Some(Action::CloseOverlay) => {
            self.ui.prompt = PromptState::None;
        }
        Some(Action::Confirm) => {
            // Per the CLAUDE.md "Space activates focused button" tenet,
            // both Enter and Space confirm/toggle the focused row.
            self.toggle_or_save_focused_setting()?;
        }
        Some(Action::FocusNext) => {
            prompt.focus = next_focus(prompt.focus, prompt.rules.len());
        }
        Some(Action::FocusPrev) => {
            prompt.focus = prev_focus(prompt.focus, prompt.rules.len());
        }
        Some(Action::MoveDown) | Some(Action::MoveUp) => {
            prompt.focus = if matches!(action, Some(Action::MoveDown)) {
                next_focus(prompt.focus, prompt.rules.len())
            } else {
                prev_focus(prompt.focus, prompt.rules.len())
            };
        }
        _ => {}
    }
    return Ok(false);
}
```

`toggle_or_save_focused_setting` mutates `prompt.draft` based on
`prompt.focus`. For radio groups (Mode, Verify), set the chosen
variant. For checkboxes (YOLO, watch rules, AutoClearOnDone), flip
the bool. For SaveButton, call `save_session_settings()`. For
CancelButton, set `prompt = None`.

### 4.4 Renderer

**File**: `src/app/render.rs`, new function `render_session_settings`,
called from the prompt-dispatch site near `render_watch_rules`.

Use the same modal frame conventions: centered, bordered `Block`,
inner `Layout::default().direction(Direction::Vertical).constraints(...)`.
Each row a `Paragraph` or styled `Line`. Focused row uses
`theme.modal_focused()` style (define in `src/theme.rs` if not
already there).

Footer hint bar:
```
↑↓ navigate  Space/Enter toggle  Tab next field  Ctrl-S save  Esc cancel
```

### 4.5 Save action

**File**: `src/app/sessions.rs`:

```rust
pub(crate) fn save_session_settings(&mut self) -> Result<()> {
    let PromptState::SessionSettings(prompt) = &self.ui.prompt else {
        return Ok(());
    };
    let session_id = prompt.session_id.clone();
    let new_settings = prompt.draft.clone();
    let new_title_raw = prompt.draft_title.text().trim().to_string();
    let new_title = (!new_title_raw.is_empty()).then_some(new_title_raw);

    let Some(session) = self
        .git
        .sessions
        .iter_mut()
        .find(|s| s.id == session_id)
    else {
        self.set_error("Session disappeared while editing settings.");
        self.ui.prompt = PromptState::None;
        return Ok(());
    };

    // Detect what changed for status line + apply-timing summary.
    let mode_changed = session.settings.mode != new_settings.mode;
    let yolo_changed = session.settings.yolo_permissions
        != new_settings.yolo_permissions;
    let verify_changed = session.settings.verify_envelope_override
        != new_settings.verify_envelope_override;
    let title_changed = session.title != new_title;

    session.settings = new_settings.clone();
    if title_changed {
        session.title = new_title;
    }
    session.updated_at = Utc::now();

    if let Err(err) = self.session_store.upsert_session(session) {
        self.set_error(format!("Failed to save settings: {err}"));
        return Ok(());
    }

    // Apply live changes immediately.
    self.apply_session_settings_to_runtime(&session_id, &new_settings);

    // Build the status-line summary.
    let needs_respawn = yolo_changed || verify_changed;
    let summary = build_save_summary(mode_changed, title_changed, needs_respawn);
    if needs_respawn {
        self.set_warning(format!(
            "{summary} Detach + relaunch this session for spawn-time settings to apply."
        ));
    } else {
        self.set_info(summary);
    }

    self.ui.prompt = PromptState::None;
    self.rebuild_left_items();
    Ok(())
}

fn apply_session_settings_to_runtime(
    &mut self,
    session_id: &str,
    settings: &SessionSettings,
) {
    if let Some(engine) = self.runtime.watch_engines.get_mut(session_id) {
        for (idx, &armed) in &settings.watch_rule_arm {
            if armed {
                engine.rearm(*idx);
            } else {
                engine.disarm(*idx);
            }
        }
    }
    // Auto-clear watch rule (built-in, ID below) is keyed off settings
    // at engine.observe time — see Phase 6.
    // Mode change requires no other live action; consumers read
    // session.settings.mode at their next decision point.
}
```

**Verification gate 4.A**: hand-roll an integration-style test in
`src/app/sessions.rs` (mirror the existing
`refuse_agent_spawn_when_max_panes_reached` style):

```rust
#[test]
fn save_session_settings_persists_and_applies_live() {
    let mut app = test_app_with_sessions(...);
    // Open the modal:
    app.open_session_settings().unwrap();
    // Mutate the draft:
    if let PromptState::SessionSettings(p) = &mut app.ui.prompt {
        p.draft.mode = ContextMode::Worker;
        p.draft.auto_clear_on_task_done = true;
    }
    app.save_session_settings().unwrap();
    // Verify in-memory updated.
    let s = app.git.sessions.iter().find(|s| s.id == "s1").unwrap();
    assert_eq!(s.settings.mode, ContextMode::Worker);
    assert!(s.settings.auto_clear_on_task_done);
    // Verify sqlite updated.
    let reloaded = app.session_store.load_sessions().unwrap();
    assert_eq!(reloaded[0].settings.mode, ContextMode::Worker);
}
```

---

## 5 · Wire YOLO + verify_envelope into PTY spawn

### 5.1 Threading the per-session settings

`PtyClient::spawn` currently takes no env-var hooks beyond what
`apply_terminal_env` provides. Extend the spawn path:

**File**: `src/pty.rs`

```rust
/// Per-session env vars to set on a spawned PTY child. Empty by
/// default; callers populate from `SessionSettings`. Distinct from
/// the global `apply_terminal_env` which sets terminal-protocol vars
/// (`TERM`, `COLORTERM`, `DUX_PANE`). New env vars whose value
/// depends on which session is being spawned go here.
#[derive(Clone, Debug, Default)]
pub struct PerSessionEnv {
    pub vars: Vec<(String, String)>,
}

impl PtyClient {
    pub fn spawn_with_env(
        command: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        scrollback_lines: usize,
        per_session: PerSessionEnv,
    ) -> Result<Self> {
        // ... existing body, plus:
        for (k, v) in &per_session.vars {
            cmd.env(k, v);
        }
        apply_terminal_env(&mut cmd);
        // ... rest unchanged
    }

    /// Backwards-compatible wrapper for paths that don't have
    /// per-session settings (tests, terminals).
    pub fn spawn(...) -> Result<Self> {
        Self::spawn_with_env(..., PerSessionEnv::default())
    }
}
```

### 5.2 Settings → env vars helper

**File**: `src/model.rs` (with `SessionSettings`):

```rust
impl SessionSettings {
    /// Translate per-session settings into env vars for the spawned
    /// PTY child. Wrappers (`claude-amq` etc.) read these to decide
    /// CLI flags. Order is stable for log readability.
    pub fn to_pty_env(&self, provider: ProviderKind) -> PerSessionEnv {
        let mut vars = Vec::new();
        if self.yolo_permissions {
            match provider {
                ProviderKind::Claude => {
                    vars.push(("CLAUDE_AMQ_YOLO".into(), "1".into()));
                }
                ProviderKind::Codex => {
                    vars.push(("CODEX_AMQ_YOLO".into(), "1".into()));
                }
                ProviderKind::Gemini => {
                    // Gemini wrapper has no YOLO flag today; document
                    // this as a no-op rather than silently dropping.
                    tracing::debug!(
                        target: "dux::session_settings",
                        "yolo_permissions=true for gemini session ignored \
                         (no wrapper flag)",
                    );
                }
                _ => {}
            }
        }
        if let Some(strict) = self.verify_envelope_override {
            vars.push((
                "DUX_AMQ_VERIFY".into(),
                if strict { "1".into() } else { "0".into() },
            ));
        }
        PerSessionEnv { vars }
    }
}
```

### 5.3 Update spawn call sites

Every `PtyClient::spawn` for an agent session (NOT for tests, NOT for
companion terminals) becomes `PtyClient::spawn_with_env(..., session
.settings.to_pty_env(provider))`:

- `src/app/workers.rs:1522` (`run_create_agent_job`)
- `src/app/sessions.rs:335`, `:377`, `:405` (resume / re-spawn paths)

Test-fixtures and companion-terminal spawns keep using the old `spawn`
(per-session env doesn't apply there).

### 5.4 Remove the global `DUX_AMQ_VERIFY` set/unset

**File**: `src/app/mod.rs:1095-1118` — the global
`std::env::set_var("DUX_AMQ_VERIFY", ...)` block becomes obsolete
once every session-level spawn gets its env explicitly. The global
flag was a workaround for not having per-session env. Replace with:

```rust
// audit03 Phase 01: per-session DUX_AMQ_VERIFY now lives in
// SessionSettings.verify_envelope_override and is set per-PTY in
// PtyClient::spawn_with_env. Sessions with override=None inherit
// the global config default at spawn time (handled in
// to_pty_env's caller — passing through config.amq.inject.verify_envelope
// when override is None).
```

The caller in workers.rs/sessions.rs assembles the env: if
`session.settings.verify_envelope_override.is_none()`, fall back to
`self.config.amq.inject.verify_envelope`.

**Verification gate 5.A**: `tests/pty_integration.rs` — spawn a
session whose settings have `yolo_permissions=true`, assert the
spawned shell sees `CLAUDE_AMQ_YOLO=1` (use `printenv` in the spawn
command).

---

## 6 · Auto-clear on task-done sentinel

### 6.1 Sentinel format

Standard sentinel: literal `[task-done]` (lowercase, square brackets,
no whitespace internal). Operator can override per-provider via a new
config key `[providers.<X>].task_done_sentinel` (default `[task-done]`).
The bridge appends a postscript to AMQ wakes for `Worker`-mode
receivers asking the agent to emit this token at end-of-task.

### 6.2 Bridge change — postscript injection

**File**: `dux-amq/scripts/dux-amq-inject-bridge`

When `DUX_PANE` is set AND the dux-side knows the receiver is in
Worker mode, append:

```
[Orchestrator note] When this task is complete, end your reply with the literal token [task-done] so the orchestration layer knows to clean up.
```

Mode discovery: dux already maintains the queue per-receiver. The
bridge can't hit sqlite directly; instead, dux's drainer performs
the postscript injection on its side when reading the queue file —
**not the bridge**. This keeps the bridge stateless.

So the postscript injection lives in
`crate::app::inject_runtime::deliver_inject_body` in `src/app/inject_runtime.rs`:

```rust
fn deliver_inject_body(&mut self, session_id: &str, receiver: &str, body: &str) {
    // Look up session settings.
    let mode = self
        .git
        .sessions
        .iter()
        .find(|s| s.id == session_id)
        .map(|s| s.settings.mode)
        .unwrap_or_default();

    let body_with_postscript = match mode {
        ContextMode::Worker => format!(
            "{body}\n\n[Orchestrator note] When this task is complete, \
             end your reply with the literal token [task-done] so the \
             orchestration layer knows to clean up."
        ),
        _ => body.to_string(),
    };

    let payload = crate::app::input::macro_payload_bytes(&body_with_postscript);
    // ... rest unchanged
}
```

### 6.3 Built-in task-done watch rule

A new built-in rule, **not** in `[providers.<X>.watch]` (so operator
edits to that config don't accidentally remove it). Constructed at
session-spawn time and only added to the engine when
`session.settings.mode == Worker && session.settings.auto_clear_on_task_done`.

**File**: `src/watch/builtin.rs` (new file)

```rust
//! Built-in watch rules. Currently: the auto-clear-on-task-done rule
//! used by `Worker`-mode sessions. Operator can't edit these via
//! config.toml — they're constructed in code based on
//! `SessionSettings`.

use super::{WatchAction, WatchBackoff, WatchBudget, WatchRule};
use crate::model::SessionSettings;

pub fn auto_clear_rule_for(provider_clear_command: &str) -> WatchRule {
    WatchRule {
        pattern: r"\[task-done\]".to_string(),
        label: "auto-clear after task done".to_string(),
        action: WatchAction::SendText {
            text: provider_clear_command.to_string(),
            append_enter: true,
        },
        backoff: WatchBackoff {
            initial_ms: 2000,
            max_ms: 10000,
            multiplier: 2.0,
            jitter_ms: 500,
        },
        budget: WatchBudget { max_attempts: 1 },
        cooldown_ms: 60000,
    }
}

pub fn provider_clear_command(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Claude => "/clear",
        ProviderKind::Codex => "/new",
        ProviderKind::Gemini => "/clear",
        _ => "/clear",
    }
}
```

### 6.4 Wire into engine attachment

**File**: `src/app/sessions.rs` — wherever a session's watch engine is
constructed (find the call to `WatchEngine::new`), append the built-in
rule when applicable:

```rust
let mut rules = config_rules_for(&session.provider, &self.config);
if session.settings.mode == ContextMode::Worker
    && session.settings.auto_clear_on_task_done
{
    rules.push(crate::watch::builtin::auto_clear_rule_for(
        crate::watch::builtin::provider_clear_command(provider_kind),
    ));
}
let engine = WatchEngine::new(session.id.clone(), &rules, &mut errors);
```

Also: replay `session.settings.watch_rule_arm` after construction so
disarmed rules start in `Disarmed` state.

**Verification gate 6.A**: `tests/watch_engine_integration.rs` —
spawn a worker session, write `[task-done]` to its PTY, observe the
engine fires `SendText { text: "/clear", append_enter: true }`.

---

## 7 · Action / keybinding wiring

### 7.1 New `Action` variant

**File**: `src/keybindings.rs`

```rust
pub enum Action {
    // ... existing ...
    SessionSettings,
}
```

### 7.2 BindingDef entry

```rust
BindingDef {
    action: Action::SessionSettings,
    default_keys: &[/* Ctrl-Shift-S — write the actual KeyCombination */],
    scopes: &[BindingScope::Global],
    help: "Open per-session settings modal for the selected session.",
    palette: Some(PaletteEntry {
        name: "session-settings",
        description: "Per-session settings (mode, YOLO, watch rules, auto-clear)",
    }),
},
```

### 7.3 Dispatch

**File**: `src/app/input.rs` — global key handler:

```rust
Some(Action::SessionSettings) => {
    self.open_session_settings()?;
}
```

**Verification gate 7.A**: launch dux, press `Ctrl-Shift-S` on a
selected session, verify modal opens. Open the palette, search
"session-settings", verify it's there.

---

## 8 · Documentation updates (in the same PR)

Touch all of:

- **README.md** (top-level) — add a small "Per-session settings"
  paragraph in the features list, point at the modal.
- **dux-amq/config/claude-md-additions.md** — add a note for agents
  about the `[task-done]` sentinel: "If your wake notification
  includes an Orchestrator note about emitting `[task-done]`, do so
  at the end of your final reply."
- **SECURITY.md** — accepted-risk paragraph: "YOLO is per-session,
  default off. The asymmetric default policy applies: a malformed
  `session_settings` blob loads `Default::default()` which has
  `yolo_permissions=false`."
- **dux-amq/README.md** — describe the new sentinel-injection
  behaviour for Worker mode wakes.

---

## 9 · Production hardening checklist

Before merging:

1. **Concurrency**: `session_store.upsert_session` is synchronous;
   `save_session_settings` calls it on the UI thread. Confirm that's
   acceptable (matches the existing rename modal). If it ever blocks
   noticeably, push to a background worker — but do NOT cache the
   write (the operator must see save-failed feedback in the same
   tick).
2. **Defaults**: every consumer of `SessionSettings` MUST handle the
   default value gracefully. Tests above force this (default-on-NULL,
   default-on-malformed-JSON).
3. **Migration safety**: confirm `0003_session_settings.sql` is
   strictly additive. Re-running it on a v3 DB must be a no-op
   (sqlite's `add column` errors on existing column; idempotency is
   provided by the `user_version` gate).
4. **Forward compat**: adding a field to `SessionSettings` must not
   require a schema migration. `#[serde(default)]` on the struct and
   `#[serde(default = "..")]` on individual fields handle this.
5. **Backwards compat**: rolling back to a pre-v3 dux on a v3 DB must
   not corrupt anything. Older dux ignores unknown columns; rows are
   readable.
6. **Logging**: every settings-driven decision logs at `debug` with
   `target: "dux::session_settings"` (mode applied, env var set,
   built-in rule attached) so post-hoc diagnosis is easy.
7. **Status line**: every save reports what changed and what timing
   applies. No silent saves.
8. **Error surfacing**: sqlite write failures, JSON parse failures,
   and runtime apply failures all surface in the status line.
9. **Tests**: every new function has at least one unit test or is
   covered by an existing integration test.
10. **Clippy `-D warnings`**: green at every commit.

---

## 10 · Suggested commit-splitting

Single PR is fine but commits should isolate concerns for review:

1. `feat(storage): add session_settings column (migration 0003)` —
   migration + load/save plumbing + tests.
2. `feat(model): SessionSettings + ContextMode types` — pure types,
   no consumers yet.
3. `feat(pty): per-session env via PtyClient::spawn_with_env` — wire
   the spawn path; existing global `DUX_AMQ_VERIFY` removed in
   favour of per-session.
4. `feat(watch): built-in auto-clear-on-task-done rule` — engine
   support + provider-specific clear commands.
5. `feat(amq-inject): postscript injection for Worker-mode receivers` —
   sentinel-required note appended to wakes.
6. `feat(app): SessionSettings modal + Ctrl-Shift-S binding` — the
   prompt, renderer, input, save, palette.
7. `docs(audit03): session settings — modal landed, doc updates` —
   README, SECURITY, threat-model, audit03 phase-doc updates.

---

## 11 · Open questions (need decision before implementation)

- **Q1**: should the `Orchestrator` mode have any behaviour
  difference from `Attended` in v1? Spec says no (same auto-clear
  protections), label is purely future-proofing. **Default
  decision**: ship as-is, no behaviour difference. Revisit if a
  concrete orchestrator-only feature lands.
- **Q2**: should `auto_clear_on_task_done` default to `true` for
  Worker-mode sessions? Spec says no — operator must opt in twice
  (set Worker mode + tick the box) to enable auto-clear. **Default
  decision**: ship asymmetric (both required), can relax later if
  feedback wants Worker mode to imply auto-clear.
- **Q3**: how does the modal handle a session whose watch engine
  isn't attached (Detached state)? Spec answers: read the rule list
  from `provider config`, show all as `(not yet armed — will arm on
  spawn)`. Save still works; arm state is stored in
  `settings.watch_rule_arm` and applied at next engine attachment.
- **Q4**: should we provide a "reset to defaults" button in the
  modal? **Default decision**: no in v1. Operator can save a draft
  with no changes; if they want to reset they can clear individual
  fields. Revisit if it becomes painful.

---

## 12 · Risks

- **R1**: per-session env propagation interacts with the AMQ wake
  daemon, which `setsid`/`disown`s itself. The wake daemon's env is
  inherited from the wrapper at spawn — and the wrapper's env is
  inherited from the dux PTY child env (which is what we're now
  setting per-session). Confirmed clean propagation in
  `audit02 Phase 13`. No risk if Phase 5 is implemented exactly as
  specified.
- **R2**: `[task-done]` token is naive — agents discussing this
  spec verbally would emit the literal string and trigger a clear.
  Mitigation: confine to Worker-mode sessions only; no orchestrator
  ever sees the rule. Operator awareness via the modal label.
- **R3**: `/clear` typed into a busy claude session might be
  rejected (the input gate). Two-phase delivery from
  `audit03 prior fix` (commit `9d28bcd`) handles this — body and
  Enter are split across ticks.
- **R4**: schema migration on a corrupted DB. The migration runner
  already integrity-checks at load (audit02 Phase 14); a corrupt DB
  refuses to upgrade and surfaces an error. No new risk.

---

## 13 · Appendix: file/symbol manifest

```
NEW
├── docs/plans/audits/audit03/01-session-settings-modal.md  (this file)
├── src/storage/migrations/0003_session_settings.sql
├── src/watch/builtin.rs
└── tests/session_settings_round_trip.rs                    (new test file)

MODIFIED
├── src/storage.rs                  (MIGRATIONS + upsert/load columns)
├── src/model.rs                    (SessionSettings, ContextMode, AgentSession)
├── src/pty.rs                      (PtyClient::spawn_with_env)
├── src/keybindings.rs              (Action::SessionSettings + BindingDef)
├── src/app/mod.rs                  (PromptState variant + bootstrap env removal)
├── src/app/sessions.rs             (open + save + apply + engine attach update)
├── src/app/input.rs                (modal input handler)
├── src/app/render.rs               (modal renderer)
├── src/app/inject_runtime.rs       (postscript injection for Worker)
├── src/app/state/runtime.rs        (any new field, if needed; spec says no)
├── src/app/workers.rs              (spawn callsites pass per-session env)
├── tests/storage_migrations.rs     (v2→v3 test)
├── tests/storage_integration.rs    (round-trip)
├── tests/watch_engine_integration.rs (auto-clear E2E)
├── tests/pty_integration.rs        (per-session env)
├── README.md, SECURITY.md, docs/operations/threat-model.md,
└── dux-amq/README.md, dux-amq/config/claude-md-additions.md
```

---

## 14 · References

- Existing modal patterns: `src/app/mod.rs:443-546` (PromptState),
  `src/app/mod.rs:2326` (rename open),
  `src/app/mod.rs:2790-2820` (watch-rules open).
- Schema migration mechanism: `src/storage.rs:25-98`.
- PTY env current state: `src/pty.rs:121-149`, `:1082-1111`.
- YOLO wrapper logic:
  `dux-amq/wrappers/claude-amq:181-195`,
  `dux-amq/wrappers/codex-amq:107-114`.
- Watch engine arm/disarm: `src/watch/engine.rs:105-115`, `:245-268`.
- Audit02 Phase 13 (TIOCSTI bridge) for env-propagation precedent.
- Audit03 prior commits: `9d28bcd` (two-phase Enter delivery —
  required for `/clear` to actually submit).

---

End of spec. Implement phases 2 → 7 in order. Run all verification
gates. Open one PR with the commits split per Section 10. Cross
out completed gates as you go and append "DONE" + commit SHA next
to each one — this doc is the authoritative checklist.

---

## 15 · Appendix: per-session system prompt knob

Phase 6 originally shipped without a system-prompt override, on the
theory that operators editing `CLAUDE.md` is the documented path. In
practice that conflates two failure modes: (a) one session needs a
narrow persona for a single task, and (b) you want the persona
permanently in the project. Forcing (a) through (b) means every
worktree clone inherits the persona, and reverting requires a commit
in the worktree's git history. This appendix describes a
per-session, opt-in `system_prompt` knob that addresses (a) without
disturbing (b).

### 15.1 Storage + type extension

`SessionSettings` grows one field:

```rust
/// Per-session system-prompt text. When `Some` and non-empty (after
/// trim), dux exports DUX_SYSTEM_PROMPT in the PTY child env at
/// spawn time; the wrappers translate that into the
/// provider-specific CLI flag. Default `None`.
#[serde(default)]
pub system_prompt: Option<String>,
```

`#[serde(default)]` keeps the forward-compat invariant: a pre-§15 dux
binary writes a JSON blob without `system_prompt`; a §15+ binary
deserialises it as `None`, applies the asymmetric default, and never
injects an unintended prompt. The matching round-trip test in
`tests/storage_integration.rs::session_settings_full_payload_round_trip_through_sqlite`
exercises a `Some("…")` payload to prove serde isn't stripping data.

### 15.2 PTY env propagation

`SessionSettings::to_pty_env` appends one var when the field is
`Some` AND the contents are non-blank:

```rust
if let Some(prompt) = self.system_prompt.as_deref()
    && !prompt.trim().is_empty()
{
    vars.push(("DUX_SYSTEM_PROMPT".into(), prompt.to_string()));
}
```

Whitespace-only is treated as `None`. This is defensive on two
boundaries:

- The wrapper's `[[ -n "${DUX_SYSTEM_PROMPT:-}" ]]` test in bash
  treats any non-empty string (including `"   "`) as truthy, so a
  single literal space would still trigger
  `claude --append-system-prompt " "` and quietly mutate the upstream
  prompt with empty-but-not-quite-empty content. Stripping at the
  dux side prevents that.
- The save path in `App::save_session_settings` does the same trim
  on the persisted blob, so the on-disk JSON never carries `""` —
  matching the `None` return from `to_pty_env`.

`tests/pty_integration.rs` covers the contract: `None`, empty,
whitespace-only → no env var; non-blank → exact byte-for-byte
round-trip including embedded newlines.

### 15.3 Wrapper translation

| Provider | Flag | Behaviour |
|----------|------|-----------|
| claude   | `--append-system-prompt <text>` | Appended to default system prompt; verified flag in `claude --help`. |
| codex    | (none)                          | Warn-and-drop. Codex's system prompt is at `~/.codex/instructions.md` (process-global) or via `-c` config (TOML keys, not free-form text). No safe per-invocation way to inject. |
| gemini   | (none)                          | Warn-and-drop. Gemini CLI's system prompt is via project-scoped `GEMINI.md`; passing text via `-i/--prompt-interactive` would land in user-message slot, with materially different semantics. |

The warn-and-drop wrappers print one line to stderr so the operator
learns their setting was a no-op for that provider:

```bash
codex-amq: DUX_SYSTEM_PROMPT set but codex has no equivalent flag; ignoring
```

When upstream codex/gemini grow an `--append-system-prompt`
equivalent the wrapper change is a one-line edit in the same place
the YOLO flag is wired.

### 15.4 Modal UX

Adds one row to the modal between YOLO and the watch rules (logical
sibling — both are spawn-time settings):

```
  System prompt  (--append-system-prompt; needs respawn)  <N> chars
  > current prompt preview…
```

`SettingsFocus::SystemPrompt` slots into the focus cycle there, so
Tab/Shift-Tab and arrow nav reach it naturally. When focused, an
inline 4-row multiline `TextInput` expands below the header — Enter
inserts a newline (multiline mode), Tab/Shift-Tab leave the field,
Esc closes the modal (discarding the buffer the same way Esc on
Title does).

The "open `$EDITOR` on a tempfile" pattern from the original spec
draft was replaced with this inline editor because (a) there is no
existing `$EDITOR` precedent in the dux codebase to mirror, (b)
spawning external processes from the TUI broadens attack surface,
and (c) `EditMacros` already uses the inline `with_multiline(N)`
pattern for prompt-shaped text. The trade-off: editing >4 visible
lines requires scrolling the inline editor; pasting a long prompt
still works.

Mouse: clicking the System prompt header row sets focus to
`SettingsFocus::SystemPrompt`; clicking inside the expanded editor
is a no-op for hit-testing (the operator can already type once
focused).

### 15.5 Apply timing

`system_prompt` is a spawn-time setting (the wrapper reads
`DUX_SYSTEM_PROMPT` once at PTY spawn). The save-summary code rolls
a system-prompt change into the respawn warning, alongside YOLO and
AMQ verify, so the operator sees:

```
Session settings saved: system prompt, spawn-time settings. Press <Reconnect> for spawn-time settings (YOLO, AMQ verify) to take effect.
```

### 15.6 Tests

- `tests/storage_integration.rs::session_settings_full_payload_round_trip_through_sqlite`
  — round-trips `system_prompt: Some("…")` through sqlite.
- `tests/pty_integration.rs::to_pty_env_emits_system_prompt_only_when_set_and_non_blank`
  + `to_pty_env_emits_system_prompt_for_every_provider` — env-var
  emission contract.
- `src/app/sessions.rs` test module — modal seeding,
  whitespace-only-persists-as-None, save triggers respawn warning,
  focus cycle includes SystemPrompt.
- `dux-amq/tests/wrappers.bats` — claude flag pair, codex/gemini
  warn-and-drop, multi-line preservation, default-deny baseline
  (no flag when env unset/empty).

### 15.7 Asymmetric default

A missing or corrupted `system_prompt` field MUST default to `None`
(no override). The serde `#[serde(default)]` attribute combined with
the existing `parse_or_default` recovery path means:

- pre-§15 binaries' blobs (no `system_prompt` key) → `None` on load.
- corrupted JSON → `SessionSettings::default()` → `None`.
- explicit `null` in the JSON → `None`.

Tests pin all three paths.

# Porting Plan: Jcode/AMQ Features to Upstream

## Status
We forked from upstream commit `1197ef5e` (April 29, 2026).
Upstream is now 2377 commits ahead with major restructuring (monolith → workspace crates).

## Upstream Structure
```
crates/
  dux-core/      - Core logic, config, providers
  dux-tui/       - Terminal UI
  dux-web/       - Web UI
  dux/           - Binary entrypoint
```

## Our Custom Features to Port

### 1. Jcode Provider (Priority: HIGH)
**Location in our fork:** `src/config.rs` default_provider_commands
**Target location:** `crates/dux-core/src/config.rs` line ~2231

**Changes needed:**
- Add jcode to `default_provider_commands()` array (4 → 5 providers)
- Config:
  - command: "jcode"
  - args: `["--no-update"]` (pins binary, prevents mid-session updates)
  - resume_args: `None` (no "resume latest" form exists)
  - resume_by_id_args: `["--no-update", "--resume", "{session_id}"]` ⚠️ NEEDS UPSTREAM FEATURE
  - oneshot_args: `["run", "--quiet", "{prompt}"]` ⚠️ NEEDS UPSTREAM FEATURE
  - forward_scroll: `true` (alt-screen TUI with own scrollback)
  - install_hint: `"brew tap 1jehuang/jcode && brew install jcode"`

**Blockers:**
- Upstream ProviderCommandConfig lacks `resume_by_id_args` and `oneshot_args` fields
- Need to add these fields first or drop the functionality

### 2. Extended Provider Config Fields (Priority: HIGH)
**Our additions to ProviderCommandConfig:**
- `resume_by_id_args: Option<Vec<String>>` - Resume specific session by ID
- `oneshot_args: Vec<String>` - One-off command execution
- `oneshot_output: OneshotOutput` - Where oneshot reads output from
- `forward_mouse: Option<bool>` - Mouse event forwarding policy

**Upstream has:**
- `forward_scroll: Option<bool>` - Similar concept for scroll
- `web_dragdrop_paste: Option<String>` - Web UI specific

**Decision needed:** Do we port these extensions or simplify jcode config?

### 3. AMQ (Async Message Queue) Features (Priority: MEDIUM)
**Files in our fork:**
- `src/amq_activity.rs` (137 lines)
- `src/amq_inject.rs` (1311 lines)
- `src/app/inject_runtime.rs` (1756 lines)
- `src/app/orchestrator.rs` (494 lines)
- Config: `AmqConfig`, `AmqInjectConfig`, `AmqOrchestratorConfig`

**Target location:** `crates/dux-core/src/` or `crates/dux-tui/src/`

**Dependencies:** Needs investigation of how these integrate with app/workers/sessions

### 4. Auto-resume Features (Priority: MEDIUM)
**File:** `src/auto_resume.rs` (150 lines)
**Config:** `AutoResumeConfig`
**Methods:** `auto_resume_shared()`, resume by jcode metadata

### 5. Input/Mouse Handling (Priority: LOW)
**Changes:** Wheel scrolling, mouse forwarding for jcode
**File:** `src/app/input.rs` (massive changes)

### 6. Bug Fixes (Priority: LOW)
- Reconnect failure logging
- Release workflow fixes (macos-13 → macos-15, cargo-edit removal)
- Install script output handling

## Porting Strategy

### Phase 1: Minimal Jcode Provider (CURRENT)
1. ✅ Understand upstream structure
2. ⬜ Add basic jcode provider WITHOUT extended fields
3. ⬜ Test basic launch/interaction
4. ⬜ Commit and verify build

### Phase 2: Extended Provider Features
1. ⬜ Add `resume_by_id_args` to ProviderCommandConfig
2. ⬜ Add `oneshot_args` and `oneshot_output` enum
3. ⬜ Update jcode provider to use these
4. ⬜ Test resume-by-id functionality

### Phase 3: AMQ Infrastructure
1. ⬜ Port amq_activity.rs, amq_inject.rs
2. ⬜ Port inject_runtime.rs, orchestrator.rs
3. ⬜ Integrate with new app structure
4. ⬜ Add AMQ configs to main Config struct

### Phase 4: Polish
1. ⬜ Port auto-resume features
2. ⬜ Port input/mouse handling improvements
3. ⬜ Port relevant bug fixes
4. ⬜ Comprehensive testing

## Decision Points

1. **Simplify vs Full Port?**
   - Option A: Basic jcode without resume_by_id/oneshot (quick, less useful)
   - Option B: Port all extended provider features (thorough, more work)
   - **Recommendation:** Option B - these features are valuable

2. **AMQ: Keep or Drop?**
   - AMQ is 3500+ lines of custom code
   - If critical: port after provider basics
   - If experimental: defer or drop

3. **Testing Strategy**
   - Build after each phase
   - Manual testing of jcode provider
   - Run existing test suite

## Current Step
Starting Phase 1.2: Add basic jcode provider to upstream config

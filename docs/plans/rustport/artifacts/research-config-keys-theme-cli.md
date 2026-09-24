# Research Brief — Configuration, Keybindings, Theming, CLI, Providers

Domain research for the `dux` overhaul (ratatui TUI, ~69.5k LOC Rust) covering
`src/config.rs`, `src/keybindings.rs`, `src/theme.rs`, `src/cli.rs`, `src/main.rs`,
`src/provider.rs`, `src/editor.rs`, `src/clipboard.rs`.

Every structural claim below was verified by reading the relevant functions and
following call paths, not by pattern-matching. Where a naive search would give a
misleading answer, the corrected figure and the reason are recorded.

---

## Measurement note: production vs. test line counts

The headline file sizes are inflated by inline `#[cfg(test)]` modules. All
decomposition planning must use production counts.

| File | Total | Production | Tests | Test share |
|---|---|---|---|---|
| `src/config.rs` | 4,952 | L1–3237 | L3238–4952 | 35% |
| `src/keybindings.rs` | 2,862 | L1–2101 | L2102–2862 | 27% |
| `src/cli.rs` | 1,991 | L1–1206 | L1207–1991 | 39% |
| `src/theme.rs` | 976 | L1–666 | L667–975 | 32% |
| `src/app/render.rs` | 7,565 | L1–7178 | L7179–7565 | 5% |
| `src/provider.rs` | 270 | L1–103 | L104–270 | 61% |
| `src/editor.rs` | 268 | L1–176 | L177–268 | 34% |
| `src/clipboard.rs` | 263 | L1–61 | L62–263 | 76% |
| `src/main.rs` | 169 | all | none | 0% |

`keybindings.rs` has two `#[cfg(test)]` markers (L1609 and L2101); the first is a
short attribute block, and the real test module begins at L2102. Production
therefore runs to L2101, not L1608.

---

## 1. Verified structure

### 1.1 `src/config.rs` — schema, renderer, migration

**Root type.** `Config` at `config.rs:34-58`, carrying `#[serde(default)]` at the
*container* level (`config.rs:33`). This is load-bearing: a `config.toml` missing
entire sections still deserializes, because every absent section falls back to
`Default::default()`.

**The 13 top-level sections** (`config.rs:34-58`):
`schema_version` (root scalar), `defaults`, `workspace`, `providers`, `terminal`,
`logging`, `projects`, `ui`, `editor`, `keys`, `macros`, `storage`, `auto_resume`,
`limits`, `amq`.

#### Complete key inventory

`[defaults]` — `config.rs:231-239`
| Key | Type | Default source |
|---|---|---|
| `provider` | `String` | `"claude"` (`:864`) |
| `start_directory` | `Option<String>` | resolved to `$HOME` at first boot |
| `commit_prompt` | `Option<String>` | `None`; falls back to `DEFAULT_COMMIT_PROMPT` (`:20-30`) |
| `enable_randomized_pet_name_by_default` | `bool` | `false` |
| `auto_resume_on_start` | `bool` | — |

`[workspace]` — `config.rs:327-336`. Typed `Option<WorkspaceConfig>`; `None` means
the config predates workspace modes and must retain worktree isolation (`:9-12`).
| `default_mode` | `WorkspaceMode` (`shared`\|`worktree`, `:310-314`) | `Shared` |
| `auto_resume_shared` | `bool` | `false` |

`[providers.<name>]` — `ProvidersConfig` `:241-246` is `#[serde(flatten)]` over an
`IndexMap<String, ProviderCommandConfig>`, so provider names are open-ended.
`ProviderCommandConfig` `:275-295`:
| Key | Type | Notes |
|---|---|---|
| `command` | `String` | binary name, PATH-resolved |
| `args` | `Vec<String>` | fresh-launch args |
| `resume_args` | `Option<Vec<String>>` | latest-session resume |
| `resume_by_id_args` | `Option<Vec<String>>` | supports `{session_id}` placeholder |
| `resume_wait_timeout_ms` | `Option<u64>` | |
| `oneshot_args` | `Vec<String>` | supports `{prompt}` and `{tempfile}` |
| `oneshot_output` | `OneshotOutput` (`stdout`\|`tempfile`, `:268-272`) | |
| `install_hint` | `Option<String>` | shown when the binary is missing |
| `forward_scroll` | `bool` | |
| `forward_mouse` | `Option<bool>` | drag forwarding under terminal mouse mode |
| `watch` | `Vec<WatchRule>` | nested; see below |

Nested `WatchRule` (`watch/rule.rs:10-34`): `pattern`, `label`, `action`,
`backoff{initial_ms, max_ms, multiplier, jitter_ms}` (`rule.rs:166-170`),
`budget{max_attempts}` (`rule.rs:202-203`), `cooldown_ms`, `kind`.

`[terminal]` — `:248-253`: `command` (`String`, defaults from `$SHELL`, `:2663-2668`),
`args` (`Vec<String>`).

`[logging]` — `:255-260`: `level` (`String`), `path` (`String`).

`[[projects]]` — `ProjectConfig` `:297-306`: `id` (`String`, `#[serde(default = "new_project_id")]`
generating a UUID, `:356-360`), `path`, `name` (`Option`), `default_provider` (`Option`),
`commit_prompt` (`Option`), `workspace_mode` (`Option<WorkspaceMode>` with a custom
deserializer `:338-354` that tolerates a legacy string form).

`[ui]` — `:786-799` (12 keys): `left_width_pct`, `right_width_pct`,
`terminal_pane_height_pct`, `staged_pane_height_pct`, `commit_pane_height_pct`
(all `u16`), `agent_scrollback_lines` (`usize`), `branch_sync_interval` (`u16`),
`show_diff_line_numbers` (`bool`), `diff_tab_width` (`u16`), `github_integration`
(`bool`), `pr_banner_position` (`String`), `theme` (`String`, default
`theme::DEFAULT_THEME_NAME` = `"dux_dark"`, `:826`).

`[editor]` — `:262-266`: `default` (`String`).

`[keys]` — `KeysConfig` `:158-161`: `show_terminal_keys` (`bool`) plus
`#[serde(flatten)] bindings: BTreeMap<String, Vec<String>>`. Action names are
open-ended in the type but validated against `BINDING_DEFS` at load.

`[macros]` — `MacrosConfig` `:224-228` is `#[serde(flatten)]` over
`IndexMap<String, MacroEntry>`; `MacroEntry` `:215-218` = `text` (`String`) +
`surface` (`MacroSurface`: `agent`\|`terminal`\|`both`, `:168-173`).

`[storage]` — `:362-367`: `backup_interval_minutes` (`u32`, default 30, `:369-371`).

`[auto_resume]` — `:391-407`: `concurrency` (`usize`, default 4, `:409`),
`stale_days` (`u32`, default 30, `:413`), `stagger_ms` (`u64`, default 250, `:417`).

`[limits]` — `:441-485`: `max_panes` (default 0 = unlimited, `:487`),
`max_panes_soft_warn` (16, `:491`), `max_companion_terminals` (0, `:495`),
`max_total_scrollback_mb` (256, `:499`), `disk_high_water_pct` (95, `:503`),
`disk_warn_pct` (80, `:507`), `enable_scrollback_overflow_autodetach` (false, `:511`).

`[amq.inject]` — `AmqInjectConfig` `:547-668`, 14 keys: `enabled`, `queue_dir`,
`busy_markers` (`Vec<String>`), `busy_scan_lines`, `delivery_timeout_secs`,
`max_message_age_secs`, `poll_interval_ms`, `max_message_bytes`, `verify_envelope`,
`active_session_quiet_secs`, `phase_delay_ms`, `startup_grace_ms`,
`post_delivery_cooldown_ms`, `auto_clear_collaboration_quiet_secs`. Each has a
dedicated `default_amq_inject_*` fn at `:669-722`.

`[amq.orchestrator]` — `:754-765`: `enabled` (`:767`), `poll_interval_secs` (`:771`).

**Total: 53 keys reach the canonical template** (counted as `key:` entries in the
schema table), plus the dynamically-rendered `[providers.*]`, `[[projects]]`,
`[keys]`, `[macros]`, and `[terminal]` blocks.

#### The canonical commented-config renderer

This is **not** a giant string literal — it is a declarative table interpreted by a
small renderer, which is why the "config file is the documentation" tenet holds.

- `ConfigEntry` enum `config.rs:1368-1391`, with variants `Comment(&'static str)`,
  `Blank`, `Section(&'static str)`, `Field { key, comment: Option<CommentSource>, value_fn: fn(&Config) -> FieldValue }`,
  plus five dynamic-arity escape hatches: `Providers`, `Terminal`, `Projects`,
  `Keys`, `Macros`.
- `CommentSource` `:1362-1365` is `Static(&'static str)` or `Dynamic(String)` — the
  `Dynamic` variant exists specifically so comments can embed runtime keybinding labels.
- `FieldValue` `:1347-1360` is the typed value union.
- `config_schema(generate_commit_key: &str) -> Vec<ConfigEntry>` `:1393-1922`
  (531 lines) is the data.
- `render_config(config, bindings) -> String` `:1924-1998` (~75 lines) is the
  interpreter. It takes `&RuntimeBindings`, so binding-aware comments are available
  throughout.
- `render_default_config()` `:2000-2004` and `render_config_with(...)` `:2006-2017`
  are the public entry points.
- Section renderers for the dynamic variants: `render_keys_config` `:2476-2537`,
  `render_macros_config` `:2538-2580`, `render_projects` `:2581-2631`,
  `render_provider_configs` `:2857-2862`, `render_terminal_config` `:2863-2880`,
  `render_provider_config` `:2881-3045`.
- TOML escaping helpers: `escape_toml_multiline` `:2632`, `escape_toml_string` `:2636`,
  `render_string_list` `:2654`.

**Documentation coverage is complete for the static fields: `comment: None`
appears zero times** (verified by direct count). Every one of the 53 `key:` entries
carries a substantive comment.

#### The `toml_edit`-preserving save path

- `save_config(...)` `:2019-2186` parses the on-disk file into a
  `toml_edit::DocumentMut`, then applies targeted patches. Comments, key order,
  whitespace, and unknown keys all survive. Proven by
  `save_config_preserves_user_comments` (`:4674-4729`).
- Patch helpers `:2251-2344`: `patch_table_str`, `patch_table_opt_str`,
  `patch_table_opt_multiline`, `patch_table_u16`, `patch_root_u32`,
  `patch_table_usize`, `patch_table_bool`, `patch_table_string_array`,
  `remove_table_key`, `remove_table_key_item`, plus `ensure_table` `:2244`.
- Composite patchers: `patch_providers` `:2346-2402`, `patch_projects` `:2403-2442`,
  `patch_macros` `:2444-2475`.
- `write_config_atomic` `:2187-2192` → `atomic_write_with` `:2193-2243` (temp file +
  rename).

**Known gap, see Risk register:** five sections have no `patch_table_*` call at all.

#### `dux config diff` and `dux config regenerate`

- `run_diff` `cli.rs:394-411` reads the on-disk file, parses it to `Config`, and
  dispatches to one of two modes.
  - `run_diff_summary` `cli.rs:425-437` → `config_summary_changes` `:438-483`:
    serializes both the current config and `Config::default()` to
    `serde_json::Value` and walks them with `diff_typed_value` `:446-483`, emitting
    one line per differing leaf. This is a *semantic* diff against defaults.
  - `run_diff_raw` `cli.rs:412-424` renders `config::render_default_config()` and
    prints a unified textual diff via `print_unified_diff` `:546-558`. Note it
    ignores its own `_current_raw` parameter and instead re-renders the *parsed*
    config through `render_config_for_diff` `:540-545`, so the comparison is
    canonical-form vs canonical-form, not literal file bytes.
- `run_regenerate` `cli.rs:496-526`: renders a fresh canonical config; without
  `--yes` it only previews the diff, with `--yes` it overwrites. **It does not take
  a backup** — the previous file is replaced.
- `truncate_display` `:527-539` bounds long values in the summary output.

#### Config location per platform

`DuxPaths::discover()` `config.rs:1088-1101` → `resolve_root` `:3108-3125` →
`discover_root` `:3127-3143`. Precedence:
1. `DUX_HOME` (must be an absolute path, else rejected)
2. macOS: `~/.dux/`
3. Linux: `$XDG_CONFIG_HOME/dux/`, else `~/.config/dux/`

Siblings in that root: `config.toml`, `sessions.sqlite3` (+ `.bak`), `dux.lock`,
`dux.log`, `dux-crash.log`, `store-id`, `worktrees/`, `themes/`.
`expand_path` `:3148-3228` handles `~` and `$VAR` expansion, guarded by
`is_valid_var_name` `:3229-3236`.

#### The `migrate_config` version chain

`CONFIG_SCHEMA_CURRENT = 4` (`config.rs:63`). A config with no `schema_version`
field is treated as version 1 (`default_schema_version` `:69-71`). The ladder is
`migrate_config` `:82-154`, a `while c.schema_version < CURRENT` loop:

| Arm | Line | What it does |
|---|---|---|
| `0 → 1` | `:84-91` | No-op. Pre-versioning configs adopt the current layout; every field added since carries a serde default. |
| `1 → 2` | `:92-102` | Clears the stale default `"o"` binding for `Action::OpenWorktreeInEditor`, but **only if** the user's binding list is exactly `["o"]` — a hand-customised binding is left alone. |
| `2 → 3` | `:103-137` | Rewrites the Codex provider entry: adds `--no-alt-screen` to `args`, `resume_args`, and `resume_by_id_args`, and sets `forward_scroll = false`. Guarded by `uses_legacy_defaults` (`:107-119`) so only stock, unmodified Codex config is touched. |
| `3 → 4` | `:138-149` | Sets `forward_mouse = Some(false)` for `claude` and `codex` entries whose `command` is still one of the stock values (`claude`/`claude-amq`, `codex`/`codex-amq`). |
| `_` | `:150` | `break` — a config from a *newer* dux build passes through untouched rather than being downgraded. |

Policy is documented at `docs/contributing/schema-policy.md`: forward-only,
append-only, never edit a shipped arm.

There is also a separate, document-level deprecation mechanism that runs *before*
parsing: `DeprecatedConfigKeyRule` / `DEPRECATED_CONFIG_KEYS` `:1255-1287` with
actions `Replace`/`Remove`/`Fail` (`:1262-1272`), applied by
`apply_config_deprecations` `:1288-1319`. Currently one rule is registered
(renaming `prompt_for_name`, `:1278-1287`, via `migrate_prompt_for_name` `:1320-1346`).

#### Load and validation flow

`ensure_config(paths)` `:1112-1149`:
1. `apply_config_deprecations` on the raw document
2. `toml::from_str` → `Config`. **Parse failure is a hard error** propagated to
   `main()`; there is no silent fallback to defaults.
3. `migrate_config`
4. `ProvidersConfig::ensure_defaults()` `:1046-1073` — backfills any of the 8 stock
   providers missing from an older config, and fills in individually-missing fields
   (e.g. `:1058-1060` adds `resume_args` if absent)
5. `validate_shared_project_paths` `:1240-1254`
6. Persist if the schema version advanced

`load_config_read_only` `:1154-1171` is the non-mutating variant used by `dux doctor`
and the reset inventory.

Validation:
- `validate_keys` `:3046-3081` — rejects unknown action names (`:3051-3053`),
  unparseable key strings (`:3057`), and scope conflicts (`:3062-3078`).
- `check_provider_available` `:3085-3107` — PATH probe with the configured
  `install_hint`.
- `validate_shared_workspace_path` `:1192-1217`, `canonicalize_allow_missing` `:1218-1239`.

On validation failure, `app/mod.rs:1476-1482` prints the message and calls
`std::process::exit(1)`. This runs *before* `RuntimeBindings::from_keys_config`
(`app/mod.rs:1483`).

#### Default providers

`default_provider_commands() -> [(&'static str, ProviderCommandConfig); 8]`
`config.rs:2677-2856`. The eight names, at `:2680, 2711, 2727, 2760, 2776, 2792, 2808, 2831`:
`claude`, `cline`, `codex`, `gemini`, `opencode`, `kilocode`, `ntl`, `copilot`.
**This matches the CLAUDE.md claim exactly.**

---

### 1.2 `src/keybindings.rs` — one declarative table drives everything

**Line map (production L1–2101):**

| Range | Contents |
|---|---|
| L8–98 | `Action` enum |
| L100–186 | `BindingScope` (+`display_name`), `HintContext`, `HelpEntry`, `PaletteEntry`, `BindingDef` |
| L187–466 | `impl Action` — `config_name`, `config_description`, `help_section` |
| L468–1559 | `BINDING_DEFS` const table (~1,090 lines) |
| L1561–1638 | `HELP_SECTION_ORDER`, key normalizers, formatters |
| L1640–1857 | `RuntimeBinding`, `RuntimeBindings` + display API |
| L1859–1957 | `KeyConflict`, `keys_conflict`, `resolve_keys`, `detect_conflicts` |
| L1958–2099 | `InteractiveByteBinding(s)`, `interactive_byte_patterns`, `key_combination_to_bytes` |
| L2102–2861 | tests |

**`Action` has exactly 80 variants** (`keybindings.rs:8-98`). Verified by
`awk 'NR>=8&&NR<=100' src/keybindings.rs | grep -cE '^\s{4}[A-Za-z]+,'`. Of these,
**13 are palette-only** with `default_keys: &[]` and `scopes: &[]`: `KillRunning`
(`:1323-1333`), `SortAgentsByUpdated`/`SortAgentsByCreated`/`SortAgentsByName`
(`:1384-1416`), `EditMacros` (`:1431`), `DebugInput` (`:1442`),
`ToggleDiffLineNumbers` (`:1453`), `ResourceMonitor` (`:1464`),
`PruneOrphanWorktrees` (`:1475`), `ToggleGithubIntegration` (`:1486`),
`ToggleRandomizedPetNameDefault` (`:1497`), `TogglePrBannerPosition` (`:1508`),
`ForceReconnectAgent` (`:1519`), `WatchRules` (`:1530`).

**`BindingScope`** `:102-115` — 12 variants: `Global`, `Left`, `Center`, `Files`,
`Interactive`, `Resize`, `Palette`, `Browser`, `RuntimeKill`, `Dialog`,
`CommitInput`, `Help`.

**`BindingDef`** `:178-186` = `{ action, default_keys: &'static [KeyCombination],
scopes: &'static [BindingScope], help: Option<HelpEntry>,
hint_contexts: &'static [(HintContext, &'static str)], palette: Option<PaletteEntry> }`.
42 entries carry palette metadata, 58 carry help metadata.

**This single table generates the help overlay, the command palette, and the footer
hint bar.** That is why hardcoded-label violations are rare.

**Parsing.** `crokey = "1.4.0"` (`Cargo.toml:27`) over `crossterm = "0.29.0"`
(`Cargo.toml:11`); imports at `keybindings.rs:3-4`.
- Config strings live in `KeysConfig.bindings` (`config.rs:158-161`).
- `RuntimeBindings::from_keys_config` `:1667-1685` resolves each action's
  `config_name()`, calling `crokey::parse(&normalize_key_string(s)).ok()` (`:1674`).
- `normalize_key_string` `:1631-1638` rewrites a bare uppercase letter (`"P"`) to
  `"shift-p"`, because `crokey::parse` lowercases its input (documented `:485-493`).
- At lookup time both sides are `.normalized()` plus two hand-rolled fixups:
  `normalize_backtab` `:1574-1580` (crossterm `BackTab` → `Tab+SHIFT`) and
  `normalize_ctrl_punct` `:1586-1598` (crossterm delivers Ctrl+`` \ ] ^ _ `` as
  Ctrl+digit 4–7).
- Defaults use the compile-time `key!()` macro, e.g. `key!(ctrl - p)` at `:1082`.

**Error handling is fail-fast, not lenient.** An unparseable key, an unknown action
name, or a scope conflict all abort startup via `validate_keys` → `exit(1)`. The
`.filter_map(...ok())` silent-drop at `:1674`/`:1902` is therefore unreachable on
the normal boot path; it only matters for code paths that construct
`RuntimeBindings` without validating first (test helpers and `config.rs` render
helpers, all of which pass `KeysConfig::default()`).

**Multiple keys per action** are native (`default_keys` is a slice): `Quit` →
`[q, ctrl-c]` (`:1181`), `MoveDown` → `[j, Down]` (`:502`).

**Same key across disjoint scopes is deliberate and load-bearing.** `ctrl-g` is the
default for three different actions: `ExitInteractive` (scopes Interactive/Center/Left,
`:816`), `GenerateCommitMessage` (Files, `:966`), `ExitCommitInput` (CommitInput,
`:1047`). `keys_conflict` `:1878-1889` only fires within overlapping scopes; the
design is documented in the comment block at `:468-497`.

**`RuntimeBindings` API** (`:1656-1857`):
- `from_keys_config` `:1667`, `new` `:1689`
- `lookup(&KeyEvent, BindingScope) -> Option<Action>` `:1717-1738` — the single
  scope-gated dispatch primitive, with subset-modifier matching and
  declaration-order tiebreak
- `label_for(Action) -> String` `:1742-1749` — first key, formatted via
  `display_format()`. **Returns `""` for an unbound action** (`.unwrap_or_default()`,
  `:1748`).
- `labels_for` `:1753-1765` — all keys joined with `/`
- `combined_label(a, b)` `:1769-1781` — e.g. `MoveDown`+`MoveUp` → `"j/k"`
- `hints_for(HintContext)` `:1784-1800`
- `help_sections()` `:1803-1829`, ordered by `HELP_SECTION_ORDER` `:1561-1570`
- `filtered_palette(&str)` `:1832-1856`
- `interactive_byte_patterns()` `:1990-2018` and `match_sequence(&[u8])` `:1982`

Module-level formatters: `display_format()` `:1604-1606` (crokey default → title-case,
`Ctrl-p`/`Shift-Tab`/`PgDn`), `config_format()` `:1616-1618` (all-lowercase, for TOML),
`format_key_for_config` `:1621-1623`, `format_key` `:1609-1612` (`#[cfg(test)]` only).

**Consumption** (`src/app/input.rs`). `handle_key` `:277-529` is **not** one big
match; it is an ordered chain of early-return gates, each delegating to
`self.bindings.lookup(&key, <scope>)`:
1. open `PromptState` → `handle_prompt_key` (`:281-283`), itself ~20 `lookup` sites
2. macro bar → `handle_macro_bar_key` (`:285-286`) — **bypasses `RuntimeBindings`**
3. help overlay → inline, `BindingScope::Help` (`:304-331`), plus one hardcoded
   `KeyCode::Char(' ')` (`:326`)
4. files search → `handle_files_search_key` (`:334-336`)
5. commit input → `handle_commit_input_key` (`:341-344`)
6. global-vs-pane defer logic (`:345-359`) — if Global resolves to `Quit` (`q`) but
   the focused pane also binds `q`, the pane wins
7. global action match (`:361-517`)
8. resize mode → `handle_resize_key` (`:518-521`)
9. per-pane dispatch → `handle_left_key` / `handle_center_key` / `handle_files_key`
   (`:523-527`)

Interactive/PTY mode bypasses `handle_key` entirely (`:292-298`): raw stdin flows
through `poll_and_forward_raw_input` `:1199-1342` → `process_raw_input_bytes` `:1368+`
→ `match_sequence` (`:1410`). Scroll actions carry a `conditional` flag consulted at
`:1523-1581` to implement the "only scroll when `scrollback_offset > 0`" tenet
(`has_scrollback` checks at `:1527, 1543, 1559, 1575`).

---

### 1.3 `src/theme.rs` — a bridge over the `opaline` crate

`opaline = { version = "0.4", features = ["ratatui", "builtin-themes"] }`
(`Cargo.toml:34`). dux does **not** implement its own theme engine; `theme.rs` is a
bridge that registers dux-specific semantic tokens on top of opaline's generic ones.

**Line map (production L1–666):** consts `:14-29`; `Theme` struct `:32-127`;
`load` `:139-170`; `ThemeSource` `:175-184`; `ThemeListing` `:186-197`;
`discover_available` `:199-254`; `load_or_fallback` `:256-270`;
`load_from_file` `:272-277`; `load_from_str` `:279-288`;
`register_dux_defaults` `:290-418`; `into_ratatui` `:429-442`; `impl Theme` `:444-665`.

**`Theme` is 78 flat `Color` fields** (`:32-127`) — no intermediate `Style` layer.
Semantic groups: app/text (`app_bg`, `text_fg`), header (5), borders/titles (4),
selection (2), project/session state (6), status (6), diff (11), hints (8),
overlay (4), prompt/input (5), buttons (3), PR banner (7), help (3), tips (4),
plus assorted (`warning_fg`, `branch_fg`, `scroll_indicator_*`, `nudge_border`,
`runtime_context_value_fg`). ~12 helper methods at `:548-660` compose `Style` on
demand (e.g. `key_badge_default`).

**Loading** — `load(name, paths)` `:139-170` resolution order:
1. user file `<config_root>/themes/<name>.toml`
2. embedded `dux_dark` — `const DUX_DARK_TOML: &str = include_str!("../assets/themes/dux_dark.toml")` (`:22`)
3. opaline builtin, tried in both kebab-case and underscore forms
4. `Err`

`load_or_fallback` `:256-270` wraps this and **never panics**, returning
`(Theme, Option<String>)` where the `String` is a warning surfaced to the user.
`DEFAULT_THEME_NAME = "dux_dark"` (`:18`). Selection is via the `ui.theme` config
key (`config.rs:798`, rendered with a comment at `config.rs:1576-1580`).

**Color parsing** lives inside opaline (`#rrggbb` hex, with palette/token
indirection and cycle detection). `into_ratatui` `:429-442` maps 9 canonical RGB
triples back to *named* ANSI colors so terminal palettes still apply.
`register_dux_defaults` `:290-418` derives ~70 `dux.*` tokens from opaline's generic
semantics, which is what lets any opaline builtin (Nord, Dracula, Gruvbox,
Catppuccin, …) drive the entire dux UI without a dux-specific theme file.

**Six hardcoded RGB constants** at `:24-29` — `GITHUB_PR_OPEN_BG`,
`GITHUB_PR_MERGED_BG`, `GITHUB_PR_CLOSED_BG`, `GITHUB_PR_OPEN_LABEL`,
`GITHUB_PR_MERGED_LABEL`, `GITHUB_PR_CLOSED_LABEL` — deliberately not theme-driven
(they mirror GitHub's own brand colors), and correspondingly absent from
`dux_dark.toml`.

**syntect is a separate, unbridged system.** `diff.rs:77` does
`&cache.theme_set.themes["base16-ocean.dark"]` — a hardcoded string index. Changing
the dux theme does not change code-token colors inside diffs, only the diff chrome.
Note opaline ships an (unused) syntect adapter.

`assets/themes/dux_dark.toml` (7,187 bytes) holds ~71 `dux.*` tokens under 9
commented section headers; only 2 tokens carry individual comments.

---

### 1.4 CLI surface — `cli.rs`, `main.rs`

**There is no `clap` dependency.** All argument parsing is handrolled across four
dispatchers:
- `main.rs:35-118` — top level
- `cli.rs:24-52` — `dux config`
- `cli.rs:74-104` — `dux session`
- `peer.rs` `run_peer` — `dux peer`

Flag rejection: `reject_unknown_flags` `cli.rs:54-60` and the near-duplicate
`reject_unknown_flags_and_positional` `cli.rs:109-117`.

**`cli.rs` production line map:** dispatch `:24-117`; session help/purge `:118-336`;
config help `:337-359`; reset `:360-393`; diff `:394-495`; regenerate `:496-526`;
display helpers `:527-558`; agent-data reset `:559-834`; `WithContextPath` trait
`:835-871`; doctor `:872-1106`; snapshot types + collection `:1106-1206`.

**Single-instance locking** (`main.rs:66-104`) is per-subcommand:
- `doctor` — **no lock** (read-only; must work while the TUI holds it, `:51-53`)
- `config reset` — locks only if `paths.root` exists (`:70`)
- `config regenerate --yes` — creates root, then locks (`:76-79`)
- `config path`/`diff`/`regenerate` preview/help — no lock (`:84`)
- all `session` subcommands — always lock, including `--dry-run` (`:99-103`)
- TUI — creates root, locks, then bootstraps (`:114-116`)

`acquire_lock_or_exit` `main.rs:161-168` prints and exits 1 on contention.

**Environment variables** (production reads):
| Var | Site | Purpose |
|---|---|---|
| `DUX_HOME` | `config.rs:1090` | config root override (absolute required) |
| `XDG_CONFIG_HOME` | `config.rs:1092` | Linux config root |
| `SHELL` | `config.rs:2664` | default terminal command |
| `DUX_AMQ_DOCTOR_BIN` | `cli.rs:873` | external doctor script override |
| `CLAUDE_PEERS_PORT` | `peer.rs:549` | Claude Peers broker port |
| `DUX_SESSION_ID` | `peer.rs:352` | session identity |
| `AMQ_GLOBAL_ROOT` / `AM_ROOT` | `peer.rs:663`, `purge.rs:120`, `app/mod.rs:4142-4143` | AMQ queue root |
| `STATE_ROOT` | `purge.rs:105` | persistent state root |
| `XDG_DATA_HOME` | `amq_inject.rs:143` | inject queue location |
| `PATH` | `editor.rs:67` | editor detection scan |
| `HOME` | `app/sessions.rs:44` | |
| `DISPLAY` / `WAYLAND_DISPLAY` | `clipboard.rs:85-86` | headless detection |
| `TERM` / `COLORTERM` | `pty.rs:1200-1201` | PTY environment |

Only `DUX_HOME` is documented in `dux --help` (`main.rs:147-151`).

---

### 1.5 `src/provider.rs` — genuinely generic (103 production lines)

`GenericProvider` `:11-14` = `{ name: String, config: ProviderCommandConfig }`.
**There is no `Provider` trait and there are no per-provider impls** — this file
fully honors the "no adapters" tenet.

- `command()` `:17-19`
- `build_oneshot_command(prompt, cwd) -> (Command, Option<NamedTempFile>)` `:30-64`
  — substitutes `{prompt}` and `{tempfile}`; when `oneshot_output == Tempfile` it
  creates a `0600`-mode `NamedTempFile` whose `Drop` unlinks it (RAII, no manual
  cleanup, `:36-49`).
- `run_oneshot(prompt, cwd) -> Result<String>` `:76-92` — reads from the tempfile or
  stdout; the tempfile handle is held until after the read so it is unlinked on
  every return path including errors.
- `create_provider(name, config)` `:97-102`.

**Oneshot has exactly one production consumer:** commit-message generation at
`app/input.rs:1079-1082` (`create_provider` then `run_oneshot` on a spawned thread).
The base prompt comes from `config.commit_prompt_for_project(project_path)`
(`app/input.rs:1075`), falling back to `DEFAULT_COMMIT_PROMPT` (`config.rs:20-30`).

**PTY spawn** — `pty.rs`: `spawn(command, args, cwd, rows, cols, scrollback_lines)`
`:155-172` delegating to `spawn_with_env(..., per_session: PerSessionEnv)` `:187+`.
Signature is fully generic. **`pty.rs` contains zero provider-name branching in
production code** — the only mentions are comments at `:182-183` and `:899-900`.

---

### 1.6 `src/editor.rs` (176 production lines)

- `EditorKind` `:9-15` — a closed enum: `Cursor`, `VsCode`, `Zed`, `Antigravity`.
- `EditorSpec` `:25-31` and `const EDITOR_SPECS: &[EditorSpec]` `:33-61` — a
  **hardcoded 4-entry table** of `{kind, label, config_key, commands, aliases}`.
- `detect_installed_editors()` `:64-81` scans every `PATH` directory and matches
  executable names against `EDITOR_SPECS`.
- `preferred_editor(detected, configured)` `:83-93` — honors the `editor.default`
  config key, falling back to the first detected editor.
- `matches_configured_editor` `:95-105` — matches on `config_key`, `aliases`, or
  `commands`.
- `launch_editor` `:107-122` — spawns detached with null stdio.
- `editor_launch_args` `:125-133` — hardcodes `--new-window` for
  Cursor/VsCode/Antigravity; nothing for Zed.

**`$EDITOR` and `$VISUAL` are never read** (verified: no `env::var` for either
anywhere in `src/`). Adding a fifth editor requires a code change.

### 1.7 `src/clipboard.rs` (61 production lines)

Uses `arboard = "3.4.1"` (`Cargo.toml:8`) on a long-lived worker thread — required
on X11, where the clipboard owner process must stay alive to serve pastes
(`:19-24`). `Clipboard::new()` `:28-38` spawns the thread; `copy_text` `:46-60` is
fire-and-forget, with the result returned later as
`WorkerEvent::ClipboardCopyCompleted`.

Fallback chain (`clipboard_worker` `:164-200`): if `no_display()` `:83-88` (no
`DISPLAY` and no `WAYLAND_DISPLAY`, non-macOS) → OSC 52 only (`osc52_only_loop`
`:154-162`); if `arboard` init fails → OSC 52; otherwise arboard with OSC 52 as
per-request fallback. `osc52_copy` `:134-151` writes to `/dev/tty` so the escape
reaches the controlling terminal even when stdout is redirected.

`const OSC52_MAX_BYTES: usize = 100_000` (`:10`) — hardcoded, matching tmux/WezTerm.
Base64 is hand-rolled (`:90-120`) because there is no `base64` crate dependency;
this is justified, not gold-plating.

---

## 2. Decomposition seams

Target: no file over 500 lines. **Inline test modules must move to sibling
`#[path]` test files as part of each split** — otherwise the arithmetic does not work
(tests are 27–39% of these files).

### 2.1 `src/config/` — from 3,237 production lines

| Module | Contents | ~Lines |
|---|---|---|
| `mod.rs` | `Config` struct, re-exports | 90 |
| `migrate.rs` | `migrate_config` ladder, `CONFIG_SCHEMA_CURRENT`, `default_schema_version` (`:60-154`) | 100 |
| `deprecations.rs` | `DeprecatedConfigKey*`, `DEPRECATED_CONFIG_KEYS`, `apply_config_deprecations*`, `migrate_prompt_for_name` (`:1255-1346`) | 95 |
| `schema/providers.rs` | `ProvidersConfig`, `ProviderCommandConfig`, `OneshotOutput` + impls (`:241-295, 873-946, 1046-1076`) | 180 |
| `schema/projects.rs` | `ProjectConfig`, `WorkspaceMode`, `WorkspaceConfig`, `deserialize_workspace_mode_override`, `new_project_id` (`:297-360`) | 130 |
| `schema/ui.rs` | `UiConfig`, `EditorConfig`, `TerminalConfig`, `LoggingConfig` + Defaults (`:248-266, 786-799, 947-991`) | 130 |
| `schema/keys_macros.rs` | `KeysConfig`, `MacroSurface`, `MacroEntry`, `MacrosConfig` (`:158-229, 839-859`) | 170 |
| `schema/limits.rs` | `StorageConfig`, `AutoResumeConfig`, `LimitsConfig` + 11 default fns (`:362-533`) | 200 |
| `schema/amq.rs` | `AmqConfig`, `AmqInjectConfig` (14 keys), `AmqOrchestratorConfig` + 17 default fns (`:534-785`) | 250 |
| `paths.rs` | `DuxPaths`, `resolve_root`, `discover_root`, `expand_path`, `is_valid_var_name` (`:1077-1111, 3108-3237`) | 180 |
| `load.rs` | `ensure_config`, `load_config_read_only`, `registered_project_paths`, shared-workspace validators (`:1112-1254`) | 180 |
| `template/entries.rs` | `FieldValue`, `CommentSource`, `ConfigEntry`, first half of `config_schema` | 320 |
| `template/entries_amq.rs` | `config_schema` tail (amq / keys / macros / projects sections) | 220 |
| `template/render.rs` | `render_config`, `render_default_config`, `render_config_with`, `render_keys_config`, `render_macros_config`, `render_projects`, `render_terminal_config`, TOML escapers (`:1924-2017, 2476-2676`) | 280 |
| `template/providers.rs` | `default_provider_commands`, `render_provider_configs`, `render_provider_config` (`:2677-3045`) | 380 |
| `save.rs` | `save_config`, `write_config_atomic`, `atomic_write_with`, `ensure_table`, all `patch_*` (`:2019-2475`) | 460 |
| `validate.rs` | `validate_keys`, `check_provider_available` (`:3046-3107`) | 70 |

`template/providers.rs` is the only file near the cap; extracting the ~75-line
Claude-only watch-rule example block (`:2964-3035`) to an `include_str!` fragment
brings it to ~300 and simultaneously fixes tenet violation P12 (below).

### 2.2 `src/keys/` — from 2,101 production lines

| Module | Contents | ~Lines |
|---|---|---|
| `mod.rs` | module docs, re-exports, `HELP_SECTION_ORDER`, `BINDING_DEFS` assembly | 80 |
| `types.rs` | `Action` (80 variants) + its three impl blocks, `BindingScope`, `HintContext`, `HelpEntry`, `PaletteEntry`, `BindingDef` (`:8-466`) | 466 |
| `defs/nav_and_projects.rs` | `BINDING_DEFS` sections Navigation + Projects (`:498-801`) | 304 |
| `defs/agent_and_files.rs` | Agent + Files + CommitInput (`:802-1055`) | 254 |
| `defs/global_and_overlays.rs` | Global + Resize + Overlays (`:1056-1321`) | 266 |
| `defs/palette_only.rs` | the 13 palette-only actions (`:1322-1559`) | 238 |
| `format.rs` | `normalize_backtab`, `normalize_ctrl_punct`, `display_format`, `format_key`, `config_format`, `format_key_for_config`, `normalize_key_string` (`:1574-1638`) | 110 |
| `runtime_lookup.rs` | `RuntimeBinding`, `RuntimeBindings` struct, `new`, `from_keys_config`, `lookup` (`:1645-1738`) | 450 |
| `runtime_display.rs` | `label_for`, `labels_for`, `combined_label`, `hints_for`, `help_sections`, `filtered_palette` (`:1742-1857`) | 270 |
| `conflicts.rs` | `KeyConflict`, `keys_conflict`, `resolve_keys`, `detect_conflicts` (`:1859-1957`) | 195 |
| `interactive_bytes.rs` | `InteractiveByteBinding(s)`, `interactive_byte_patterns`, `key_combination_to_bytes` (`:1958-2099`) | 255 |

Line counts include each module's migrated tests.

**Splitting the 1,090-line `BINDING_DEFS` is the only structurally non-trivial
part.** The safe technique: each `defs/*.rs` exports
`pub const DEFS: &[BindingDef] = &[...]`, and `mod.rs` reassembles with
`pub static BINDING_DEFS: LazyLock<Vec<BindingDef>> = LazyLock::new(|| [nav_and_projects::DEFS, agent_and_files::DEFS, global_and_overlays::DEFS, palette_only::DEFS].concat());`.
Because `LazyLock<Vec<T>>` derefs to a slice, **every existing `BINDING_DEFS.iter()`
call site keeps compiling unchanged** — including the two in `config.rs`
(`validate_keys` `:3049`, and `render_keys_config`). This requires adding
`#[derive(Clone, Copy)]` to `BindingDef`/`HelpEntry`/`PaletteEntry`, which is free
since they hold only `&'static` data.

### 2.3 `src/theme/` — from 666 production lines (LOWEST PRIORITY)

At 666 production lines this file is only 33% over the cap and is already coherent
and well-tested. Recommend deferring until after config/, keys/, and cli/.

| Module | Contents | ~Lines |
|---|---|---|
| `mod.rs` | `Theme` struct (78 fields), `SPINNER_FRAMES`, `DEFAULT_THEME_NAME`, `DUX_DARK_TOML`, `GITHUB_PR_*` (`:14-127`) | 150 |
| `loader.rs` | `load`, `load_or_fallback`, `load_from_file`, `load_from_str`, `discover_available`, `ThemeSource`, `ThemeListing` (`:139-288`) | 260 |
| `defaults.rs` | `register_dux_defaults` (`:290-418`) | 135 |
| `convert.rs` | `into_ratatui` and opaline→ratatui mapping (`:429-442`) | 120 |
| `style.rs` | the ~12 `impl Theme` style/badge helpers (`:444-665`) | 120 |

### 2.4 `src/cli/` — from 1,206 production lines

| Module | Contents | ~Lines |
|---|---|---|
| `mod.rs` | `run`, `run_session`, `reject_unknown_flags*`, `print_config_help`, `print_session_help` (`:24-152, 337-359`) | 180 |
| `config_cmd.rs` | `run_diff`, `run_diff_raw`, `run_diff_summary`, `config_summary_changes`, `diff_typed_value`, `summary_value`, `run_regenerate`, `truncate_display`, `render_config_for_diff`, `print_unified_diff` (`:394-558`) | 200 |
| `reset.rs` | `run_reset`, `reset_agent_data*`, `remove_session_worktree`, `resolve_reset_log_path`, all `remove_*`/`prune_*` helpers, `WithContextPath` (`:360-393, 559-871`) | 350 |
| `purge_cmd.rs` | `run_session_purge`, `run_session_purge_all`, `runtime_purge_config`, `format_plan` (`:153-336`) | 190 |
| `doctor.rs` | `resolve_doctor_script`, `run_doctor`, `merge_doctor_json`, `emit_rust_section_text`, `render_rust_section_text`, `append_orphaned_sessions_text`, `build_rust_section_json`, `doctor_db_path`, snapshot types, `collect_sessions_snapshot` (`:872-1206`) | 340 |

---

## 3. Tenet violations

### 3.1 Provider-name special-casing — 12 production sites

CLAUDE.md: *"Any CLI tool can be a provider… Adding a new provider is a config-only
change, not a code change."* and *"No adapters, no protocol layer."*

`ProviderKind` (`model.rs:42-58`) is correctly a thin, unvalidated newtype over
`String` with no enum constraint — the problem is entirely in callers that compare
its value. Test fixtures were excluded from this sweep.

| # | Site | Condition | What it changes |
|---|---|---|---|
| P1 | `watch/builtin.rs:90-97` | `match provider.as_str()` → `"claude"=>"/clear"`, `"codex"=>"/new"`, `"gemini"=>"/clear"`, `"opencode"=>"/clear"`, `_=>"/clear"` | The context-wipe slash command. **No config key exists.** Consumed at `app/mod.rs:3362`. Highest-value single fix. |
| P2 | `model.rs:642-668` | `to_pty_env` matches 4 names when `yolo_permissions` is set | Emits `CLAUDE_AMQ_YOLO` / `CODEX_AMQ_YOLO`; opencode handled elsewhere; gemini logs a documented no-op. |
| P3 | `app/sessions.rs:694-696` | `provider.as_str() == "codex" && matches!(launch, ResumeId(_))` | Appends `-C <cwd>` to the resume args. |
| P4 | `app/sessions.rs:697-702` | `provider.as_str() == "opencode" && yolo_permissions` | Appends `--auto`. |
| P5 | `app/sessions.rs:649` | `session.provider.as_str() != "claude"` inside `should_resume_session` | Claude-only guard calling `resume_recovery::claude_resume_target_exists` — the "reject Claude bridge stubs on resume" fix. |
| P6 | `app/orchestrator.rs:84-86` | `provider_needs_pty_orchestrator_policy` = `!provider.eq_ignore_ascii_case("claude")` | Every non-Claude provider gets a PTY orchestrator policy. |
| P7 | `app/inject_runtime.rs:1206-1215` | `name.eq_ignore_ascii_case("claude") \|\| ..("codex")` | Chooses bracketed-paste bytes vs `macro_payload_bytes` for prompt injection. |
| P8 | `resume_recovery.rs:68` and `:74-99` | `!matches!(provider.as_str(), "claude" \| "codex")` early-return; then `match` with `"claude"` and `"codex"` arms | Session-ID capture strategy. Backed by `enum FreshCapture { None, Claude{..}, Codex(CodexCapture) }` (`:40-47`) — **a provider adapter in all but name.** |
| P9 | `resume_recovery.rs:425` and `:493` | `identity.provider == "claude"`; `!matches!(provider.as_str(), "claude" \| "codex")` | Gates stranded-history recovery. |
| P10 | `app/workers.rs:1896` | `matches!(session.provider.as_str(), "claude" \| "codex")` | Gates `dispatch_fresh_launch_preparation`. |
| P11 | `app/workers.rs:2020` (+ `:2179, 2204, 2212`) | `provider == "codex"` | Codex-specific warning text and session-ID persistence. |
| P12 | `config.rs:2964` | `if name == "claude"` **inside the template renderer** | Only Claude gets the 4 worked watch-rule examples; the other 7 providers get a 3-line blurb. |

Also present but **acceptable**: `peer.rs:461-478` (`is_claude_session` /
`ensure_claude_peers_target`) gates the Claude Peers transport, which genuinely *is*
a Claude-specific IPC mechanism — though it would be cleaner as a capability flag.
`purge.rs:132-134` and `:561-564` hardcode `claude`/`codex`/`gemini`/`opencode`
state directories; this is listed separately below because its impact is a data
gap, not a style issue. `config.rs:104-149` (migration arms) are frozen by the
append-only schema policy and must not be edited.

**Verified CLEAN** (no production provider branching): `pty.rs`, `provider.rs`,
`amq_inject.rs`, `auto_resume.rs`, `statusline.rs`, `orphan_worktrees.rs`,
`storage.rs`, `diff.rs`, `theme.rs`, `keybindings.rs`.

**Data-integrity consequence of P-adjacent `purge.rs:132-134`:** the GDPR purge
cascade only knows the provider state directories for four hardcoded names. A
user-added provider's chat history is **never purged** by `dux session purge`. This
is a completeness gap in a compliance feature, not merely a tenet violation.

### 3.2 Hardcoded keybinding labels — 5 violations

CLAUDE.md: *"Never hardcode keybinding labels in user-facing strings… always look
up the actual binding via `RuntimeBindings::label_for()`."* Exceptions are pure
text-input contexts and palette-only command names.

A naive search for `Ctrl-`/`^X`/`Shift-` finds only one of these; the other four use
bare `Enter`/`Space`/`Esc` in status messages. All five have `self.bindings` in
scope at the call site.

| # | Site | Literal | Action it names |
|---|---|---|---|
| B1 | `config.rs:1578` | `"…Use the \`change-theme\` command in the palette (Ctrl-p) for an interactive picker."` | `OpenPalette`. Uses `CommentSource::Static` even though `CommentSource::Dynamic` is used correctly for `commit_prompt` at `:1428`. |
| B2 | `app/sessions.rs:2220` | `"Choose an editor and press Enter to open the selected worktree."` | `Confirm` — the modal really dispatches via `lookup(.., Palette)` at `input.rs:2370-2382`. |
| B3 | `app/sessions.rs:2416` | `"No running agents or terminals are selected. Press Space to select one or more runtimes first."` | `ToggleMarked` (default `space`, scope `RuntimeKill`; live at `input.rs:2057-2058`). |
| B4 | `app/sessions.rs:2433` | `"…press Enter to confirm, or Esc to keep your running sessions alive."` | `Confirm` **and** `CloseOverlay` — dialog dispatches via `lookup(.., Dialog)` at `input.rs:2555-2569`. |
| B5 | `app/input.rs:4461` | `"Select one or more runtimes before using Kill Selected. Press Space to mark the highlighted row."` | `ToggleMarked` (second instance of B3). |

**Reference implementation for the fix:** `WELCOME_TIPS` (`app/render.rs:58-220`) —
every tip is `fn(&RuntimeBindings) -> String`, zero literals. A regression test for
this exact class already exists: `config_comment_uses_dynamic_keybinding_label`
(`config.rs:4148-4160`) asserts `(Ctrl-g)` present and `(Ctrl+G)` absent. Extend
that pattern to the five sites above.

**Correct usages, for contrast:** `config.rs:1925` and `:2543`,
`app/mod.rs:1499-1503, 2090-2091, 2102, 2362-2363, 2404-2406`,
`app/workers.rs:90-93, 731-733, 993-1003`, `app/sessions.rs:2255-2262`,
`app/input.rs:591-597, 5548-5551`, `app/render.rs:1764-1766, 2331-2345, 2481-2486`.

### 3.3 Label/handler desync — a functional bug, not cosmetic

`app/render.rs:6576` renders the Resource Monitor footer from
`self.bindings.label_for(Action::CloseOverlay)` — dynamic, and therefore
*correct-looking*. But the handler at **`app/input.rs:1787`** is a bare
`if key.code == KeyCode::Esc { self.ui.prompt = PromptState::None; return Ok(false); }`
that never consults `lookup`.

Rebind `close_overlay` away from `esc` and the footer confidently advertises the new
key while only `Esc` actually closes the modal. This is worse than a static
hardcode, because the UI is *confidently wrong* rather than merely stale.

Contrast `app/input.rs:1834-1840`, which is the identical hardcode but carries the
rationale `// Esc always closes — hardcoded so a broken binding can't trap the user.`
That is a sound design; the fix at `:1787` is either to adopt the same rationale
(and make the label static to match) or to route the handler through `lookup`.

### 3.4 Three modals bypass the binding system entirely

No `BindingScope` variant covers these, so their keys **cannot be rebound at all**.

| Modal | Handler | Hint labels |
|---|---|---|
| Macro editor | `app/input.rs:3537-3709` — raw `KeyCode` for `Esc`, `Tab`/`BackTab`, `Enter`, `n`, `d`/`Delete`, `j`/`k` | `app/render.rs:5878-5880, 5916, 5945, 5993-5996` |
| Macro bar | `app/input.rs:891-956` — raw `KeyCode` | `app/render.rs:6275` |
| Help-overlay Space-scroll | `app/input.rs:326-330` — hardcoded `KeyCode::Char(' ')`, deliberately outside the table to avoid colliding with `ToggleProject`/`StageUnstage` in other scopes | `app/render.rs:2260` |

The Resource Monitor's row-expand (`app/input.rs:1812`, `KeyCode::Enter | Char(' ')`,
label at `render.rs:6580`) is a fourth instance with no corresponding `Action`.

These are internally consistent, and the rationale is written down in exactly one
place (`input.rs:1835`). The rationale is defensible; the inconsistency is that it
is undocumented in the other three.

### 3.5 Non-configurable hardcoded values — 25 HIGH

CLAUDE.md: *"All settings are configurable. Every single one."*

| # | Site | Value | Controls | Proposed key |
|---|---|---|---|---|
| H1 | `app/input.rs:10` | `MAX_RIGHT_WIDTH_PCT = 50` | **Contract bug** — `config.rs:1507` documents the range as `(5-80)`. A value of 60–80 loads fine but the first resize keypress silently snaps to 50. | fix clamp to match docs |
| H2 | `app/input.rs:7` | `MIN_LEFT_WIDTH_PCT = 14` | left pane resize floor | `[limits] min_left_width_pct` |
| H3 | `app/input.rs:8` | `MAX_LEFT_WIDTH_PCT = 38` | left pane resize ceiling | `[limits] max_left_width_pct` |
| H4 | `app/input.rs:9` | `MIN_RIGHT_WIDTH_PCT = 14` | right pane floor | `[limits] min_right_width_pct` |
| H5 | `app/input.rs:11` | `MIN_CENTER_WIDTH_PCT = 20` | center pane floor | `[limits] min_center_width_pct` |
| H6 | `app/input.rs:6` | `MOUSE_WHEEL_LINES = 3` | wheel scroll step | `[ui] mouse_wheel_lines` |
| H7 | `app/input.rs:12` | `DOUBLE_CLICK_THRESHOLD = 500ms` | double-click window | `[ui] double_click_ms` |
| H8 | `app/mod.rs:1754` | `event::poll(Duration::from_millis(100))` | the main-loop tick governor | `[ui] tick_interval_ms` |
| H9 | `app/workers.rs:3505` | `DISK_WATCHDOG_INTERVAL = 60s` | disk sampling; `config.rs:1698-1701` literally says *"samples once per minute"* but exposes no field | `[limits] disk_watchdog_secs` |
| H10 | `app/workers.rs:3509` | `SCROLLBACK_WATCHDOG_INTERVAL = 60s` | scrollback sampling | `[limits] scrollback_watchdog_secs` |
| H11 | `app/workers.rs:1566` | `Duration::from_secs(45)` | shared-workspace HEAD refresh — **quoted verbatim** in the `branch_sync_interval` comment at `config.rs:1543` yet not configurable | `[ui] shared_head_refresh_secs` |
| H12 | `app/workers.rs:1053` | `Duration::from_secs(2)` | git-status refresh floor | `[ui] git_status_poll_secs` |
| H13 | `app/workers.rs:1697` | `Duration::from_secs(2)` | git poll (focused) | same |
| H14 | `app/workers.rs:1699` | `Duration::from_secs(10)` | git poll (unfocused) | `[ui] git_status_idle_poll_secs` |
| H15 | `app/workers.rs:1619` | `Duration::from_secs(10)` | refresh debounce | `[ui] refresh_debounce_secs` |
| H16 | `app/inject_runtime.rs:53` | `MAX_INJECT_CLAIMS_PER_SCAN = 32` | inject queue drain batch | `[amq.inject] max_claims_per_scan` |
| H17 | `app/inject_runtime.rs:57` | `MAX_INJECT_PENDING_TOTAL = 128` | global pending cap | `[amq.inject] max_pending_total` |
| H18 | `app/inject_runtime.rs:61` | `MAX_INJECT_PENDING_PER_RECEIVER = 32` | per-receiver cap | `[amq.inject] max_pending_per_receiver` |
| H19 | `app/inject_runtime.rs:65` | `MAX_INJECT_ACTIONS_PER_TICK = 16` | per-tick action cap | `[amq.inject] max_actions_per_tick` |
| H20 | `app/inject_runtime.rs:84` | `WATCH_SUPPRESS_AFTER_INJECT = 10s` | watch suppression window | `[amq.inject] watch_suppress_secs` |
| H21 | `clipboard.rs:10` | `OSC52_MAX_BYTES = 100_000` | OSC 52 payload cap | `[ui] osc52_max_bytes` |
| H22 | `diff.rs:77` | `"base16-ocean.dark"` | syntect theme for diff code tokens; does not follow the dux theme | `[ui] syntax_theme` |
| H23 | `peer.rs:27` | `DEFAULT_CLAUDE_PEERS_PORT = 7899` | broker port; only the undocumented `CLAUDE_PEERS_PORT` overrides it | `[amq] claude_peers_port` |
| H24 | `cli.rs:879` | `"dux-amq-doctor"` binary name | doctor script discovery | `[logging] doctor_command` |
| H25 | `cli.rs:892` | `"../dux-amq/scripts/dux-amq-doctor"` relative path | doctor fallback path | same |

**Also violating the wall-clock tenet** (*"Animations and periodic refreshes use
wall-clock time, not tick counts"*): `app/mod.rs:2210`
`const NUDGE_DURATION_TICKS: u64 = 15; // ~1.5s at 100ms/tick` (consumed `:2214`),
and `app/workers.rs:1042` `if self.tick_count.is_multiple_of(20)` (comment at
`:1041` says "every ~2 seconds"). The assumed 100 ms cadence is not guaranteed,
because interactive/PTY mode bypasses the `event::poll` governor entirely.

**Editor selection** (`editor.rs:33-61`, `EDITOR_SPECS`) is the same defect class as
the provider tenet, one layer over: a closed 4-entry table, `--new-window` hardcoded
per kind (`:125-133`), and `$EDITOR`/`$VISUAL` never consulted. Adding an editor is
a code change.

### 3.6 Data-safety tenet — `dux config reset --all` has no confirmation

CLAUDE.md: *"Worktrees are user data. Never removed or mutated casually. Deletion
requires explicit user confirmation."*

`run_reset` (`cli.rs:360-388`) calls `reset_agent_data` when `--all` is passed,
which removes worktrees via `git worktree remove --force` (`cli.rs:710-720`), falls
back to `fs::remove_dir_all` (`:721-728`), and force-deletes the branch with
`git branch -D` (`:731-737`). **There is no interactive prompt and no `--yes` flag
on this path.**

The *structural* guards are genuinely good — `whole_workspace_target_is_within`
(`:695`), `guard_whole_workspace_removal` (`:702`), a shared-workspace skip
(`:691`), and a complete read-only inventory before the first mutation
(`:565-598`, which aborts with repair instructions if the config, project list,
store-id, or database cannot be read). But none of those is *user* confirmation.

The inconsistency is stark: `dux session purge` requires both `--hard` **and** a
typed `PURGE <branch>` confirmation (`cli.rs:218-231`), and `config regenerate`
requires `--yes` — yet the single most destructive command in the tool requires
neither.

### 3.7 Config documentation coverage — CLEAN (record this)

**53 of 53 keys are documented.** `comment: None` appears **zero** times in
`config_schema()`. There are no emitted-but-unexplained keys and no schema fields
that never reach the template. A future audit should not re-flag this.

Two narrow gaps remain, neither a tenet breach:
- `ProjectConfig.id` (`config.rs:299`) renders as a bare `id = "<uuid>"` at
  `render_projects` (`:2600`) with no comment, while every sibling field has one.
- `WatchRule` sub-fields are documented only through the Claude-only examples (see
  P12), so a user writing a Codex watch rule must read `src/watch/rule.rs` — i.e.
  leave the config file, which is exactly what the tenet forbids.
- `assets/themes/dux_dark.toml`: only 2 of ~71 tokens carry individual comments (9
  section headers cover the rest), and `README.md:244` calls this file "the complete
  reference."

### 3.8 Raw `Color::*` in rendering — CLEAN, zero real violations (record this)

CLAUDE.md: *"Use `theme.rs` constants for all colors — never use raw `Color::*` in
rendering code."*

A naive `grep -rn "Color::" src/ | grep -v theme.rs` returns **100 hits**. After
excluding test modules, comments, and legitimate categories, **the count of genuine
violations is zero.** Both an independent sub-agent and I reached this separately.

| File | Raw hits | Production, non-comment | Verdict |
|---|---|---|---|
| `pty.rs` | 50 | `:1153-1195`, `:1262-1309` | Terminal-emulator color conversion: `TermColor`/`NamedColor` → ratatui. Operates on the **child process's** colors; cannot be theme-driven. |
| `app/render.rs` | 38 | only 6 (`:1744, 6622, 6624, 6626, 6629, 6691`) | `to_grayscale()` and `pty_cell_colors()` — PTY passthrough for the read-only-pane effect. Legitimate. |
| `diff.rs` | 7 | `:161, 203, 217, 302` | syntect bridging + `Color::Reset` defaults. Legitimate. |
| `app/components/checkbox.rs` | 4 | 0 (all in tests, `#[cfg(test)]` at `:253`) | — |
| `app/input.rs` | 1 | 0 (test module starts `:6164`) | — |

`Color::Reset` (16 production uses) is a special case: it means "inherit the
terminal default," which is a semantic instruction, never a paint decision, and
must not be replaced with a theme token.

The one genuine theme gap is architectural, not a raw-color violation: **`diff.rs:77`
hardcodes the syntect theme** (H22), so code-token colors inside diffs never follow
the active dux theme.

### 3.9 Casing — CLEAN

All user-facing labels flow through `display_format()` (`keybindings.rs:1604-1606`),
which is crokey's default title-case format (`Ctrl-p`, `Shift-Tab`, `PgDn`), proven
by `format_key_display_uses_title_case_modifiers` (`:2377-2386`). Lowercase forms
appear only in the config-serialization path (`config_format()` `:1616-1618`), in
comments and tests, and in README config-syntax examples — all correct.

### 3.10 README — CLEAN

`README.md` contains **zero** keybinding labels (verified by direct search). The
`ctrl-d`/`ctrl-p`/`ctrl-q`/`ctrl-k` occurrences at `README.md:248, 252-253` are
config-*syntax* examples, and `:256` explicitly defers to the in-app `?` overlay.
This honors the "do not enumerate keybindings in README" rule.

### 3.11 Verified false positives (do not re-flag)

- `amq_inject.rs:950, 1249, 1251` and `config.rs:676, 3483, 3514` — strings like
  `"esc to interrupt"` are `busy_markers`: substrings dux scans for in the **child
  CLI's own** terminal footer to detect busy state. Not dux keybinding labels.
- `config.rs:1747, 1843` — `"Enter"` refers to the Enter **byte dux sends to the
  child PTY**, not a dux binding.
- `config.rs:2487` — `"…or modifier combos (\"Ctrl-d\")"` documents binding *syntax*.
- `config.rs:2490` — `"Ctrl-j for newline"` is a terminal convention, not a dux binding.
- `app/render.rs:2096-2097` — the help overlay's key-*notation* legend explaining
  the `Ctrl-X` syntax itself. Borderline (the `e.g. Ctrl-p` half names a real
  rebindable action) but defensible as pure notation documentation.
- `app/render.rs:2340, 2489-2499` — carry inline comments marking Tab/Enter/Esc as
  text-input controls, matching the documented CLAUDE.md exception.
- `app/render.rs:5879` — `("Tab/Shift-Tab", "surface")` is modal field navigation.

---

## 4. Reusable extractions

1. **`ProviderCapabilities` on `ProviderCommandConfig`** — see section 6. Retires
   most of the P-series without introducing a trait or adapter.
2. **A `SessionCapture` strategy, config-selected** — the honest fix for P8/P9.
   `resume_recovery.rs` already *is* an adapter; making it explicit and selected by
   a config value (`capture_mode = "uuid-arg" | "rollout-file" | "none"`) is more
   truthful than name-matching, and is the only P-site that a capabilities block
   cannot absorb.
3. **One shared commented-field writer.** `config_schema()` handles the 51 flat
   fields well, but five escape-hatch renderers each re-implement comment emission
   (`render_provider_config`, `render_keys_config`, `render_macros_config`,
   `render_projects`, `render_terminal_config`). A single `CommentedField` writer
   they all share is also where P12's per-provider asymmetry gets fixed.
4. **`label_for` must not silently return `""`.** `keybindings.rs:1742-1749` yields
   an empty string for an unbound action, so a user who unbinds one gets
   "Press  to continue". Add `label_or(action, fallback)` and make the empty case
   loud in debug builds.
5. **A `[timing]` config section** collecting H8–H15 rather than 12 scattered
   consts, and converting the two tick-count sites to `Instant::elapsed()`.
6. **Unify the four handrolled arg dispatchers.** `main.rs`, `cli.rs` (×2), and
   `peer.rs` each hand-roll flag parsing and help text; `reject_unknown_flags` and
   `reject_unknown_flags_and_positional` (`cli.rs:54, 109`) are near-duplicates that
   differ only in positional tolerance. A small shared
   `Subcommand { name, flags, help, handler }` table removes the drift risk without
   adding a `clap` dependency.
7. **A `PromptState` scope for modal dialogs** — would let the macro editor, macro
   bar, and Resource Monitor (section 3.4) join the binding system instead of
   matching raw `KeyCode`.

---

## 5. Risk register

**Back-compat is genuinely well-handled.** The strong guarantees, all verified:
- `#[serde(default)]` on the `Config` container (`config.rs:33`) — a config missing
  whole sections still loads.
- The `migrate_config` ladder is forward-only, append-only, policy-documented, and
  covered by `tests/storage_migrations.rs:516-543` (schema_version 0 → current, plus
  idempotency at `:542-543`).
- `ProvidersConfig::ensure_defaults()` (`config.rs:1046-1073`) backfills new provider
  fields into old configs field-by-field.
- `toml_edit` save preserves comments, formatting, ordering, and unknown keys
  (proved by `config.rs:4674-4729`).
- A config-key deprecation mechanism exists (`DEPRECATED_CONFIG_KEYS` `:1278`) with
  `Replace`/`Remove`/`Fail` actions.
- Bad keybinding config is **fail-fast**: `validate_keys` rejects unknown actions,
  unparseable keys, and scope conflicts, and `app/mod.rs:1476-1482` exits 1.

### Risks for the overhaul

**R1 — `save_config` silently skips five sections (highest risk).** No
`patch_table_*` call exists for `[storage]`, `[auto_resume]`, `[limits]`,
`[amq.inject]`, or `[amq.orchestrator]`. Today nothing mutates them at runtime, so
the gap is latent. The moment a refactor adds a UI path that writes any of these,
the change will appear to work and vanish on restart, with no error. **Mitigation:**
add a test that reflects over the schema and asserts every field is either patched
by `save_config` or explicitly listed as render-only.

**R2 — `patch_projects` rebuilds `[[projects]]` wholesale** (`config.rs:2403-2442`).
Hand-edited project entries not present in `Config::projects` are destroyed. This is
the one place where the otherwise-solid "preserve user edits" property does not
hold, and it should be documented or fixed before any project-management refactor.

**R3 — No `deny_unknown_fields` anywhere** (verified: zero occurrences in
`config.rs`). A typo'd key is silently ignored; the user gets default behavior and
no feedback. `[keys]` is the sole exception (`config.rs:3051-3053`). Consider a
load-time warning that lists unrecognized keys without erroring — erroring would
break the forward-compat guarantee that lets an old dux read a new config.

**R4 — `MacroEntry` has no serde defaults** (`config.rs:215-218`). `surface` is
required, so a hand-authored macro omitting it fails to deserialize and hard-errors
the *entire* config, not just that macro. No test covers this.

**R5 — No exhaustive template-coverage property test.** Coverage today is
`assert!(rendered.contains("…"))` spot-checks (`config.rs:3261, 3470, 4015, 4150,
4510, 4559, 4807`) plus round-trips (`:3446, 3463, 3469, 3480, 3555, 3572`) and
`tests/limits.rs:72, 99`. Nothing asserts that *every* schema field reaches the
template. **This test must land before `config_schema()` is touched** — it is
precisely the property a 531-line table split would silently regress, and no
existing test would catch a dropped `ConfigEntry`.

**R6 — The untested `migrate_config` 1→2 arm** (`config.rs:92-102`, clearing the
stale `"o"` binding). Every arm is covered only transitively by the 0→current
round-trip; none is exercised by name with a realistic before/after config. Arms 2→3
and 3→4 have non-trivial `uses_legacy_defaults` guards that deserve direct tests
before the file is split.

**R7 — `detect_conflicts_default_config_clean` (`keybindings.rs:2599-2607`) is the
single most valuable guardrail for the `keys/` split.** It proves the shipped
`BINDING_DEFS` never self-conflicts. Keep it green through every step of the
decomposition; if it goes red, a `defs/*.rs` file was mis-partitioned.

**R8 — Test placement blocks the split.** 1,714 test lines in `config.rs`, 760 in
`keybindings.rs`, 785 in `cli.rs`, and 309 in `theme.rs` reference private items via
`use super::*`. Splitting the modules breaks these imports en masse. **Plan the test
migration as its own commit, landed before the production split.**

**R9 — Same-key-different-scope reuse is load-bearing.** `ctrl-g` serves three
actions in disjoint scopes (`keybindings.rs:816, 966, 1047`). Any refactor that
flattens scope handling, or that changes `BINDING_DEFS` ordering across the
`defs/*.rs` boundary, risks changing the declaration-order tiebreak documented at
`:472-474` and tested by `lookup_declaration_order_wins` (`:2612-2638`).

**R10 — `dux config regenerate --yes` takes no backup** (`cli.rs:496-526`). Given
R2 and the absence of a confirmation on `reset --all` (3.6), a user has two ways to
lose hand-authored config with no recovery path.

**Existing coverage worth preserving:** `tests/storage_migrations.rs` (config
migration + idempotency), `tests/limits.rs` (explicitly asserts the
"config is documentation" tenet for `[limits]`), `theme.rs:766-862`
(`dux_dark_matches_original_palette` — an exhaustive 78-field oracle, the strongest
test in this domain), `theme.rs:880-889` (opaline derivation),
`keybindings.rs:2102-2861` (scope resolution, modifier subsets, crossterm quirks,
conflict detection, byte encoding).

**Coverage gaps:** **no file under `tests/` covers keybindings, themes, or CLI
parsing** — all three live only in inline modules, which is exactly the code that
R8 says must move first.

---

## 6. Provider capabilities proposal

The highest-leverage single change in this domain. Add a capabilities block to
`ProviderCommandConfig` (`config.rs:275-295`) so the P-series becomes config data
rather than code branches. This stays true to the tenet — it adds **no trait, no
adapter binary, and no protocol layer**; it only widens the existing config struct.

### Proposed fields

| Field | Type | Default | Replaces |
|---|---|---|---|
| `clear_command` | `Option<String>` | `Some("/clear")` | P1 |
| `yolo_args` | `Vec<String>` | `[]` | P4 |
| `yolo_env` | `Vec<(String, String)>` | `[]` | P2 |
| `paste_mode` | `PasteMode` (`bracketed`\|`literal`) | `Literal` | P7 |
| `orchestrator_policy` | `OrchestratorPolicy` (`native`\|`pty`) | `Pty` | P6 |
| `state_dirs` | `Vec<String>` | `[]` | `purge.rs:132-134, 561-564` |
| `resume_cwd_flag` | `Option<String>` | `None` | P3 |

Notes on shape:
- `clear_command` as `Option<String>` (rather than `String`) lets a provider declare
  it has *no* context-wipe command, which `watch/builtin.rs:86-89` currently cannot
  express — its comment says an unsupported provider "still fires the rule" and
  prints a help message, which is a documented-but-unfortunate fallback.
- `yolo_env` as pairs rather than a bool keeps `model.rs:637-668`'s
  `to_pty_env` a single loop with no match.
- `resume_cwd_flag: Some("-C")` turns P3 into `args.extend([flag, cwd])`.
- `state_dirs` should hold path templates resolved against `$STATE_ROOT` /
  `$XDG_DATA_HOME`, so `purge.rs` can iterate config instead of a hardcoded triple.
- Defaults must be chosen so **existing configs behave identically**: `paste_mode`
  defaults to `Literal` and `orchestrator_policy` to `Pty`, with the stock `claude`
  and `codex` entries in `default_provider_commands()` (`config.rs:2677-2856`)
  carrying the non-default values explicitly. This requires a `migrate_config`
  arm `4 → 5` following the same `uses_legacy_defaults` guard pattern as arms 2→3
  and 3→4, so hand-customized provider entries are left alone.

### What this retires

**Fully retired (7 of 12):** P1, P2, P3, P4, P6, P7, P12 — plus the `purge.rs`
state-directory gap, which is the one with actual compliance impact.

P12 is retired indirectly: once watch-rule behavior is expressed in capability
fields, the renderer no longer needs a Claude-only example block, and the shared
`CommentedField` writer (extraction 3) can emit provider-neutral documentation.

### What this does NOT retire

**P5, P8, P9, P10, P11 — the session-ID capture cluster.** These are not simple
flag differences; they encode genuinely different *algorithms*:
- Claude: dux generates a UUID and injects it via `--session-id`, then verifies the
  resume target exists (`resume_recovery.rs:80-92`, `app/sessions.rs:649`).
- Codex: dux watches the provider's own rollout files, parsing them to discover the
  session id after the fact (`resume_recovery.rs:94-96, 284-330`), coordinated
  through a global mutex (`CodexCaptureCoordinator` `:101-105`) because concurrent
  launches in one workspace are ambiguous.

No config schema expresses "parse this provider's rollout files." These five sites
need extraction 2 (a `SessionCapture` strategy selected by a `capture_mode` config
value), which is a larger change and should be a separate workstream.

**`peer.rs:461-478`** also survives, and arguably should: Claude Peers is a
Claude-specific IPC transport, not a provider behavior. It would be cleaner as a
`supports_claude_peers` capability flag but is not a tenet violation in the same
sense.

### Sequencing

1. Add the fields with backward-compatible defaults + migration arm `4 → 5`.
2. Convert P1, P2, P4, P6, P7 (pure lookups — mechanical, low risk).
3. Convert P3 and the `purge.rs` state dirs (touch data paths — needs tests).
4. Separately, extract `SessionCapture` for P5/P8/P9/P10/P11.

Steps 1–3 are safe to land inside the config decomposition; step 4 should not be
mixed with it.

---

## 7. Complete CLI surface

Handrolled parsing throughout — **no `clap` dependency**.

```
dux                                  Launch the TUI (default when no subcommand)
                                     main.rs:114-117 — creates root, acquires lock,
                                     App::bootstrap_with_lock, app.run()

dux --help | -h                      main.rs:38-45 → print_help() main.rs:120-159

dux config                           cli.rs:24-52 (lock policy: main.rs:66-85)
  path                               Print config file path.  cli.rs:41-44
  diff                               Semantic diff vs defaults, one line per
                                     differing leaf.  cli.rs:394-411, 425-483
    --raw                            Unified textual diff against the rendered
                                     default config.  cli.rs:412-424, 546-558
  regenerate                         Preview a fresh canonical config (no write).
                                     cli.rs:496-526
    --yes                            Overwrite config.toml. Takes the lock
                                     (main.rs:76-79). NO BACKUP is taken.
  reset                              Remove config.toml + dux.log. Keeps agents,
                                     sessions, and worktrees.  cli.rs:360-388
    --all                            ALSO deletes sessions.sqlite3, worktrees
                                     (git worktree remove --force), branches
                                     (git branch -D), AMQ dirs, store-id.
                                     NO CONFIRMATION PROMPT — see section 3.6.
  --help | -h | (empty)              print_config_help() cli.rs:337-359

dux session                          cli.rs:74-104. ALL subcommands take the
                                     single-instance lock (main.rs:99-103),
                                     including --dry-run.
  purge <target>                     GDPR hard-delete of one session, cascading
                                     into worktree, provider chat dirs, AMQ inbox,
                                     logs, and the sqlite row.  cli.rs:153-255
    --hard                           REQUIRED. Affirms destructive intent.
    --yes                            Skip the typed 'PURGE <branch>' confirmation
                                     (cli.rs:218-231).
    --dry-run                        Print the plan; change nothing.
    --accept-residual-data           Shared-workspace targets only: keep shared
                                     provider history, delete owned records.
                                     Rejected for non-shared targets (purge.rs:333-335).
    --workspace-wide-provider-history
                                     Shared only: purge EVERY dux session at that
                                     workspace plus provider history, including
                                     non-dux chats. Requires typing
                                     'PURGE WORKSPACE <path>' (cli.rs:218-228).
                                     Rejected for non-shared targets (purge.rs:336-339).
  purge-all                          Apply purge to every session.  cli.rs:256-312
    --yes, --dry-run
  --help | -h | (empty)              print_session_help() cli.rs:118-152

dux peer                             peer.rs run_peer
  send <target> <message...>         Route a message to another dux agent session.
    --from <handle>                  Override sender identity.
    --transport auto|amq|claude-peers
                                     Default auto. 'claude-peers' requires a Claude
                                     target (peer.rs:461-478) and rejects
                                     shared-workspace sessions (peer.rs:439).
  list                               List dux sessions and transport health.
  sync-amq | sync                    Reconcile AMQ's agent registry from
                                     sessions.sqlite3.
  --help | -h | (empty)              print_peer_help() peer.rs:227-234

dux doctor                           main.rs:50-57 → cli.rs:899-957.
                                     NO LOCK — read-only by design, must work
                                     while a TUI holds the lock.
  --json                             Merge external dux-amq-doctor --json output
                                     with a Rust-generated section
                                     (merge_doctor_json cli.rs:959-978).
  --anonymize                        Redact paths and identifiers; forwarded to
                                     the bash script (cli.rs:907-909, 934-936).
```

**Target resolution for `purge`** (`purge.rs:368-394`): session UUID → agent handle
→ branch name, in that order, with explicit rejection when a branch matches more
than one session (`:387-392`).

**Doctor script discovery** (`cli.rs:872-896`): `$DUX_AMQ_DOCTOR_BIN` (if the path
exists) → `which dux-amq-doctor` → `<exe_dir>/../dux-amq/scripts/dux-amq-doctor`.
If none resolves, dux emits its Rust-only section. In text mode the script owns
stdout so its TTY color detection works (`cli.rs:930-931`).

**Exit codes** (`session purge`, `cli.rs:118-152` help text): `0` success or
dry-run; `1` any cascade item errored; `2` invalid args, unknown target, or aborted
at confirmation.

**No global flags exist** — no `--config`, `--verbose`, or `-q`. The only
cross-cutting override is the `DUX_HOME` environment variable.

---

## 8. Open questions

1. **Is `resume_recovery.rs` intended as a permanent adapter?** If yes, CLAUDE.md's
   "No adapters, no protocol layer" tenet needs amending to something like "no
   adapters *for launching*; session-ID capture is provider-specific by necessity."
   If no, it needs the extraction-2 rewrite. This is a tenet-vs-reality decision
   only the owner can make, and it determines whether section 6 step 4 happens.
2. **Should the `purge.rs` provider-directory gap be treated as a compliance
   defect?** A user-added provider's chat history is never purged. Does
   `SECURITY.md` or `docs/operations/threat-model.md` already acknowledge this
   limitation, and does the `state_dirs` fix require a threat-model update in the
   same PR (per the CLAUDE.md rule on new attack surface)?
3. **Was `dux config reset --all`'s missing confirmation deliberate**, on the theory
   that the structural guards suffice? `dux session purge` sets the opposite
   precedent within the same binary.
4. **Should `[keys]` and `[macros]` keep `#[serde(flatten)]`?** It makes typo'd names
   representable. `validate_keys` catches them for `[keys]`, but `[macros]` has no
   equivalent validation — a misspelled macro key is silently accepted.
5. **Is `ui.syntax_theme` (H22) wanted**, or is the hardcoded `base16-ocean.dark` an
   accepted constraint? opaline already ships an unused syntect adapter that could
   derive it from the active theme automatically.
6. **How frequently does `BINDING_DEFS` churn?** If new actions are added often, the
   four-way `defs/*.rs` split risks merge conflicts at file boundaries; a single
   1,090-line table may be the lesser evil despite the line cap.
7. **What is the intended `right_width_pct` range** (H1)? The clamp says 50, the
   documentation says 80. Fixing the clamp changes layout behavior for anyone who
   set a value above 50 and never noticed it was being ignored; fixing the docs
   admits a smaller range than intended.
8. **Should the three raw-`KeyCode` modals (3.4) join the binding system**, or is
   the "a broken binding can't trap the user" rationale (`app/input.rs:1835`) meant
   to apply to all of them? If the latter, that rationale should be written down at
   each site and the labels made deliberately static.

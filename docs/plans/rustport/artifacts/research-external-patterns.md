# External research: patterns, memory, crates and modularity tooling for the dux overhaul

Compiled 2026-08-31 from four parallel research sub-agents plus direct source/registry verification.
Every claim carries a URL. Crate versions were read from the crates.io JSON API or docs.rs on
2026-08-31, not from memory. Anything that could not be confirmed is in `## Unverified`.

**Calibration facts supplied by the coordinator** (treated as given, not re-verified here):
toolchain pinned to Rust 1.88.0 while current stable is 1.98.0; dux already has an `Action` enum
with 80 variants and a declarative binding table; the three monoliths are a **3,109-line render
match**, a **1,100-line input `if let` cascade**, and a **1,055-line event-drain match**;
dev-dependencies are only `filetime` and `serial_test`; `ratatui::backend::TestBackend` is already
used in 7 places; each live PTY pane retains an alacritty grid estimated at 20–51 MiB.

---

## Answers to the two priority questions, up front

### 1. `clap` multicall is real, stable, and the right mechanism for `claude-amq` / `codex-amq` / `gemini-amq`

`Command::multicall(true)` exists and has been **stable since clap 3.1.0 (2022-02-16)** — the
changelog entry reads *"Command::multicall is now stable for busybox-like programs and REPLs
(#2861, #3684)"*
([CHANGELOG](https://github.com/clap-rs/clap/blob/master/CHANGELOG.md)). It carries through all of
4.x; current release is **clap 4.6.6 (2026-08-06)**
([docs](https://docs.rs/clap/latest/clap/struct.Command.html#method.multicall)).

Mechanism: with `multicall(true)`, clap stops treating `argv[0]` as the program name and instead
parses it as **the first subcommand name**. So one binary, hardlinked or symlinked as `claude-amq`,
`codex-amq`, `gemini-amq`, dispatches to the matching subcommand automatically; invoking the real
binary as `amq claude ...` works too if you register both spellings. Upstream ships two reference
examples: `examples/multicall-busybox.rs` and `examples/multicall-hostname.rs`.

Trade-offs against the alternatives, so the choice is deliberate:

| Approach | Wins when | Costs |
|---|---|---|
| `multicall(true)` | many small applets from one shipped artifact (a bash script collection maps well) | help/error text no longer shows a program name in the usual slot; top-level `--help` lists applets not flags; you must create symlinks at install time |
| Multiple `[[bin]]` targets | 2–4 genuinely distinct tools | N binaries on disk, N link times, N `main`s |
| Manual `argv[0]` dispatch (`args_os().next()` → `Path::file_name()` → match) | 2–3 names, total control over help text | ~15 lines you own; no clap semantics to lean on |

**Recommendation for dux:** `multicall(true)` for the `*-amq` wrapper family (they are variations on
one program, which is exactly the busybox shape), plus one conventional `[[bin]]` for the primary
`amq`/`dux` entry point. Pair with `clap_complete` 4.6.9 and `clap_mangen` 0.3.3 (both 2026-08) —
no competing parser has equivalents.

### 2. Module-tree tooling and the max-file-length gate: what exists vs. what you must write

**Exists and is usable today:**

- **`cargo-modules` 0.27.0 (2026-08-03)** — `cargo modules structure` prints a real ASCII module
  tree (`--focus-on <path>`, `--max-depth`, `--no-fns/--no-traits/--no-types`, `--sort-by`);
  `cargo modules dependencies` emits a **DOT** graph of intra-crate module dependencies with
  `--acyclic` (cycle detection, non-zero exit); `cargo modules orphans --deny` fails CI on source
  files that fell out of the module tree — invaluable during a 140-file split.
  ([repo](https://github.com/regexident/cargo-modules),
  [crates.io](https://crates.io/api/v1/crates/cargo-modules)). It resolves names through
  rust-analyzer internals, so it is accurate, not regex-based.
- **`rust-analyzer scip` / `rust-analyzer lsif`** — both subcommands exist and emit
  definition-and-reference indexes
  ([CLI docs](https://rust-lang.github.io/rust-analyzer/rust_analyzer/cli/index.html),
  [PR #17900](https://github.com/rust-analyzer/rust-analyzer/pull/17900)). SCIP output *could* drive
  a reverse-dependency generator. Nothing off-the-shelf does.
- **`ra_ap_hir` 0.0.350 (updated 2026-08-31)** — rust-analyzer internals published as real crates
  ([crates.io](https://crates.io/api/v1/crates/ra_ap_hir)). Zero-version, no API stability
  guarantee; pin exactly. This is how cargo-modules resolves names.
- **`syn` 3.0.4 (2026-08-24)** ([crates.io](https://crates.io/crates/syn)) for a cheap parser-only
  generator.
- **`dep_graph_rs` 0.2.0 (2025-07-20)** — the closest existing thing: parses with `syn`, analyses
  `use crate::…`, emits DOT. But 58 recent downloads, no reverse-dep mode, and *plain/ASCII output
  is a TODO* ([repo](https://github.com/PSeitz/dep_graph_rs)). Treat as a reference implementation
  to copy, not a dependency.

**Does not exist — must be written:**

- Reverse-dependency ("who uses me") trees. `cargo-modules` has no such flag.
- Splicing generated trees into file headers, and a `--check` mode that diffs them.
- **Any Rust lint for FILE length.** `clippy::too_many_lines` is **functions only**, default
  threshold **100**, key `too-many-lines-threshold`
  ([lint configuration](https://doc.rust-lang.org/clippy/lint_configuration.html)). A
  `clippy::too_many_lines_in_file` was proposed in
  [issue #16674](https://github.com/rust-lang/rust-clippy/issues/16674) /
  [PR #16675](https://github.com/rust-lang/rust-clippy/pull/16675) (opened 2026-03-06, restriction
  group, default 1000) but the PR is **still open with merge conflicts flagged 2026-08-01 and no
  approval**. Do not plan around it landing.

**The precedent for writing it yourself is the Rust project itself.** `src/tools/tidy/src/style.rs`
hardcodes `const LINES: usize = 3000;`, errors with *"too many lines ({lines}) (add
`// ignore-tidy-file-filelength` to the file to suppress this error)"*, and supports a per-file
opt-out directive
([style.rs](https://github.com/rust-lang/rust/blob/master/src/tools/tidy/src/style.rs)). rustc uses
a custom script, not a lint. So should dux.

Minimal CI gate:

```bash
#!/usr/bin/env bash
# ci/check-file-length.sh — fail if any tracked .rs file exceeds MAX lines.
set -euo pipefail
MAX=${MAX:-500}
fail=0
while IFS= read -r f; do
  head -n 5 "$f" | grep -q 'allow-long-file' && continue   # rustc-tidy-style opt-out
  n=$(wc -l < "$f")
  if (( n > MAX )); then
    printf '%s: %d lines (max %d)\n' "$f" "$n" "$MAX" >&2
    fail=1
  fi
done < <(git ls-files '*.rs')
exit $fail
```

Wire it as a GitHub Actions step and a pre-commit hook, and add
`cargo modules orphans --deny --bin dux` beside it.

**Recommended header-comment format** (rustdoc-safe — see the doctest rule below):

```rust
//! Renders the diff overlay pane.
//!
//! Owns the syntect-highlighted diff view and its scroll state. Pure: every
//! call must be cheap enough to run inside one frame. No git I/O here — the
//! caller supplies an already-computed `FileDiff`.
//!
//! # Uses
//!
//! ```text
//! ui::diff_overlay
//! ├── dux_core::theme          (semantic colours)
//! ├── dux_core::model::FileDiff
//! └── dux_widgets
//!     ├── scrollbar
//!     └── gutter
//! ```
//!
//! # Used by
//!
//! ```text
//! ui::diff_overlay
//! ├── ui::panes::center        (renders it when DiffOverlay is active)
//! └── app::render              (modal stack dispatch)
//! ```
```

**Why exactly this form, and why the obvious alternatives break `cargo test`:**

1. An **unannotated** ` ``` ` fence in a doc comment **is a Rust doctest**. The rustdoc book:
   *"by default, if no language is set for the block code, rustdoc assumes it is Rust code"* — ` ``` `
   is *"strictly equivalent to"* ` ```rust `
   ([documentation-tests](https://doc.rust-lang.org/rustdoc/write-documentation/documentation-tests.html)).
   A tree containing `├──` will not parse as Rust and the doctest fails to compile.
2. A **4-space-indented block is also a code block, hence also a doctest**, and — critically —
   *"there is no way to use attributes such as `ignore` or `should_panic` with indented code
   blocks."* So an indented tree **cannot be escaped**. This is a known open footgun:
   [rust#100225](https://github.com/rust-lang/rust/issues/100225),
   [rust#94757](https://github.com/rust-lang/rust/issues/94757). **Never indent the tree.**
3. The fix is the **`text`** annotation. `text` is not a rustdoc attribute, so the block is not
   treated as Rust, not compiled, not tested. `ignore` also works but still syntax-highlights as
   Rust and shows a "not tested" marker; `no_run` does **not** work — it still compiles.
   **Use ` ```text `.**
4. Safety net: **`rustdoc::invalid_rust_codeblocks` is warn-by-default** and fires on *"Rust code
   blocks in documentation examples that are invalid (e.g. empty, not parsable as Rust)"*
   ([rustdoc lints](https://doc.rust-lang.org/rustdoc/lints.html)) — so a forgotten annotation
   surfaces at `cargo doc` time even if doctests never run.
5. **Caveat specific to a binary crate:** doctests are extracted **from the library target only** —
   *"Documentation tests are run by default and handled by rustdoc, which extracts code samples from
   documentation comments of the library target"*
   ([cargo targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html)). If dux stays a
   pure `src/main.rs` binary, `cargo test` runs zero doctests and a malformed fence goes unnoticed
   locally — until you add a `lib` target. The workspace split recommended below adds lib targets,
   so this becomes live. Use `text` from day one.

`//!` (not `//`) is correct for the header: the rustdoc book says *"Lines should start with `//!`
which indicate module-level or crate-level documentation"*
([how-to-write-documentation](https://doc.rust-lang.org/rustdoc/how-to-write-documentation.html)).
Enforce presence with **`clippy::missing_docs_in_private_items`** (restriction group; covers private
modules, which is what a binary crate mostly has) rather than rustc's `missing_docs`, which is
public-items-only ([allowed-by-default lints](https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html)).
**No lint can verify the tree exists or is accurate** — that is the custom generator's job.

Note for expectation-setting: the per-file-header-with-dependency-tree convention is a **house
style, not an ecosystem norm**. The Rust API Guidelines mandate only crate-level docs (`C-CRATE-DOC`)
([api-guidelines](https://rust-lang.github.io/api-guidelines/documentation.html)); rust-analyzer's
style guide (now at `docs/book/src/contributing/style.md`) covers imports, ordering and naming but
says **nothing** about file headers or file-size limits. The one published precedent found is
Microsoft Project Mu: *"Module documentation should be placed at the top of a module, whether that
be a mod.rs file or the module itself if contained to a single file"*
([Mu doc conventions](https://microsoft.github.io/mu/CodeDevelopment/rust_documentation_conventions/)).
The mainstream alternative is matklad's `ARCHITECTURE.md` — one root codemap, *"a map of a country,
not an atlas of maps of its states"*, naming important files without linking them because *"links go
stale"* ([ARCHITECTURE.md](https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html)). **Do both:**
the generated per-file trees give local navigation, ARCHITECTURE.md gives the bird's-eye view and
the architectural invariants, which no generator can infer.

---

## Architecture patterns

### What the ratatui docs actually prescribe (and don't)

ratatui documents three patterns and endorses none:

- **The Elm Architecture** — Model / Message / Update / View. Named trade-offs on the page:
  immutability vs. performance (mutable references are sanctioned when needed); immediate-mode
  rendering means the view only learns the drawable area at render time, causing frame lag on
  resize; and `StatefulWidget` forces you to abandon strict view purity.
  <https://ratatui.rs/concepts/application-patterns/the-elm-architecture/>
- **Component Architecture** — `init` / `handle_events` / `handle_key_events` /
  `handle_mouse_events` / `update` / `render`. Stated benefit: *"This approach incentivizes
  co-locating the `handle_events`, `update` and `render` functions on a component level."* The page
  lists **no downsides**.
  <https://ratatui.rs/concepts/application-patterns/component-architecture/>
- **Flux** — dispatcher / stores / actions / views, unidirectional flow. No trade-offs discussed.
  <https://ratatui.rs/concepts/application-patterns/flux-architecture/>

The event-handling concept page is explicitly non-prescriptive: *"the correct way, is the one that
works for you and your current application"*, and it names centralized handling as the thing that
*"doesn't scale well since all events are handled in one place"* — which is precisely dux's three
monoliths. <https://ratatui.rs/concepts/event-handling/>

The official **component template** (`cargo generate ratatui/templates`) produces
`src/{action,app,cli,components,config,errors,logging,main,tui}.rs` plus `src/components/{fps,home}.rs`
([tree](https://github.com/ratatui/templates)). Its `Component` trait:

```rust
pub trait Component {
    fn register_action_handler(&mut self, tx: UnboundedSender<Action>) -> Result<()> { Ok(()) }
    fn register_config_handler(&mut self, config: Config) -> Result<()> { Ok(()) }
    fn init(&mut self, area: Size) -> Result<()> { Ok(()) }
    fn handle_events(&mut self, event: Option<Event>) -> Result<Option<Action>> { … }
    fn handle_key_event(&mut self, key: KeyEvent) -> Result<Option<Action>> { Ok(None) }
    fn handle_mouse_event(&mut self, mouse: MouseEvent) -> Result<Option<Action>> { Ok(None) }
    fn update(&mut self, action: Action) -> Result<Option<Action>> { Ok(None) }
    fn draw(&mut self, frame: &mut Frame, area: Rect) -> Result<()>;
}
```
([components.rs](https://raw.githubusercontent.com/ratatui/templates/main/component/template/src/components.rs))
with `enum Action { Tick, Render, Resize(u16,u16), Suspend, Resume, Quit, ClearScreen, Error(String), Help }`
([action.rs](https://raw.githubusercontent.com/ratatui/templates/main/component/template/src/action.rs)).

This is the shape dux already half-has. The template is a *starting* structure, not a 70k-LOC one.

### Real large-app structures (measured, not asserted)

Sizes from the GitHub git-trees API on 2026-08-31.

| Project | Layout | Files | Largest `.rs` | Notes | URL |
|---|---|---|---|---|---|
| **yazi** | **31 workspace crates** (`yazi-actor`, `yazi-parser`, `yazi-fm`, `yazi-widgets`, `yazi-core`, `yazi-config`, `yazi-shared`, `yazi-macro`, …) | `yazi-actor` alone: 91 `.rs` | ~8.9 KB (`yazi-binding/src/elements/line.rs`) | **The one project that actually achieves a ~500-line cap.** `yazi-actor/src/mgr/` = **64 files**, 369–8,508 bytes each, median ≈ 800 B (~25 lines). `yazi-parser/src/mgr/` mirrors it with **54** param-struct files, 475–1,510 B each. | [repo](https://github.com/sxyazi/yazi), [Cargo.toml](https://raw.githubusercontent.com/sxyazi/yazi/main/Cargo.toml) |
| **gitui** | 6 crates (`gitui`, `asyncgit`, `filetreelist`, `git2-hooks`, `git2-testing`, `scopetime`) + `src/{components,popups,tabs}/` | ~1,229-line `app.rs` | 44 KB `src/strings.rs` (a string table), 30.8 KB `src/app.rs` | Wide-but-shallow root: `App` has ~40 component fields, so its width is a *routing table*, not behaviour. `src/popups/` is one file per modal (13–19 KB each). | [repo](https://github.com/gitui-org/gitui) |
| **television** | single crate | ~40 `.rs` in `television/` | **51 KB `television.rs`**, 33 KB `app.rs` | **Counter-example.** Popular, current, and still monolithic. | [repo](https://github.com/alexpasmantier/television) |
| **bottom** | single crate, `src/{app,canvas,collection,components,options,widgets}/` | 68 `.rs` | **101 KB `src/app.rs`**, 63 KB `widgets/process_table.rs` | **Counter-example.** Directory structure alone does not produce small files. | [repo](https://github.com/ClementTsang/bottom) |
| **zellij** | workspace (`zellij-server`, `zellij-client`, `zellij-utils`, …) | — | — | Best PTY-hosting reference. **1,118 `.snap` files**; a `fake_pty.rs` behind a trait so UI logic is tested without real subprocesses. | [repo](https://github.com/zellij-org/zellij) |

**The finding that matters:** among large ratatui apps, *directory structure does not correlate with
file size* — bottom and television are well-organised and still have 50–100 KB files. Only **yazi's
workspace-crate + one-unit-per-file discipline** actually produces the cap dux wants. Compiler-enforced
crate boundaries are what make the discipline stick.

### How to decompose dux's three specific monoliths

#### (a) The 3,109-line render match

Two verified mechanisms, both applicable:

- **ratatui 0.30 implements `Widget` for `&T`** on every built-in widget. Docs: *"Starting with
  Ratatui 0.26.0, all the internal widgets implement Widget for a reference to themselves. This
  allows you to store a reference to a widget and render it later"*, and *"In general where you
  expect a widget to immutably work on its data, we recommended to implement `Widget` for a
  reference to the widget (`impl Widget for &MyWidget`)"*
  ([Widget docs, 0.30.2](https://docs.rs/ratatui/0.30.2/ratatui/widgets/trait.Widget.html)). The
  trait is still `fn render(self, area: Rect, buf: &mut Buffer) where Self: Sized`, so `&T` impls
  are the supported way to render without consuming.
  **Do not reach for `WidgetRef`** — in 0.30.2 it is still gated behind the `unstable-widget-ref`
  feature and documented as *"marked as unstable … no stability guarantees, and could be changed or
  removed at any time"*
  ([WidgetRef docs](https://docs.rs/ratatui/0.30.2/ratatui/widgets/trait.WidgetRef.html)).
- **gitui's split trait pair** is the proven in-the-large version:
  ```rust
  pub trait DrawableComponent { fn draw(&self, f: &mut Frame, rect: Rect) -> Result<()>; }
  ```
  separate from the event-handling `Component` trait
  ([components/mod.rs](https://github.com/gitui-org/gitui/blob/master/src/components/mod.rs)).
  Splitting draw from event means a render file never needs to know about actions.

**Concretely:** the 3,109-line match becomes one `impl Widget for &X` (or `DrawableComponent`) per
pane and per modal, one file each, plus a ~40-line dispatch that walks the modal stack. gitui's
`src/popups/` (one file per modal, 13–19 KB) is the direct precedent, and yazi's
`yazi-widgets`/`yazi-fm` split is the aggressive version.

#### (b) The 1,100-line input `if let` cascade

dux already has the binding table, so the missing half is **table → handler dispatch without a
match**. Three verified mechanisms:

- **helix's keymap** resolves chords through a trie, not a cascade:
  ```rust
  pub enum KeyTrie { MappableCommand(MappableCommand), Sequence(Vec<MappableCommand>), Node(KeyTrieNode) }
  pub struct KeyTrieNode { name: String, map: IndexMap<KeyEvent, KeyTrie>, pub is_sticky: bool }
  pub enum KeymapResult { Pending(KeyTrieNode), Matched(MappableCommand), MatchedSequence(Vec<MappableCommand>), NotFound, Cancelled(Vec<KeyEvent>) }
  pub fn get(&mut self, mode: Mode, key: KeyEvent) -> KeymapResult
  ```
  Pending keys live in `state: Vec<KeyEvent>`; sticky nodes persist a submenu.
  ([keymap.rs](https://raw.githubusercontent.com/helix-editor/helix/master/helix-term/src/keymap.rs)).
  Commands come from a **static table**, not a match:
  ```rust
  pub enum MappableCommand {
      Typable { name: String, args: String, doc: String },
      Static  { name: &'static str, fun: fn(cx: &mut Context), doc: &'static str },
      Macro   { name: String, keys: Vec<KeyEvent> },
  }
  static_commands!( no_op, "Do nothing", move_char_left, "Move left", … );
  ```
  The macro generates `pub const`s plus `STATIC_COMMAND_LIST: &'static [Self]`, and `FromStr` looks
  a name up in that list. ~180 static commands, each a plain `fn(&mut Context)`.
  ([commands.rs](https://raw.githubusercontent.com/helix-editor/helix/master/helix-term/src/commands.rs))
- **yazi's `Actor` trait** is the cleanest one-file-per-command form and the single most transplantable
  pattern here:
  ```rust
  pub trait Actor {
      type Form;
      const NAME: &str;
      fn act(cx: &mut Ctx, form: Self::Form) -> Result<Data>;
      fn hook(_cx: &Ctx, _form: &Self::Form) -> Option<SparkKind> { None }
  }
  ```
  ([actor.rs](https://raw.githubusercontent.com/sxyazi/yazi/main/yazi-actor/src/actor.rs)). A whole
  command, verbatim, is 20 lines:
  ```rust
  pub struct Arrow;
  impl Actor for Arrow {
      type Form = ArrowForm;
      const NAME: &str = "arrow";
      fn act(cx: &mut Ctx, form: Self::Form) -> Result<Data> {
          let tab = cx.tab_mut();
          let old = tab.current.cursor;
          if !tab.current.arrow(form.step) { succ!(); }
          tab.current.retrace();
          if let Some(visual) = tab.mode.visual_mut() { visual.arrow(form.step, old, tab.current.cursor); }
          act!(mgr:hover, cx)?;  act!(mgr:peek, cx)?;  act!(mgr:watch, cx).ok();
          cx.tasks.scheduler.behavior.reset();
          succ!(render!());
      }
  }
  ```
  ([arrow.rs](https://raw.githubusercontent.com/sxyazi/yazi/main/yazi-actor/src/mgr/arrow.rs)).
  Dispatch is **compile-time**, via a macro that maps `layer:name` to a type — no runtime match at
  all, plus a debug-only call backtrace:
  ```rust
  ($layer:ident : $name:ident) => { paste::paste! { yazi_actor::$layer::[<$name:camel>] } };
  (@impl $layer:ident : $name:ident, $cx:ident, $opt:ident) => {{
      $cx.level += 1;
      #[cfg(debug_assertions)] $cx.backtrace.push(concat!(stringify!($layer), ":", stringify!($name)));
      let result = match $crate::act!(@pre $layer:$name, $cx, $opt) {
          Ok(opt) => <$crate::act!($layer:$name) as yazi_actor::Actor>::act($cx, opt),
          Err(e)  => Err(e),
      };
      $cx.level -= 1;
      #[cfg(debug_assertions)] $cx.backtrace.pop();
      result
  }};
  ```
  ([yazi-macro/src/actor.rs](https://raw.githubusercontent.com/sxyazi/yazi/main/yazi-macro/src/actor.rs))
  Bindings are pure data — `{ on = [ "g", "g" ], run = "arrow top", desc = "Go to top" }`,
  `{ on = "<C-a>", run = "toggle_all --state=on", … }`, `{ on = [",","m"], run = ["sort mtime --reverse=no","linemode mtime"], … }`
  across sections `[mgr] [tasks] [spot] [pick] [input] [confirm] [cmp] [help]`
  ([keymap-default.toml](https://raw.githubusercontent.com/sxyazi/yazi/main/yazi-config/preset/keymap-default.toml)).
  Note the per-context sections — that is how yazi scopes bindings without a mode `match`.
- **gitui's consumed-bubbling** for the cases a table can't express:
  ```rust
  pub enum EventState { Consumed, NotConsumed }
  pub enum CommandBlocking { Blocking, PassingOn }
  fn event(&mut self, ev: &Event) -> Result<EventState>;
  ```
  plus `commands(&self, out: &mut Vec<CommandInfo>, force_all: bool) -> CommandBlocking`, which
  doubles as the source of the help/footer hint bar — the components declare their own bindings
  rather than a central list going stale
  ([components/mod.rs](https://github.com/gitui-org/gitui/blob/master/src/components/mod.rs)).
  gitui enumerates components exactly once for both draw and event via `accessors!`/`event_pump!`
  macros generating `components()`/`components_mut()`, and its module docs are explicit that this is
  *composition by code*, not by data.

#### (c) The 1,055-line event-drain match

Same registry treatment. Worker/PTY events become `Action`s and go through the identical dispatch,
so there is one reducer, not two. gitui's split is the model: a **synchronous `Queue`** for
intra-frame intent (`Rc<RefCell<VecDeque<InternalEvent>>>` behind a 4-method `push`/`pop`/`clear`
wrapper, so no borrow is ever held across a call) and **`crossbeam-channel` senders** only for
cross-thread completion (`sender_git`, `sender_app`)
([queue.rs](https://github.com/gitui-org/gitui/blob/master/src/queue.rs)). That is directly
transplantable to dux's worker events.

#### The 80-variant `Action` enum: nest by routing target, and measure it

- gitui does not have one enum — it has `InternalEvent` (~45 variants, the app bus) and `Action`
  (16 variants, **only** destructive ops needing confirmation), joined by
  `InternalEvent::ConfirmAction(Action)` / `ConfirmedAction(Action)`, plus `StackablePopupOpen`
  nested under `OpenPopup(_)` and `AppTabs` under `TabSwitch(_)`. The whole file is **193 lines**.
  Nesting is applied by **shared lifecycle**, not by topic.
- The strongest statement of *why* to nest is the Iced multipage howto:
  *"Each page has its own enum over its messages. A page is only allowed to send and recieve messages
  in that enum … There should not ever be a case where a page would send or recieve any messages that
  aren't contained within its variant in the global message."*
  ([howto-iced-multipage](https://github.com/max-ishere/howto-iced-multipage)) — i.e. "modal X cannot
  emit modal Y's action" becomes a compile-time fact.
- The counter-argument is real and worth respecting:
  [rust-lang/rfcs#3385](https://github.com/rust-lang/rfcs/issues/3385) notes nesting *"would add
  majorly to the amount of noise required to perform an exhaustive match"* and cannot express a
  variant belonging to two overlapping groups.
- **Size discipline:** an enum is as large as its largest variant.
  `clippy::large_enum_variant` is **warn-by-default**, threshold `enum-variant-size-threshold`
  **default 200 bytes**; `result_large_err` uses `large-error-threshold` **default 128**
  ([lint configuration](https://doc.rust-lang.org/clippy/lint_configuration.html)). rustc also has
  `variant_size_differences`, *"detects enums with widely varying variant sizes"*, **allow by
  default** — opt in ([allowed-by-default lints](https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html)).
  With 80 variants, run `RUSTFLAGS=-Zprint-type-sizes cargo +nightly build --release | top-type-sizes -w 120`
  and box any variant carrying `String` + `PathBuf` + `Vec`. The perf-book threshold to remember:
  types over **128 bytes** are copied by `memcpy` rather than inline code
  ([type sizes](https://nnethercote.github.io/perf-book/type-sizes.html)).

#### State ownership: the partial-borrow trap that will decide whether the split works

This is the single most actionable finding for dux's existing `state/` sub-structs.

> *"`&mut self` always exclusively borrows all of `self`, including all its fields at once. This is
> intentional, because functions are meant to be an abstraction boundary."*
> — [users.rust-lang.org](https://users.rust-lang.org/t/cannot-borrow-self-as-mutable-because-an-unrelated-field-is-already-borrowed/56241)

Borrow-splitting works *inside* a function body but **cannot cross a method boundary**. So
`RuntimeState`/`UiState`/`GitState` only pay off if methods **move onto the sub-structs**
(`impl UiState { … }`) or become free functions taking `(&mut UiState, &GitState)`. Keeping
`impl App { fn …(&mut self) }` and reaching through `self.ui.x` recovers **none** of the benefit —
it is the same god-object with extra dots.

- The language-level fix (**view types**, `#![feature(view_types)]`) is
  [tracking issue #155938](https://github.com/rust-lang/rust/issues/155938), opened 2026-04-28,
  `B-experimental`, RFC not approved, **borrow-checker work still in progress**. Do not plan around it.
- The live crate is **`borrow` 2.0.0 (2025-10-07)**, *"Zero-overhead, safe implementation of partial
  borrows"* ([repo](https://github.com/wdanilo/borrow)). `partial-borrow` 1.0.1 is dormant (2022) and
  **GPL-3.0-or-later** — a licence blocker.
- On `Rc<RefCell<_>>`: it is not forbidden, but scope every borrow to one statement. `RefCell::borrow`
  *"Panics if the value is currently mutably borrowed"*
  ([std docs](https://doc.rust-lang.org/std/cell/struct.RefCell.html)), and a TUI's
  render-during-event-handling reentrancy is exactly where that fires. gitui's 4-method Queue wrapper
  is the discipline that makes it safe. In the canonical thread on this
  ([TUI design](https://users.rust-lang.org/t/trying-to-wrap-my-head-around-tui-design/104736)),
  parasyte calls a manager-in-`Rc<RefCell<_>>` App *"your first problem"* and diagnoses borrow pain as
  usually signalling *"a circular dependency or non-DAG structure"*; kpreid states the routing rule:
  *"Widgets shouldn't be communicating to other widgets. They should be writing to application
  data/state storage."*
- The proven middle path is a **context struct**: gitui's `Environment`
  (`{ queue, theme, key_config, repo, options, sender_git, sender_app }`, built once, handed to every
  component's `new()`, and carrying a `#[cfg(test)] fn test_env()` — the context struct *is* the test
  seam), and yazi's `Ctx` passed as `&mut Ctx` into every `Actor::act`.

### Recommended structure for dux

Flat workspace, following matklad: *"for projects in between ten thousand and one million lines of
code, the flat layout makes the most sense"*, *"even comparatively large lists are easier to
understand at a glance than even small trees"*, *"flat structure doesn't need maintenance"*; crate
name == folder name; root is a **virtual manifest**; internal crates get `version = "0.0.0"`
([Large Rust Workspaces](https://matklad.github.io/2021/08/22/large-rust-workspaces.html)). Also
relevant: *"The basic unit of compilation is a crate, not a module"* — module-splitting alone buys
**zero** compile-time improvement; only crate-splitting parallelises and limits recompiles
([Rust at scale](https://mmapped.blog/posts/03-rust-packages-crates-modules)).

```text
dux/                          (virtual manifest)
├── crates/
│   ├── dux-shared/           errors, sanitize, small utils      (leaf; depended on by all)
│   ├── dux-core/             model, SessionState, Config, theme
│   ├── dux-git/              git plumbing (porcelain-free), diff computation
│   ├── dux-pty/              portable-pty + alacritty_terminal grid hosting, scrollback policy
│   ├── dux-parser/           one param-struct file per Action  (yazi-parser analogue)
│   ├── dux-actor/            one handler file per Action        (yazi-actor analogue)
│   ├── dux-widgets/          reusable ratatui widgets: scrollbar, gutter, status line
│   ├── dux-ui/               panes + one file per modal (gitui src/popups analogue)
│   ├── dux-app/              Ctx, event loop, keymap trie, worker plumbing
│   ├── dux-amq/              the ported bash layer (lib + multicall bin)
│   └── dux-macro/            act!/succ!/render! dispatch macros
└── src/main.rs               thin bin: parse CLI, ratatui::init(), call dux_app::run
```

The compiler enforces layering because the crate graph must be acyclic. Visibility discipline
enforces the rest: turn on **`unreachable_pub`** (*"triggers for `pub` items not reachable from other
crates"*) and **`unnameable_types`**, both allow-by-default rustc lints
([listing](https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html)). Skip
`clippy::redundant_pub_crate` — it is nursery, has macro false positives, and **directly conflicts
with `unreachable_pub`** ([clippy#5369](https://github.com/rust-lang/rust-clippy/issues/5369),
[#8732](https://github.com/rust-lang/rust-clippy/issues/8732)).

### Event/action flow

```text
crossterm event ─┐
                 ├─► KeyTrie lookup, scoped by (pane, mode)   [helix keymap.rs]
worker/PTY event ┘        │
                          ├─ Pending(node)   → show which-key overlay, keep state
                          ├─ NotFound        → bubble: pane → parent → global  [gitui EventState]
                          └─ Matched(action) → Action (nested per routing target)
                                                    │
                                       act!(layer:name, cx, action)   [yazi-macro, compile-time]
                                                    │
                                       Actor::act(&mut Ctx, Form) -> Result<Outcome>
                                              one file per action, ≤ ~120 lines
                                                    │
                        Outcome ──► render? ──► modal-stack walk ──► impl Widget for &Pane/&Modal
                                                                     one file per pane/modal
```

Rules that keep it decomposed: handlers never touch ratatui; widgets never touch git, PTY or the
action bus; the `Ctx` is passed, never stored; every long operation is an `Action` emitted from a
worker thread, never a blocking call in a handler.

### Testing the decomposition: yes, add `insta`

**Recommendation: add `insta` 1.48.0 (2026-06-11) as a dev-dependency and keep `TestBackend`.** They
are complementary — insta asserts *on* the `TestBackend` buffer.

- It is the officially documented approach. ratatui's recipe
  (<https://ratatui.rs/recipes/testing/snapshots/>) shows exactly:
  ```rust
  let mut terminal = Terminal::new(TestBackend::new(80, 20)).unwrap();
  terminal.draw(|frame| frame.render_widget(&app, frame.area())).unwrap();
  assert_snapshot!(terminal.backend());
  ```
  Note `&app` — this is the same `impl Widget for &T` idiom recommended for the render split, so the
  testing story and the decomposition story reinforce each other.
- **Scale is the argument.** Splitting a 3,109-line render match into ~30 pane/modal widgets means
  writing ~30 expected buffers. Hand-rolled `assert_buffer_lines` at that count is prohibitive to
  write and worse to maintain; `cargo insta review` makes an intentional visual change a one-keystroke
  approval. Below ~5 widgets hand-rolled wins; at 30 it does not.
- **It is the strongest safety net for this specific refactor.** Snapshot every pane and modal
  *before* touching the render match, then require byte-identical snapshots after. That converts
  "did the split change rendering?" from a review question into a CI gate.
- **gitui is the template to copy**, including the nondeterminism handling — `insta` with the
  `filters` feature and an `apply_common_filters!` macro scrubbing temp paths and short SHAs:
  ```rust
  settings.add_filter(r" *\[…\]\S+-insta/?", "[TEMP_FILE]");
  settings.add_filter(r"│[a-z0-9]{7} ", "│[AAAAA] ");
  let _bound = settings.bind_to_scope();
  ```
  dux needs the same for spinner frames, timestamps, worktree paths and commit SHAs, or snapshots
  will flap.
- **zellij** proves it scales to a PTY host: 1,118 `.snap` files, split between e2e
  (`zellij__tests__e2e__cases__*.snap`) and per-module unit snapshots — enabled by workspace-crate
  decomposition *and* a `fake_pty.rs` behind a trait. **Put dux's PTY behind a trait with a fake for
  the same reason**; otherwise UI snapshots need real subprocesses and will be flaky.
- **Known limit:** the recipe states *"Asserting with color is not supported as of now"* — snapshots
  capture text layout only. Keep a handful of hand-rolled `Buffer` assertions for style/colour
  regressions (theme constants, selection highlight).
- Counter-datapoints for honesty: **bottom** uses no snapshots at all (integration tests over
  args/config/layout-movement only), and **yazi** has no `.snap` files and no top-level `tests/` dir.
  Snapshot testing is a choice, not a consensus — but it is the choice that de-risks *this* refactor.

`termlens` 0.8.0 (2026-08-29) — *"Headless PTY test harness for CLI/TUI apps — spawn in a real PTY,
assert on the rendered screen"*, with `wait_until`/`wait_idle`/`wait_exit` and
`insta::assert_snapshot!(t.screen())` — is a promising e2e complement
([repo](https://github.com/vyncint/termlens)), but at 1,518 downloads and 19 stars it is unproven.
Note that `ratatui-snapshot`, `ratatui-snapshots` and `ratatui-testing` on crates.io are
**0.0.0 name reservations** with nothing in them.

### Production readiness (verified 0.30.2 APIs)

- `ratatui::init() -> DefaultTerminal` already *"installs a panic hook that restores the terminal
  before panicking"*, and the docs warn to call it **after** any other panic hooks *"to ensure that
  the terminal is restored before those hooks are called"*
  ([init](https://docs.rs/ratatui/latest/ratatui/fn.init.html)). `restore()` disables raw mode and
  leaves the alternate screen, ignoring errors — *"generally the correct behavior when cleaning up
  before exiting"* ([restore](https://docs.rs/ratatui/latest/ratatui/fn.restore.html)). There is also
  `run<F, R>(f: F) -> R where F: FnOnce(&mut DefaultTerminal) -> R`, a wrapper around init+restore
  ([run](https://docs.rs/ratatui/0.30.2/ratatui/fn.run.html)).
- Manual hook, if you need to chain: `take_hook()` → `set_hook(|info| { let _ = restore(); hook(info); })`
  (<https://ratatui.rs/recipes/apps/panic-hooks/>, <https://ratatui.rs/recipes/apps/color-eyre/>).
- **The hook, not a `Drop` guard, is the reliable mechanism.** `std::panic::set_hook` is *"a global
  resource"* invoked *"when a thread panics, but before the panic runtime is invoked"*, and *"the hook
  will run with both the aborting and unwinding runtimes"*
  ([set_hook](https://doc.rust-lang.org/std/panic/fn.set_hook.html)) — so it fires for panics in PTY
  reader and worker threads too. A `Drop` guard does not run under `panic = "abort"` or
  `std::process::exit`.
- **Child-PTY shutdown.** POSIX `close()`: *"If fildes refers to the master side of a pseudo-terminal,
  and this is the last close, a SIGHUP signal shall be sent to the controlling process"*
  ([POSIX](https://pubs.opengroup.org/onlinepubs/9699919799/functions/close.html)) — but only if it
  really is the last fd, so watch for clones held by reader threads. `login_tty()` makes the child a
  session leader with its own controlling terminal
  ([login_tty(3)](https://man7.org/linux/man-pages/man3/login_tty.3.html)), so `kill(-pgid, …)` reaches
  grandchildren. `std::process::Child::kill()` does **not** — see
  [rust#115241](https://github.com/rust-lang/rust/issues/115241) and
  [rust#41822](https://github.com/rust-lang/rust/issues/41822).
  **zellij's contract is the model to copy:**
  ```rust
  /// Terminate the process with process ID `pid`. (SIGHUP)
  fn kill(&self, pid: u32) -> Result<()>;
  /// Terminate the process with process ID `pid`. (SIGKILL)
  fn force_kill(&self, pid: u32) -> Result<()>;
  ```
  SIGHUP → grace period → SIGKILL, per pane. `portable-pty`'s `ChildKiller::clone_killer()` exists
  precisely so you can signal *"independently from a thread that may be blocked in `.wait`"*
  ([ChildKiller](https://docs.rs/portable-pty/latest/portable_pty/trait.ChildKiller.html)).

---

## Memory playbook

Ranked by expected payoff against the stated 20–51 MiB/pane. Items 1–4 dominate everything else by an
order of magnitude; items 8+ are rounding error at dux's cardinality and should not consume refactor
budget.

### Why the number is 20–51 MiB — the mechanism, verified in source

`alacritty_terminal` 0.26.0 (2026-04-06, [crates.io](https://crates.io/api/v1/crates/alacritty_terminal)):

```rust
pub struct Cell { pub c: char, pub fg: Color, pub bg: Color, pub flags: Flags, pub extra: Option<Arc<CellExtra>> }
pub struct Row<T> { inner: Vec<T>, pub(crate) occ: usize }
const MAX_CACHE_SIZE: usize = 1_000;
struct Storage<T> { inner: Vec<Row<T>>, zero: usize, visible_lines: usize, len: usize }
```

**`Cell` is exactly 24 bytes** — upstream asserts it:
`const EXPECTED_CELL_SIZE: usize = 24; assert!(mem::size_of::<Cell>() <= EXPECTED_CELL_SIZE);`
([cell.rs](https://docs.rs/alacritty_terminal/0.26.0/src/alacritty_terminal/term/cell.rs.html),
[row.rs](https://docs.rs/alacritty_terminal/0.26.0/src/alacritty_terminal/grid/row.rs.html),
[storage.rs](https://docs.rs/alacritty_terminal/0.26.0/src/alacritty_terminal/grid/storage.rs.html)).

Per-line cost (derived from those definitions): 32 B inline `Row` header + `columns × 24` B heap,
with capacity == columns exactly (`Vec::with_capacity(columns)` then `set_len`), so no slack:

| width | bytes/line | 10,000 lines |
|---|---|---|
| 80 cols | 1,952 B | ~19 MB |
| 120 cols | 2,912 B | ~29 MB |
| 160 cols | 3,872 B | ~39 MB |
| 200 cols | 4,832 B | ~48 MB |

That range **matches the observed 20–51 MiB/pane exactly**, which confirms the diagnosis: the number
is scrollback, not leakage. It also matches an independent field report —
[alacritty#3650](https://github.com/alacritty/alacritty/issues/3650): 10,000 lines → **52.8 MB
(5.28 KB/line)**, 100,000 lines → **522.5 MB**; closed as `F - not a bug`.

**The default is the culprit:** `alacritty_terminal::term::Config { scrolling_history: usize, … }`
defaults to **10,000**
([term/mod.rs](https://raw.githubusercontent.com/alacritty/alacritty/master/alacritty_terminal/src/term/mod.rs),
[Config docs](https://docs.rs/alacritty_terminal/0.26.0/alacritty_terminal/term/struct.Config.html)).
`Term::new` passes it to the primary grid only; `inactive_grid` gets `Grid::new(lines, cols, 0)` —
the alt-screen grid carries zero history.

**Two behaviours that shape the fix:**

- Scrollback is **lazy but chunked**. `Storage::with_capacity` allocates only visible lines
  (*"Initialize visible lines; the scrollback buffer is initialized dynamically"*), but
  `initialize()` grows by `max(additional_rows, MAX_CACHE_SIZE)` with `MAX_CACHE_SIZE = 1_000`.
  **The first line that scrolls into history materialises 1,000 fully-allocated rows** — ~2.9 MB at
  120 cols, ~4.8 MB at 200 cols, per pane, instantly. So there is a hard floor of ~3–5 MB per pane
  that has ever scrolled, and setting `scrolling_history` below 1,000 saves nothing.
- Shrinking has the same hysteresis: `shrink_lines` only calls `truncate()` when
  `inner.len() > len + MAX_CACHE_SIZE`. `clear_history()` therefore returns memory but never below
  `len + 1000` rows.

### Ranked techniques

| # | Technique | Expected payoff for dux | Effort | Source |
|---|---|---|---|---|
| 1 | Expose `scrolling_history` as a dux config setting; default well below 10,000 (2,000 is a sane start) | **Largest single win.** 10k→2k at 120 cols: ~29 MB → ~5.8 MB **per pane**. 8 panes: ~233 MB → ~47 MB | Trivial — one `Config` field | [term Config](https://raw.githubusercontent.com/alacritty/alacritty/master/alacritty_terminal/src/term/mod.rs) |
| 2 | Drop the `Term` entirely for `Detached`/`Exited` sessions — fold the PTY+Term handle into `SessionState::Live` (already the planned phase 2) | Frees **100%** of that pane's grid. Highest payoff per pane for an orchestrator where most sessions are idle | Medium (lifecycle) | project `CLAUDE.md` lifecycle plan |
| 3 | `Grid::clear_history()` on backgrounded panes; reconstruct on reattach | Reclaims all but ~1,000 rows (~3–5 MB floor) | Low | [grid/mod.rs](https://docs.rs/alacritty_terminal/0.26.0/src/alacritty_terminal/grid/mod.rs.html) |
| 4 | Cap effective width, or make history a *byte* budget rather than a line count | Cost is `lines × cols × 24`; a 200-col pane costs 2.5× an 80-col one for the same line count | Low | derived from `Row::new` |
| 5 | `mimalloc` as `#[global_allocator]`, behind a Cargo feature so it can be A/B'd | Fragmentation win in a many-thread process; 2 lines of code | Trivial | [mimalloc](https://github.com/microsoft/mimalloc) |
| 6 | `MALLOC_ARENA_MAX=2` in the Linux launcher + optional `malloc_trim(0)` after closing a pane | dux spawns worker threads; glibc spawns an arena per contended thread. Reported spread from tuning this: **~976 MB … 4,425 MB**, best at 2 | Trivial / Low | [gotplt.org](https://gotplt.org/posts/malloc-per-thread-arenas-in-glibc.html), [arkey.fr](https://blog.arkey.fr/drafts/2021/01/22/native-memory-fragmentation-with-glibc/) |
| 7 | Lazy-init syntect in a `OnceLock`, built on first diff render rather than at boot | Removes syntect entirely from panes-only sessions | Low | [SyntaxSet](https://docs.rs/syntect/5.3.0/syntect/parsing/struct.SyntaxSet.html) |
| 8 | Bound the diff / change-file caches per session (`similar` output, `GitState` caches) | Unbounded today; grows with session count | Low | project structure |
| 9 | `-Zprint-type-sizes` on the 80-variant `Action` and on worker-event enums; box fat variants | Every message costs the largest variant | Low | [perf-book](https://nnethercote.github.io/perf-book/type-sizes.html) |
| 10 | Turn on `variant_size_differences`; confirm `large_enum_variant` is not `allow`ed anywhere | Free, already gated by `-D warnings` | Trivial | [clippy config](https://doc.rust-lang.org/clippy/lint_configuration.html) |
| 11 | `Box<str>` / `Box<[T]>` for frozen strings and vecs; `Arc<Config>` to workers | 1 word each; single-digit MB at best | Low | [perf-book heap](https://nnethercote.github.io/perf-book/heap-allocations.html) |
| 12 | Trim syntect to a custom `SyntaxSet` dump of only the languages dux highlights | Cuts binary and resident syntax data | Medium | syntect `dump_to_file`/`from_dump_file` |
| 13 | `PRAGMA cache_size = -512` if dux ever opens a connection per worker | ~2 MB default per connection | Trivial | [sqlite pragma](https://www.sqlite.org/pragma.html#pragma_cache_size) |
| 14 | Arena/slab (`slab`, `slotmap`) for session handles | **~Zero.** Tens of sessions, not millions of nodes | — | — |
| 15 | String interning (`lasso`, `ustr`), compact strings | **~Zero** at dux's cardinality; paths usually exceed compact_str's 24 B inline anyway | — | [CompactString](https://docs.rs/compact_str/0.10.0/compact_str/struct.CompactString.html) |

### A grid hazard worth fixing on the way past

In 0.26.0, `CellExtra.zerowidth` is a plain `Vec<char>` — unbounded per cell. Current alacritty master
replaced it with `ArrayVec<char, MAX_ZEROWIDTH_CHARS>` (`= 9`) with the comment: *"This enforces an
upper bound on memory usage per cell, to ensure malicous applications cannot use this as a DoS
vector"* ([master cell.rs](https://raw.githubusercontent.com/alacritty/alacritty/master/alacritty_terminal/src/term/cell.rs)).
Agent CLIs emit emoji and combining marks freely. Track for the release that lands it. (Hyperlinks
are already cheap: `Hyperlink { inner: Arc<HyperlinkInner> }`, refcounted, not per-cell copies.)

ratatui's own `Cell` (~40 B: `Option<CompactString>` symbol, fg, bg, underline_color, modifier, skip
— [buffer/cell.rs](https://raw.githubusercontent.com/ratatui/ratatui/main/ratatui-core/src/buffer/cell.rs))
gives ~800 KB for two 200×50 buffers. Not where the money is.

### Allocators: what the evidence actually shows

- **jemalloc**: `jemalloc/jemalloc` was archived 2025-06-02
  ([postmortem](https://jasone.github.io/2025/06/12/jemalloc-postmortem/)) but as of 2026-08-31 shows
  `archived: false`, last push 2026-08-27, latest release **5.3.1 (2026-04-13)** — Meta resumed
  maintenance ([context](https://theconsensus.dev/p/2026/04/16/who-even-uses-jemalloc-anyway.html)).
  The Rust binding is `tikv-jemallocator` 0.7.0, whose README lists `aarch64-apple-darwin` as
  compiling/running **with jemalloc's own tests failing** ([README](https://github.com/tikv/jemallocator)).
  Adopt for throughput, not RSS.
- **mimalloc** claims *"bounded space overhead (~0.2% meta-data, with low internal fragmentation)"*
  and *"does not suffer from blowup"* ([README](https://github.com/microsoft/mimalloc)). Useful knobs:
  `MIMALLOC_PURGE_DELAY`, `MIMALLOC_PURGE_DECOMMITS=1`.
- **Counter-evidence — version matters more than brand.**
  [microsoft/mimalloc#1111](https://github.com/microsoft/mimalloc/issues/1111), peak RSS:

  | workload | mi 2.0.6 | mi 2.2.4 | mi 3.1.5 |
  |---|---|---|---|
  | lineitem.parquet | 700,696 kB | 892,512 kB (**+27%**) | 669,608 kB |
  | 1000col.parquet | 2,585,096 kB | 3,224,196 kB (**+25%**) | 2,666,164 kB |
  | StockUniteLegale | 7,002,348 kB | 8,934,212 kB (**+28%**) | 7,064,604 kB |

  Reporter: *"With multi-threading (almost) disabled, the peak RSS problem mostly disappears."*
- **Meilisearch, 2026-03-20** ([blog](https://blog.kerollmops.com/the-good-the-bad-and-the-leaky-jemalloc-bumpalo-and-mimalloc-in-meilisearch)):
  mimalloc **v2** showed persistent RSS growth with LMDB; jemalloc did not; mimalloc **v3** was
  *"close to jemalloc's memory usage"*, and they shipped v3. Perf swing +13% / −9%. Also a hard
  warning if arenas tempt you: **`bumpalo::Bump` does not run `Drop`** — they leaked for ~1.5 years
  by storing `std::Vec` inside `bumpalo::Vec`.
- **macOS**: no glibc, so `MALLOC_ARENA_MAX`/`malloc_trim` do not exist, and macOS compresses memory,
  so `ps` RSS understates. Use `footprint`, which reports `phys_footprint` — *"accounts for RSS, but
  also compressed and I/O Kit related memory"*
  ([Apple forums](https://developer.apple.com/forums/thread/700139),
  [footprint(1)](https://keith.github.io/xcode-man-pages/footprint.1.html)).

**Recommendation:** mimalloc behind a feature flag, defaulted on, measured before/after on dux's own
workload; `MALLOC_ARENA_MAX=2` on Linux. Do **not** adopt jemalloc for RSS.

### syntect

- Loading and linking all default syntaxes from the internal binary dump takes **~23 ms**, and
  syntect *"lazily compiles regexes so startup time isn't taken compiling a thousand regexes for
  Actionscript that nobody will use"*
  ([SyntaxSet](https://docs.rs/syntect/5.3.0/syntect/parsing/struct.SyntaxSet.html)).
- **bat's measured breakdown** ([bat#2545](https://github.com/sharkdp/bat/issues/2545)): a typical run
  is 8.5 ms total, of which **2.5 ms (~29%)** is deserializing theme + syntax sets — and a *larger*
  3.5 ms is compiling glob-pattern regexes for syntax mappings. bat's fix was `LazyThemeSet`.
- `load_defaults_newlines` vs `load_defaults_nonewlines` is a correctness/API choice, **not** a memory
  one; the docs recommend `newlines` where you can supply them. Each dump is ~200 KB and the linker
  drops unused ones.
- `two-face` 0.5.2+bat-0.26.1 (2026-08-07) goes the **opposite** way — it *adds* 100+ syntaxes and
  25+ themes for ~0.6 MiB of binary ([docs](https://docs.rs/two-face/latest/two_face/)). Use it for
  coverage, never for shrinking.
- `tree-sitter` + `tree-sitter-highlight` 0.27.0 (2026-08-30) is the credible alternative — you link
  only the grammars you use — but it is a rewrite of the highlighting layer. `syntastica` 0.6.1 and
  `inkjet` 0.11.1 are low-adoption; avoid for a project that wants contributors.

**The measurable syntect win for dux is lazy construction (item 7), not a library swap.**

### Measurement recipe

```bash
# 0. Isolate the grid: a headless example that builds N Terms and feeds captured PTY bytes.
#    Sample with memory-stats (physical_mem == RSS on Linux/macOS).
cargo run --release --example grid_rss

# 1a. Linux, black box
/usr/bin/time -v ./target/release/dux                 # "Maximum resident set size (kbytes)"
PID=$(pgrep -x dux)
while :; do date +%s; grep -E '^(VmRSS|RssAnon|RssFile)' /proc/$PID/status; sleep 5; done
cat /proc/$PID/smaps_rollup                            # Rss / Pss / anon breakdown
grep -c 'rw-p' /proc/$PID/maps                         # crude glibc-arena count

# 1b. macOS, black box  (do NOT compare macOS ps RSS to Linux RSS)
/usr/bin/time -l ./target/release/dux                  # max RSS, in BYTES
sudo footprint $PID                                    # phys_footprint incl. compressed
sudo vmmap -summary $PID
xcrun xctrace record --template Allocations --attach $PID --output dux.trace

# 2. Allocation-site attribution. dhat writes on Drop, so gate it and quit normally.
#    Cargo.toml: [features] dhat-heap = []   /   [profile.release] debug = 1
#      #[cfg(feature="dhat-heap")] #[global_allocator] static ALLOC: dhat::Alloc = dhat::Alloc;
#      #[cfg(feature="dhat-heap")] let _p = dhat::Profiler::new_heap();
cargo run --release --features dhat-heap               # then quit via the normal keybinding
#    view dhat-heap.json at https://nnethercote.github.io/dh_view/dh_view.html
#    dhat docs: "You should only use dhat in release builds. Debug builds are too slow to be useful."

# 3. Page-level profile ≈ RSS over time (Linux)
valgrind --tool=massif --pages-as-heap=yes --time-unit=ms \
         --detailed-freq=1 --max-snapshots=200 \
         --massif-out-file=massif.out.dux ./target/profiling/dux
ms_print massif.out.dux | less

# 4. Attach to an already-running dux (Linux)
heaptrack --pid $(pidof dux)   # then heaptrack_gui heaptrack.dux.NNNN.zst

# 5. Static type sizes — run this on the 80-variant Action first
RUSTFLAGS=-Zprint-type-sizes cargo +nightly build --release 2>&1 | top-type-sizes -w 120
#    then lock results in with static_assertions::const_assert_eq!(size_of::<Action>(), N)
```

`--pages-as-heap=yes` is what makes massif track RSS rather than just heap
([valgrind(1)](https://manpages.ubuntu.com/manpages/noble/en/man1/valgrind.1.html)); heaptrack
demangles Rust names given debug symbols, and its maintainers warn that attaching *"might open a
Pandora's box of issues"* ([heaptrack](https://github.com/kde/heaptrack)); `bytehound` is the
`LD_PRELOAD` alternative that can stream to another machine
([bytehound](https://github.com/koute/bytehound)). **`cargo-bloat` measures binary size, not RSS** —
do not cite it in a memory report.

---

## Crate recommendations

Versions read from crates.io / docs.rs on **2026-08-31**.

### Toolchain

| Need | Recommendation | Verified | Status | URL | Caveat |
|---|---|---|---|---|---|
| Rust toolchain | **Bump 1.88.0 → current stable 1.98.0** | 1.89.0 shipped 2025-08-07 | — | [releases.rs/1.89.0](https://releases.rs/docs/1.89.0/) | **`File::{lock,try_lock,lock_shared,try_lock_shared,unlock}` stabilised in 1.89.0** — the bump removes a file-locking dependency outright. Staying on 1.88 forces a crate for something std now does. |

### Bash-layer port

| Need | Crate | Version (2026-08) | Last release | Status | URL | Caveat |
|---|---|---|---|---|---|---|
| File locking (primary) | **`std::fs::File::lock`** | Rust ≥ 1.89 | 2025-08-07 | std | [std docs](https://doc.rust-lang.org/std/fs/struct.File.html#method.lock) | `flock`-backed today but *"may change in the future"*; no NFS guarantee |
| File locking (if MSRV < 1.89) | `fs4` | 1.1.0 | 2026-04-28 | Active | [crates.io](https://crates.io/crates/fs4) | maintained `fs2` fork, rustix-backed |
| Raw syscalls | `rustix` | 1.1.4 | 2026-02-22 | Active | [repo](https://github.com/bytecodealliance/rustix) | `fs` feature for `flock`; MSRV 1.65 |
| HMAC-SHA256 | `hmac` + `sha2` (+ `digest`) | 0.13.0 / 0.11.0 / 0.11.3 | 2026-03-29 / 03-25 / 05-03 | Active | [RustCrypto/MACs](https://github.com/RustCrypto/MACs) | still **pre-1.0**; SHA-NI/NEON on by default with runtime detection; the `asm` feature is gone (use `sha2_backend` cfg) |
| Constant-time verify | `Mac::verify_slice` (in `hmac`) | — | — | — | [docs](https://docs.rs/hmac/latest/hmac/) | never `.into_bytes()` + `==` |
| setsid / detach | `rustix::process::setsid` (or `nix` 0.31.3) | 1.1.4 | 2026-02-22 | Active | [rustix::process](https://docs.rs/rustix/latest/rustix/process/index.html) | call from `pre_exec`, not the parent |
| Spawn detached (ergonomic) | `process-wrap` (`ProcessSession`) | 10.0.0 | 2026-08-24 | Active | [repo](https://github.com/watchexec/process-wrap) | `std` feature for sync; supersedes `command-group` |
| Signals (sync CLI) | `signal-hook` | 0.4.4 | 2026-04-04 | Active | [repo](https://github.com/vorner/signal-hook) | use the `Signals` iterator / `flag::register` |
| Subprocess + pipes | `duct` | 1.1.1 | 2025-11-09 | Active | [docs](https://docs.rs/duct/latest/duct/) | has `wait_timeout`; **`kill()` does not reach grandchildren** |
| Timeout → kill whole tree | `process-wrap` `ProcessGroup` + `kill(-pgid)` | 10.0.0 | 2026-08-24 | Active | [docs](https://docs.rs/process-wrap/latest/process_wrap/) | the only reliable grandchild reaping |
| TOML (read) | `toml` | 1.1.4 | 2026-07-28 | Active | [repo](https://github.com/toml-rs/toml) | **1.0 (2026-02-11) swapped in a new parser/writer — upstream flags regression risk**; no longer wraps `toml_edit` |
| TOML (comment-preserving write) | `toml_edit` | 0.25.13 | 2026-07-14 | Active | same repo | the **only** one that round-trips comments — required by dux's "config file is the documentation" tenet |
| JSON | `serde_json` | 1.0.151 | 2026-07-20 | Active | [repo](https://github.com/serde-rs/json) | SIMD forks not worth the `unsafe` at CLI scale |
| CLI | `clap` (derive) | 4.6.6 | 2026-08-06 | Active | [repo](https://github.com/clap-rs/clap) | 5.0.0 exists only behind `unstable-v5` |
| Multicall dispatch | `Command::multicall(true)` | stable since 3.1.0 | 2022-02-16 | Active | [docs](https://docs.rs/clap/latest/clap/struct.Command.html#method.multicall) | changes help/error text; symlinks created at install time |
| Completions / man | `clap_complete` / `clap_mangen` | 4.6.9 / 0.3.3 | 2026-08-06 / 08-12 | Active | same repo | no equivalent outside clap |
| Library errors | `thiserror` | 2.0.20 | 2026-08-08 | Active | [repo](https://github.com/dtolnay/thiserror) | v2 is current major |
| Binary errors | `anyhow` | 1.0.104 | 2026-07-18 | Active | [repo](https://github.com/dtolnay/anyhow) | `.context()` at every boundary |
| Errors (TUI, pretty) | `color-eyre` | 0.6.5 | 2025-05-30 | Low activity | [ratatui recipe](https://ratatui.rs/recipes/apps/color-eyre/) | 15 mo; still the ratatui-documented choice |
| CLI tests | `assert_cmd` + `predicates` | 2.2.2 / 3.1.4 | 2026-05-11 / 02-11 | Active | [assert-rs](https://github.com/assert-rs/assert_cmd) | — |
| Test fixtures | `assert_fs` | 1.1.4 | 2026-05-26 | Active | [repo](https://github.com/assert-rs/assert_fs) | — |
| Golden-file CLI tests | `trycmd` | 1.2.1 | 2026-07-21 | Active | [snapbox](https://github.com/assert-rs/snapbox/) | — |
| PTY-driven tests | `rexpect` | 0.7.1 | 2026-05-14 | Active (now `rust-cli` org) | [repo](https://github.com/rust-cli/rexpect) | revived after pre-2024 dormancy |
| PTY tests (alt) | `expectrl` | 0.9.0 | 2026-05-11 | Active | [repo](https://github.com/zhiburt/expectrl) | richer API, smaller user base |
| Binaries on PATH | `which` | 8.0.6 | 2026-08-26 | Active | [repo](https://github.com/harryfei/which-rs) | — |
| Shell word split/quote | `shlex` | 2.0.1 | 2026-05-17 | Active | [repo](https://github.com/comex/rust-shlex) | RUSTSEC-2024-0006 fixed in 1.3.0; use `try_quote`/`try_join`; **still cannot escape control chars** |
| Shell word split (alt) | `shell-words` | 1.1.1 | 2025-12-10 | Active | [repo](https://github.com/tmiasko/shell-words) | no advisory history, simpler |
| Temp files | `tempfile` | 3.27.0 | 2026-03-11 | Active | [repo](https://github.com/Stebalien/tempfile) | — |
| Config/state dirs | `etcetera` | 0.11.0 | 2025-10-28 | Active | [repo](https://github.com/lunacookies/etcetera) | lets you pick XDG vs Apple strategy explicitly |
| Logging | `tracing` + `tracing-subscriber` | 0.1.44 / 0.3.23 | 2025-12-18 / 2026-03-13 | Active | [repo](https://github.com/tokio-rs/tracing) | already dux's house style |

### TUI / app

| Need | Crate | Version | Last release | Status | URL | Caveat |
|---|---|---|---|---|---|---|
| TUI framework | `ratatui` | **0.30.2** | 2026-06-19 | Active | [crates.io](https://crates.io/api/v1/crates/ratatui) | `WidgetRef` still behind `unstable-widget-ref` — use `impl Widget for &T` instead |
| Terminal emulation | `alacritty_terminal` | **0.26.0** | 2026-04-06 | Active | [crates.io](https://crates.io/api/v1/crates/alacritty_terminal) | `scrolling_history` defaults to 10,000; `CellExtra.zerowidth` unbounded until the ArrayVec change lands |
| PTY | `portable-pty` | 0.9.0 | 2025-02-11 | Low activity (18 mo) | [crates.io](https://crates.io/api/v1/crates/portable-pty) | wezterm-internal; `ChildKiller::kill()` signal semantics undocumented |
| Highlighting | `syntect` | 5.3.0 | 2025-09-27 | Active | [crates.io](https://crates.io/api/v1/crates/syntect) | construct lazily; `onig` is ~2× faster than `fancy-regex` |
| Diffing | `similar` | 3.2.0 | 2026-08-17 | Active | crates.io | bound the result cache |
| SQLite | `rusqlite` | 0.40.2 | 2026-08-08 | Active | crates.io | dux on 0.39 |
| **Snapshot tests** | **`insta`** | **1.48.0** | **2026-06-11** | **Active** | [repo](https://github.com/mitsuhiko/insta) | **add this**; enable the `filters` feature and copy gitui's `apply_common_filters!` |
| Allocator | `mimalloc` | 0.1.52 | 2026-05-22 | Active | [repo](https://github.com/microsoft/mimalloc) | measure; v2→v3 changed RSS behaviour materially |
| Heap profiling | `dhat` | 0.3.3 | 2024-02-04 | Stable/quiet | [docs](https://docs.rs/dhat/0.3.3/dhat/) | release builds only |
| RSS sampling | `memory-stats` | 1.2.0 | 2024-06-26 | Quiet | crates.io | macOS semantics unverified (see Unverified) |
| Type-size triage | `top-type-sizes` | 0.2.1 | 2025-12-26 | Active | crates.io | pairs with `-Zprint-type-sizes` |
| Size regression guards | `static_assertions` | 1.1.0 | 2019-11-03 | Frozen | crates.io | tiny and stable; frozen by design |

### Modularity tooling

| Need | Tool | Version | Last release | Status | URL | Caveat |
|---|---|---|---|---|---|---|
| Module tree + cycle detection + orphans | `cargo-modules` | **0.27.0** | 2026-08-03 | Active | [repo](https://github.com/regexident/cargo-modules) | **no reverse-dependency mode**; `dependencies` emits DOT only |
| Rust parsing (custom generator) | `syn` | 3.0.4 | 2026-08-24 | Active | crates.io | `use super::*` globs carry no item names — accuracy ceiling |
| Name resolution (accurate generator) | `ra_ap_hir` | 0.0.350 | 2026-08-31 | Active | crates.io | zero-version, no API stability; pin exactly |
| Existing syn-based dep graph | `dep_graph_rs` | 0.2.0 | 2025-07-20 | Low (58 recent dl) | [repo](https://github.com/PSeitz/dep_graph_rs) | reference implementation, not a dependency |
| Crate-graph queries | `guppy` | 0.18.0 | 2026-08-25 | Active | [repo](https://github.com/guppy-rs/guppy) | package level only |
| Custom lints | `cargo-dylint` | 6.0.4 | 2026-08-14 | Active (Trail of Bits) | [repo](https://github.com/trailofbits/dylint) | a file-length dylint is unproven; not worth it vs. 10 lines of shell |
| Unused deps after the split | `cargo-shear` | 1.13.4 | 2026-08-11 | Active | [repo](https://github.com/Boshen/cargo-shear) | parses source properly, handles workspaces |
| Line counting | `tokei` | 14.0.0 | 2025-12-30 | Active | crates.io | reporting only; no "fail if > N" mode |

### Unmaintained / avoid

| Crate | Version | Last release | Reason |
|---|---|---|---|
| **`daemonize`** | 0.5.0 | 2023-02-25 | **RUSTSEC-2025-0069: "daemonize is Unmaintained"** (2025-09-15). Hard avoid. [rustsec](https://rustsec.org/packages/daemonize.html) |
| **`command-group`** | 5.0.1 | 2023-11-18 | README: *"The successor of command-group is process-wrap. No further work will be done on command-group."* |
| **`fs2`** | 0.4.3 | **2018-01-06** | 8.5 years. Use `fs4` or std. |
| `advisory-lock` | 0.3.0 | 2020-12-31 | 68 months |
| `file-guard` | 0.2.0 | 2024-03-09 | 30 months |
| `pico-args` | 0.5.0 | 2022-06-04 | 50 months |
| `xflags` | 0.3.2 | 2024-10-31 | 22 months, dormant |
| `ring` (< 0.17) | — | — | **RUSTSEC-2025-0010: unmaintained.** Even 0.17.14 is 18 months old |
| `shlex` < 1.3.0 | — | — | **RUSTSEC-2024-0006** — fixed in 1.3.0 |
| `partial-borrow` | 1.0.1 | 2022-09-05 | dormant **and GPL-3.0-or-later** |
| `generational-arena` | 0.2.9 | 2023-05-22 | dormant; prefer `slotmap` |
| `ratatui-snapshot` / `-snapshots` / `-testing` | 0.0.0 | 2026-06-13 | **name reservations, no code** |

Borderline (>15 months, watch): `dirs` 6.0.0 (19 mo — prefer `etcetera`), `portable-pty` 0.9.0 (18 mo),
`fd-lock` 4.0.4 (18 mo), `color-eyre` 0.6.5 (15 mo), `subtle` 2.6.1 (26 mo, deliberately frozen).

---

## Anti-patterns to avoid

1. **Reorganising and refactoring in the same commit.** The rule from a 2026 Rust monolith-splitting
   writeup: copy code into new files, verify identical output, *then* refactor one concern at a time
   ([monolith→distributed](https://medium.com/@philippe.baucour/from-monolith-to-distributed-systems-in-rust-a-practical-introduction-3975a032ba67)).
   For dux this means: snapshot every pane/modal with `insta` **first**, move code, require identical
   snapshots, then change behaviour.
2. **Expecting directory structure to produce small files.** bottom has `src/{app,canvas,collection,components,options,widgets}/`
   and a **101 KB `app.rs`**; television has a clean layout and a **51 KB `television.rs`**. Only
   yazi's crate-boundary + one-unit-per-file discipline actually caps file size.
3. **Splitting modules and expecting faster builds.** *"The basic unit of compilation is a crate, not
   a module"* — module splitting buys **zero** compile-time win
   ([Rust at scale](https://mmapped.blog/posts/03-rust-packages-crates-modules)). Only crate splitting does.
4. **Sub-structs with methods still on `App`.** `&mut self` borrows *all* of `self`; splitting fields
   into `UiState`/`GitState`/`RuntimeState` buys nothing unless the methods move onto them or become
   free functions taking the narrow borrows
   ([partial borrow thread](https://users.rust-lang.org/t/cannot-borrow-self-as-mutable-because-an-unrelated-field-is-already-borrowed/56241)).
   This is the most likely way the refactor produces churn with no benefit.
5. **Planning around unstable features.** View types (`#![feature(view_types)]`) are
   [experimental with borrowck work incomplete](https://github.com/rust-lang/rust/issues/155938);
   `clippy::too_many_lines_in_file` is an [unmerged PR with conflicts](https://github.com/rust-lang/rust-clippy/pull/16675);
   `WidgetRef` is gated behind `unstable-widget-ref` with *"no stability guarantees"*. None of these
   will be load-bearing in your timeframe.
6. **Indented or unannotated code blocks in `//!` headers.** Both become Rust doctests; indented ones
   **cannot be escaped at all** ([rust#100225](https://github.com/rust-lang/rust/issues/100225)).
   Always ` ```text `.
7. **`Rc<RefCell<AppState>>` with borrows held across calls.** `borrow_mut` *"Panics if the value is
   currently borrowed"* ([std](https://doc.rust-lang.org/std/cell/struct.RefCell.html)), and a TUI's
   render-during-event reentrancy is exactly where it fires. gitui's one-statement-borrow Queue is the
   safe shape.
8. **Widgets talking to widgets.** *"Widgets shouldn't be communicating to other widgets. They should
   be writing to application data/state storage"*
   ([kpreid](https://users.rust-lang.org/t/trying-to-wrap-my-head-around-tui-design/104736)).
9. **One flat enum with a fat variant.** Enum size = largest variant; over **128 bytes** values are
   copied by `memcpy` ([perf-book](https://nnethercote.github.io/perf-book/type-sizes.html)). With 80
   variants, measure before nesting. But do not over-nest either — exhaustive matches get noisy and
   some actions genuinely belong to two categories
   ([rfcs#3385](https://github.com/rust-lang/rfcs/issues/3385)).
10. **`Child::kill()` for PTY teardown.** It does not reach grandchildren
    ([rust#115241](https://github.com/rust-lang/rust/issues/115241)). Use SIGHUP → grace → SIGKILL on
    the process group, zellij-style.
11. **`fork()`/double-fork in a multithreaded process.** `pre_exec` docs: the child runs in *"a very
    constrained environment where normal operations like `malloc`, accessing environment variables …
    or acquiring a mutex are **not guaranteed to work**"*, and *"the list of allocating functions
    includes `Error::new`"*
    ([CommandExt](https://doc.rust-lang.org/std/os/unix/process/trait.CommandExt.html)). Re-exec
    (`Command::new(current_exe).arg("--daemon-worker").pre_exec(setsid)`) instead of forking in-process.
12. **`bumpalo::Bump` holding anything with a `Drop`.** It does not run destructors — Meilisearch
    leaked for ~1.5 years this way
    ([writeup](https://blog.kerollmops.com/the-good-the-bad-and-the-leaky-jemalloc-bumpalo-and-mimalloc-in-meilisearch)).
13. **Micro-optimising strings before fixing scrollback.** compact strings, interning and arenas are
    ~zero at dux's cardinality; the grid is ~95% of the addressable RSS.
14. **Reporting `cargo-bloat` numbers as memory.** It measures binary size.
15. **Comparing macOS `ps` RSS to Linux RSS.** macOS compresses memory; use `footprint`'s
    `phys_footprint` ([Apple forums](https://developer.apple.com/forums/thread/700139)).
16. **Snapshot tests without filters.** Timestamps, spinner frames, temp paths and short SHAs will
    make every snapshot flap. Copy gitui's `add_filter` set on day one.
17. **Deferring tests to "after the refactor."** From a 2026 writeup of a ratatui app hosting LLM
    agents and child processes, listed as a regret: *"Add tests from Phase 1"* — tests written
    afterwards only document existing behaviour instead of catching regressions
    ([part 3](https://jakegoldsborough.com/blog/2026/rewriting-claude-code-in-rust-part-3/)).
    Same source's first regret is directly relevant to dux's provider layer: *"I hardcoded the
    Anthropic client in Phase 1 and had to refactor it later"* — introduce the trait boundary first.

---

## Unverified

Flagged so nothing here is mistaken for a checked fact.

**Architecture**
- No published prose writeup of refactoring a large **ratatui** app's `App` struct exists. The nearest
  substitutes are source-level (gitui, zellij) and a Rust god-object writeup outside TUI
  ([Entropic Drift](https://entropicdrift.com/blog/refactoring-god-object-detector-with-stillwater/):
  4,362-line file → nine sequenced specs, 47% line reduction, **average module 277 lines** — *"Module
  size matters—averaging 277 lines proved manageable"*).
- Reddit (r/rust) blocks the research user agent, so no r/rust discussion could be cited directly.
- Two widely-surfaced anonymous gists on "TUI state machine architecture patterns" and "TUI Testing
  Guide" have strong LLM-generated fingerprints. **Not cited, not trusted.**
- <https://blog.rafaelfernandez.dev/posts/the-elm-architecture-a-loop-that-fits-in-your-head/>
  returned 403. Snippets attribute to it *"the Update function grows into a giant match block"* —
  snippet-level only, not verified against the page.
- `ratatui-testlib` 0.1.0 and `termlens` 0.8.0 were not evaluated for quality or maintenance beyond
  their download counts and stated descriptions.

**Memory**
- All per-row/per-pane MB totals and the `CellExtra` overhead estimate are **arithmetic derived from
  verified struct definitions**, not figures published upstream. They happen to match both
  alacritty#3650 and the coordinator's 20–51 MiB observation, which is corroboration, not proof.
- **Nobody publishes an isolated before/after RSS number for a Rust app switching to mimalloc.** The
  one hard table found (mimalloc#1111) is a *regression* report. `daanx/mimalloc-bench` publishes RSS
  charts but the numbers are not transcribed and the sample results date from 2019.
- No published memory (as opposed to speed) comparison between syntect's `onig` and `fancy-regex`
  backends.
- `Grid::clear_history()` freeing RSS in practice was **not empirically confirmed**. `MAX_CACHE_SIZE`
  hysteresis plus allocator retention mean the observed drop may be much smaller than the logical one.
  **Measure before promising it in a changelog.**
- `memory-stats` docs say `physical_mem` "corresponds to the Resident Set Size" but do not state
  whether macOS reads `resident_size` or `phys_footprint`. Given macOS compression this matters —
  read the crate source before trusting macOS numbers from it.
- alacritty#3650's maintainer reasoning could not be retrieved; the numbers are the reporter's.

**Crates**
- `duct`'s behaviour beyond `kill()` — whether it sets a process group at spawn — is not documented.
  Assume it does not.
- `process_control`'s kill scope (child vs. group) is not stated on its landing page.
- `fs4`'s Unix primitive (`flock` vs `fcntl`) is not stated explicitly; verify before relying on
  fork-inheritance semantics.
- `portable_pty::ChildKiller::kill()` — **which signal, and whether it targets the pid or the process
  group — is undocumented**. Read the crate source; dux may need its own `killpg`.
- `toml` 1.0's regression surface: the changelog flags *"risk for regressions"* without enumerating them.
- `clap` 5.0.0 has no release date (`## [5.0.0] - TBD` behind `unstable-v5`).
- NFS `flock` behaviour: **no crate or std doc makes any NFS claim.** If `$AMQ_GLOBAL_ROOT` could ever
  live on NFS, test it — and keep an application-level fallback (O_EXCL lockfile with pid + boot
  token and staleness detection), because silent degradation from "locked" to "not locked" is the
  failure mode the bash version probably already had.
- `shlex` 2.0.x changelog was not read for API breaks beyond confirming the advisory is fixed.

**Modularity tooling**
- Whether `cargo modules --focus-on` includes **incoming** edges — docs say only *"focus the graph on
  a particular path"*. Assume outgoing-only.
- `arch_test` / `cargo-archtest` ([repo](https://github.com/tdymel/arch_test)) is the only ArchUnit
  analogue found. It advertises `MayNotBeAccessedBy` / `MayOnlyBeAccessedBy` and cycle detection —
  i.e. real reverse-direction rules — but **16 stars, last-commit date and current version
  unverified**. A research lead, not a CI dependency.
- A file-length `dylint` lint is **feasible in principle** (dylint has full rustc lint-pass access) but
  **no example exists**, and it inherits the `#![allow]`-scoping objection that stalled clippy PR #16675.
- Compile-time delta of splitting a 69.5k-LOC binary into workspace crates was not measured for a
  project of this shape. matklad argues the direction; the magnitude is unverified.
- **No public Rust repo enforcing a 500-line file cap in CI was found.** rustc's 3,000-line tidy check
  is the only verified in-ecosystem precedent.
- Zed / Bevy / Servo / Helix per-file header conventions: no published style guide found either way.

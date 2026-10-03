# Yaran

![GitHub License](https://img.shields.io/github/license/SiavZ/dux-amq-setup)

<img src="assets/yaran-logo.svg" alt="Yaran branching Y logo" width="160" align="right" />

**A collaborative workspace for coding agents.** Yaran runs coding agents in shared checkouts or isolated git worktrees. Coordinate them through cross-provider messaging and peer routing, resume their sessions, and manage provider tabs, terminals, macros, and git changes from the terminal or browser.

Agents run as real CLIs in real terminals. Their MCP servers, hooks, skills, slash commands, and permission dialogs remain their own.

Yaran builds on [dux](https://github.com/patrickdappollonio/dux) by Patrick D'Appollonio. The original MIT license and attribution are preserved. This repository's shared-workspace, messaging, orchestration, and session-management work is maintained under the Yaran name.

[Install](#install) · [Workspace modes](#workspace-modes) · [Peer routing](#peer-routing) · [Server mode](#server-mode) · [Documentation](#documentation) · [Upgrade compatibility](docs/operations/rebranding.md)

## Why Yaran?

Keep several coding agents in one workspace without losing track of which checkout, provider, or conversation each one uses:

- Choose a shared checkout for collaboration or isolated worktrees for changes that need separate branches. Forking a session always creates an isolated worktree.
- Send messages across providers using immutable agent handles. Shared-workspace messages use AMQ rather than ambiguous directory-based routing.
- Run several provider tabs inside an agent, resume supported conversations, and open companion terminals for builds and tests.
- Use the same workspace from the terminal and browser, with live terminals, file editing, git staging, and diffs.
- Set resource limits, schedule session backups, and use `yaran doctor` for diagnostics.

Fresh configurations default to **shared workspaces**. Agents in that mode share files, the git index, and branch state. Choose `workspace.default_mode = "worktree"` when you need isolation. [Workspace modes](#workspace-modes) explains the settings and the upgrade behavior for older configurations.

Every agent runs through a PTY, the same pseudo-terminal your shell uses. That means the CLI tool (Claude, Codex, Copilot, OpenCode, or literally anything else) runs exactly like it would in your regular terminal. Your MCP servers, hooks, skills, slash commands, and permission dialogs all work. We don't mess with your setup.

## Two front ends, one workspace

Yaran has a terminal UI and a web UI over one running app. They share projects, agents, worktrees, and configuration. An agent you start in one is the same agent in the other.

They are not identical, on purpose. Each surface does what its medium is good at. The terminal gives you full keyboard control, rebindable keys, a command palette that knows more tricks than you do, and themes. The browser gives you reach: any device on your network, including a phone, plus editing files in the page and desktop notifications. Where a capability only makes sense on one side, it lives on one side, and the page that covers it says why.

You won't find a per-feature comparison table here, because a table like that is stale the week after it's written. The app is the reference: in the terminal, the help overlay and the command palette; in the browser, the cog menu and the row `⋯` menus.

One thing worth knowing before you point a browser at anything: **there is no login.** [Server Mode](#server-mode) explains exactly what that means and which shapes are safe; read it before you bind anything but loopback.

## Prerequisites

- **Rust stable** and **Node 22.12 or newer with npm** to build the executable and its embedded web UI.
- **`git`** on your PATH for project and worktree operations.
- The coding-agent CLIs you want to run, installed and authenticated through their own tools.
- **`gh` CLI** *(optional)*: authenticate it with your GitHub account and yaran can pull PR statuses, check details, and show them right in the interface. Not required, but you'll miss it once you've tried it.

[`CONTRIBUTING.md`](CONTRIBUTING.md) covers build and test commands, including the Rust-only option for contributors who do not need the web UI.

## Install

Yaran supports macOS and Linux. Windows users can build and run it inside WSL2.

### Build from source

Clone the current repository into a Yaran-named checkout, then install the executable:

```bash
git clone https://github.com/SiavZ/dux-amq-setup.git yaran
cd yaran
cargo install --path crates/yaran --locked
yaran
```

The build includes the React web UI. For development, `cargo build` places the executable at `target/debug/yaran`. See [CONTRIBUTING.md](CONTRIBUTING.md) for the full build and test instructions.

To start with the browser instead of the terminal UI:

```bash
yaran server --bind 127.0.0.1:3890 --no-tailscale
```

Open `http://127.0.0.1:3890`, add a project, and create an agent with an installed provider. This command serves only your local machine. **Yaran has no built-in login**, so read [server mode](#server-mode) before exposing it to another device. If the TUI is already running, use its `start-web-server` or `start-background-server` palette command instead of starting a second process for the same workspace.

### Release binaries

Yaran releases use `yaran-vX.Y.Z` tags and `yaran-<os>-<arch>.tar.gz` archives. The GitHub repository still lives at [SiavZ/dux-amq-setup](https://github.com/SiavZ/dux-amq-setup). That is its existing address, not the product name.

The renamed installer requires a release containing the new `yaran` binary. Until the first Yaran release is published, use the source-build instructions above. Older `dux-amq-v*` releases contain the previous `dux` executable.

For a published Yaran release, download its `install.sh` from this repository's [releases](https://github.com/SiavZ/dux-amq-setup/releases), inspect it, and run it:

```bash
YARAN_VERSION=yaran-vX.Y.Z bash install.sh
```

`YARAN_INSTALL_DIR` selects the installation directory. Legacy `DUX_VERSION`, `DUX_INSTALL_DIR`, and `DUX_REPO` inputs remain accepted when their Yaran equivalents are unset. Published archives include SHA-256 checksums. These catch corruption, but unsigned checksums from the same server are not proof against tampering.

There is no published Yaran Homebrew formula or npm package yet. Upstream dux's installers install dux, not Yaran. The local npm package is release packaging, not a claim that it is already available in the npm registry.

The optional multi-agent overlay is under [yaran-amq/](yaran-amq/README.md). Its wrappers retain their provider-specific command names, such as `claude-amq`, `codex-amq`, and `jcode-amq`.

### Upgrading from dux

Yaran accepts `YARAN_HOME` as its primary configuration override and the previous `DUX_HOME` when the new override is unset. Existing dux configuration and saved sessions are reused without moving or deleting them. New installations use Yaran-named state paths. See [the rebrand compatibility notes](docs/operations/rebranding.md) for precedence, legacy markers, and the remaining external publishing steps.

Start the application with `yaran`, or the browser server with `yaran server`. On first launch, Yaran offers to add your first project. [First run and what's new](#first-run-and-whats-new) describes onboarding and release notes.

## Documentation

The documentation sources live in [website/docs/](website/docs/). The README is the tour, and those pages are the reference. They cover:

- **Dropping and pasting files**, in the browser: drag, paste, or pick a file, and yaran saves it on the server and pastes its path into the agent.
- **Theater mode and the phone shell**: the hub-and-spoke layout on a small screen, and the mode that hands the whole page to one terminal.
- **Attention indicators**: how yaran notices an agent is waiting on you, and lights up the sidebar, the browser tab and the favicon.
- **Agent tabs**: several provider sessions inside one agent, all sharing the one worktree.
- **The in-browser editor's reach**: a real Monaco editor over any file in a worktree, with previews, path search, and diffs against `HEAD`, or rooted at a terminal's directory (no diff view there, since a plain directory has nothing committed to compare against).
- **Pull request banner settings**: including whether the banner sits above or below the terminal.
- **Naming a web instance**: `[server] title` and `favicon`, so you can tell several yaran tabs apart.
- **Hosting yaran behind a login**: a reverse proxy and oauth2-proxy in front, since yaran has no login of its own.

## How it works

Yaran organizes work around **projects**, which are git repositories, and **agents**, which run provider sessions. A shared agent uses the registered checkout directly. In worktree mode, Yaran creates a separate checkout and branch for the agent, leaving the registered checkout in place. The project's workspace setting determines which mode a new agent uses.

You can also point an agent at a folder you already have, with no project, no branch and no worktree of yaran's: a **standalone agent**. In the terminal UI it has a key of its own in the agents pane and inside the project chooser (the `?` help overlay names both, and they are rebindable), plus the `new-standalone-agent` palette command; in the browser it lives in the launcher's `⋯` menu. Both surfaces ask you to name it once you have picked the folder, and the name is optional: a blank name means the folder's name, and a typed one is used as you typed it, interior spaces and punctuation included, with surrounding whitespace trimmed. yaran runs the provider there and never creates, moves or removes that folder. The branch-identity features (push, pull, fork, pull requests) do not exist for one, and the changes panel follows the folder: you get a real one when the folder is itself a git repository. Having no project, it gets the global `[env]` table with no project overlay on top, and no startup command runs, because a startup command is a provisioning step for a worktree yaran just made. One more thing to know: the hidden upload directory yaran keeps inside the folder for files you drop on the agent outlives the agent, since deleting a standalone agent removes yaran's own record and nothing of yours; remove that directory yourself if you don't want it.

Already have a Git worktree you want yaran to use? In the browser, **Worktrees…** in a project's `⋯` menu; in the terminal UI, the `new-agent-from-worktree` palette command lets you pick a project from the chooser and then choose from its existing worktrees. If the worktree is already managed by yaran, yaran reuses it and reconnects like a continuable session; if it's outside yaran's managed worktree directory (terminal UI only), yaran forks it: a new managed worktree branched from that worktree's current `HEAD`, with dirty and untracked files copied across, so the original checkout is left alone. Gitignored files do not travel.

In the terminal UI, the interface has three panes:

- **Left:** a flat list of your agents, most-active first by default, with search and a project chooser
- **Center:** the agent's live terminal (or a file diff). Focus it and type: your keystrokes go straight to the agent, right there in the window, while yaran's own shortcuts keep working around it
- **Right:** changed files, staging, and diffs

That sort order is one setting shared by the terminal UI and the browser (activity, last updated, recently created, name, or your own hand-placed order), so whichever surface you change it on, the other follows.

Move focus between the panes with the keyboard, and resize them with keyboard or mouse. Collapse the sidebar or git pane when you want more room. Toggle the agent fullscreen when you want every key and every cell to belong to it. It's your layout.

You can also end an agent's session from outside its own CLI. **Detach agent…** in the agent's `⋯` menu in the browser, and the `detach-agent` palette command on the selected agent in the terminal UI, ask everything the agent is running to shut down, wait the top-level `shutdown_timeout_seconds` from your config (30 seconds by default), and force-close whatever is still there. Both confirm first and name that wait. The agent stays in your list as detached and can be resumed later, but whatever it was in the middle of is interrupted.

### Bring Any CLI

Any terminal command can be a provider. The defaults (Claude, Cline, Codex, OpenCode, Kilo Code, NTL, Copilot, and [jcode](https://github.com/1jehuang/jcode)) are pre-configured, but adding your own is a config-only change:

```toml
[providers.my-agent]
command = "my-cool-agent"
args = ["--some-flag"]
resume_args = ["--continue"]
resume_by_id_args = ["--resume", "{session_id}"]
```

`resume_args` is your CLI's own resume flag: when yaran relaunches the provider in a worktree it has already run in, it passes those args so the CLI picks its own conversation for that directory back up. yaran is not reattaching to a live process, it is starting a new one that continues where the old one left off. Omit `resume_args` if your CLI doesn't support resuming; yaran will just relaunch it fresh. Two tabs of the *same* provider can't both resume the one conversation, so only the first one up does; different providers in one agent each resume their own.

`resume_by_id_args` resumes one exact provider conversation instead of the most recent one for the directory: `{session_id}` is replaced with the captured conversation UUID as a literal argv token. Agents in a [shared workspace](#workspace-modes) never fall back to `resume_args`, because several agents share one directory and "most recent" would pick the wrong conversation. Without a valid captured UUID and a configured `resume_by_id_args` they start fresh and show a warning.

Provider blocks carry a few more keys than these: `install_hint` (what to suggest when the command isn't on your PATH), `resume_wait_timeout_ms`, `forward_scroll`, and `web_dragdrop_paste` (how a dropped file's path is quoted for this CLI). The [provider documentation](website/docs/custom-agents.md) covers each of them, and so do the comments in your config file.

When a provider supports resume args, yaran can auto-reopen agents that were still running when the app exited. A normal agent exit with status code 0 is treated as intentional and will not be reopened. The feature is off by default; enable it globally with `[ui].auto_reopen_agents = true`, opt out a project with `auto_reopen_agents = false` in its `[[projects]]` entry, or use the `toggle-project-auto-reopen-agents` and `toggle-agent-auto-reopen` palette commands for project and per-agent opt-outs.

Switch providers from the command palette. yaran sticks to one agent per worktree, so provider changes happen in place:

- **`change-agent-provider`** swaps the *selected* worktree's provider on next launch. If the agent is still running, yaran records your choice and warns you: the running agent keeps going until you exit and relaunch it, at which point it spawns with the new provider. yaran tells you when you pick whether that relaunch will land in the provider's previous conversation or start fresh.
- **`change-default-provider`** picks the global fallback provider for *new* agent sessions in projects without a project-specific override. Existing agents keep their current provider; to move a running one, use `change-agent-provider` after stopping it.
- **`change-project-default-provider`** picks the provider future agents should use for the selected project only, or lets that project inherit the global fallback again.

Resuming is decided at launch time, per provider, and never pinned when you choose one. yaran passes a provider's `resume_args` only when that provider defines them, when it has already run in this worktree, and when no other tab of the same agent is currently running or launching that same provider (two tabs of one provider would both reach for the same most-recent conversation, so the second one starts fresh). Tab position has nothing to do with it: whichever tab of a provider comes up on its own gets that provider's conversation. A tab you create always starts fresh, whatever else is running, because the resume slot is for tabs coming back up rather than tabs being made. Copilot ships without `resume_args` on purpose, because its own continue flag resumes the most recent session globally rather than per directory, so a Copilot tab always starts fresh.

The header shows `default provider: …` when the selected project inherits the global fallback. If a project has its own override, the header shows `project provider: …`, adding `global default: …` beside it only when the two differ. It also adds `current provider: …` when the selected agent is using a different one, so you always know which CLI you're talking to.

Project-specific provider defaults are managed from inside yaran with `change-project-default-provider`; `config.toml` only stores the global fallback.

### Startup Commands

Some projects need a little ceremony before an agent is useful. JavaScript projects want `npm install`, Rust projects may want a cache warmup, and some repos come with a setup script because apparently suffering builds character. Configure a project startup command and yaran runs it in the new agent worktree before launching the provider.

```toml
[[projects]]
id = "00000000-0000-0000-0000-000000000000"
path = "$HOME/projects/web-app"
name = "web-app"
env = { EDITOR = "true", API_KEY = "${FOOBAR_API_KEY}" }
startup_command = """
npm install
npm run build:types
ln -sfn "$YARAN_WORKTREE_PATH/.env.local" .env
"""

[startup_command_terminal]
command = "$SHELL"
args = ["-l", "-c"]
```

You can edit the command from the palette with `configure-startup-command`, or keep it in `config.toml` with the rest of your project intent. The multiline editor is not pretending to be fancy: yaran passes the whole block as one script string to your configured shell, and shells already know that newlines separate commands. Put `npm install`, symlink setup, cache priming, or whatever tiny ritual your repo demands in there.

Global env goes in the top-level `[env]` table and applies to every project:

```toml
[env]
EDITOR = "true"
API_KEY = "${FOOBAR_API_KEY}"
```

Project `env` values override global keys, because sometimes one repo deserves special treatment and the rest of your machine should not have to hear about it. Edit the global set from the palette with `configure-global-env`, and edit the selected project with `configure-project-env`.

Project paths support `$HOME`, `${HOME}`, and `~` so the file can travel between machines without hardcoding your username like a tiny portability crime. Env values reach new agents, companion terminals, and startup commands. Values support the same `$VAR` and `${VAR}` expansion, so `API_KEY = "${FOOBAR_API_KEY}"` copies a secret from the parent environment while `EDITOR = "true"` can keep agents out of interactive editors. A terminal or agent may still start a shell that evaluates your profile files again; if those files reconfigure the same variables, yaran cannot prevent that. Write shell defaults so they keep incoming values when present:

```bash
export VISUAL="${VISUAL:-nvim}"
export EDITOR="${EDITOR:-$VISUAL}"
```

The startup command itself runs through your configured shell, so shell environment expansion works inside the command (`$HOME`, `${VAR}`, `$PATH`, `$EDITOR`, and friends). It runs with the agent worktree as the current directory, so relative paths point at the new checkout and normal shells report that through `$PWD`. yaran also sets `YARAN_PROJECT_PATH`, `YARAN_WORKTREE_PATH`, `YARAN_AGENT_ID`, `YARAN_AGENT_BRANCH`, `YARAN_PROVIDER`, and `YARAN_STARTUP_COMMAND_LOG` for scripts that want to know where they are and who invited them.

If the command fails, yaran still creates the agent. The failure shows in the status line, because setup scripts are allowed to be dramatic but not allowed to block the show. Use `read-startup-command-logs` to browse every run, newest first and already open on the last one, and `rerun-startup-command-on-agent` when the fix is obvious and you want the machine to try again.

### Macros

Tired of typing the same prompt over and over? Turn it into a macro. Macros are reusable text snippets you trigger from a quick-select bar. Search by name, hit enter, and the text gets sent to the active pane.

```toml
[macros]
"Review" = { text = "review this code for bugs and security issues", surface = "agent" }
"Build" = { text = "cargo build --release 2>&1", surface = "terminal" }
"Ship it" = { text = "run all tests, fix failures, then commit", surface = "agent" }
```

Each macro can be scoped to the agent pane, the companion terminal, or both.

### Git Integration

The right pane is a full git staging area. Stage and unstage files, view syntax-highlighted diffs, write your commit message, push, and pull, all without leaving yaran. Want help wording it? Just ask your agent in its terminal to draft the commit for you.

**PR tracking:** With the `gh` CLI installed, yaran tracks pull requests for your agent branches and shows status pills right in the interface. A push, or selecting an agent, refreshes that agent's pull request there and then, and a slow background check picks up anything else, so the pills stay current without burning through your GitHub API quota.

### Companion Terminals

Each agent gets its own companion terminal: a separate shell session in the same worktree. Use it for builds, tests, git operations, or anything else you'd normally do in a terminal. You can spawn multiple companion terminals per agent.

Projects get terminals too. A **project terminal** is a plain shell opened at the project's repo root with no agent attached, handy for repo-wide chores (and, over the web UI, for reaching the machine when there is no local terminal to fall back to). Spawn one from the project's menu on either surface; removing the project closes its project terminals.

And a **standalone terminal** belongs to nothing at all: no agent, no project. It opens in your home directory, so you can reach for one before you have added a single project. Open it from the `new-standalone-terminal` palette command in the TUI, or in the browser from the `⋯` menu beside the launcher button at the bottom of the sidebar, under **Terminals** (the cog menu's **New** submenu has the same entry, and the Terminals divider in the sidebar carries a **+** once you have one). Its sidebar row shows the directory it opened in rather than an owner. Nothing closes it for you: removing a project or deleting an agent closes their own terminals and leaves this one alone, so it ends when you close it or when yaran shuts down.

### Forking Sessions

See an agent going down the wrong path? Fork it. yaran creates a new worktree with the current files copied over so you can try a different approach without losing the original session. It's branching, but for your AI conversations.

Forking always creates an isolated worktree, even when the project normally runs agents in a shared workspace.

### Workspace Modes

An agent can run in its own git worktree or directly in the project's registered checkout (a shared workspace). Freshly generated configs default new agents to the shared checkout:

```toml
[workspace]
default_mode = "shared" # or "worktree"

[[projects]]
path = "$HOME/projects/example"
workspace_mode = "worktree" # optional per-project override; "" inherits
```

Consent is preserved for existing installations: if an existing config has no `[workspace]` section at all, yaran keeps creating isolated worktrees. Regenerating a fresh config writes the shared default explicitly.

Shared sessions use the project's canonical path and never switch the real checkout during registration. Claude and Codex conversations are captured per agent and reconnect by exact provider UUID, so multiple agents in one directory never select history by recency. Shared startup auto-resume is off by default (`workspace.auto_resume_shared = false`); when enabled it uses the same exact-ID rule. Histories stranded under old yaran worktrees are copied (Claude) or mapped (Codex) once at startup without modifying their originals. Branch and PR status follow the checkout's live `HEAD`; a detached `HEAD` skips PR discovery. A shared project cannot live inside `YARAN_HOME` or its managed worktree tree.

Starting a second live shared agent asks for confirmation, because both agents share the checkout's files, index, staging area, commits, branch switches and discards. While this yaran store can see several live writers, the header shows a persistent `CURRENT STORE ONLY` warning. That warning cannot see agents launched under another `YARAN_HOME` or unmanaged processes using the checkout, so its absence is not proof of exclusive access. Fork into an isolated worktree whenever changes need to diverge.

The `prune-orphan-worktrees` palette action is opt-in and never automatic. It lists only Git-registered linked worktrees inside yaran's worktree root that have no active session row or soft-deleted tombstone, excludes the main checkout and unrelated directories, and reports dirty or untracked state. Every item needs its own confirmation, and its branch is kept unless you explicitly choose to delete it. Changing workspace mode never orphans or removes existing worktree sessions.

### Peer Routing

Agents send messages through yaran instead of picking a transport themselves:

```bash
yaran peer send <handle> "status? blockers? next proof?"
yaran peer list
yaran peer sync-amq
```

`yaran peer send` sends to a Claude agent over Claude Peers and to every other provider over AMQ; pass `--transport amq` to override. If either end is in a shared workspace, yaran always routes through AMQ by the agent's immutable handle, because matching peers by working directory would be ambiguous. Every agent is launched with `YARAN_SESSION_ID`, `YARAN_STORE_ID`, `YARAN_PROVIDER` and `YARAN_AMQ_HANDLE`, and yaran refreshes AMQ's agent registry from `sessions.sqlite3` when the TUI or `yaran server` starts, and on `yaran peer sync-amq`.

### Per-Session Settings

Every agent has its own settings drawer (the `session-settings` palette command, or its keybinding) covering context mode (Attended, Orchestrator or Worker), YOLO permissions (including OpenCode's `--auto` mode), per-rule arm and disarm for watch rules, auto-clear after a task is done, and an AMQ verify-envelope override. The defaults are cautious on purpose: a missing or corrupt settings record always loads as Attended with no YOLO and no auto-clear, so tampering with the database cannot escalate a session into autonomous mode. Settings persist in `sessions.sqlite3` and follow the agent across detach and reconnect.

### Adding Projects

Point yaran at any folder. A git repository joins the workspace as-is; a plain folder gets an offer to become one: yaran runs `git init`, seeds a commented starter `.gitignore` for the dependency and build directories it finds (`node_modules`, `target`, and friends), creates an empty initial commit, and registers the project. Your existing files are left untouched (untracked). Folders inside an existing repository are refused with a pointer to the repository root, so projects never nest inside each other's history. In the web UI the picker can even create a new folder first, which makes starting a brand-new project from a phone entirely shell-free.

### First Run and What's New

The first time Yaran launches, it opens a welcome screen explaining projects, agents, providers, and the actual config path on your machine. You can add your first project or dismiss the screen and explore. The web UI's **View project** link opens this GitHub repository. Yaran does not send you to the upstream dux website as if it were the fork's own site.

After an update, yaran shows a **What's new** screen for the release you just moved to: that release's headline, its opening paragraphs, and its feature titles, plus a button to the full notes on GitHub. yaran asks GitHub for the tag it is actually running, not for whatever is newest, so you never get shown features you don't have. The notes are fetched at launch, with no account or token involved, and a copy is kept next to your config. If the fetch can't get through, yaran shows nothing and stays quiet: a failure that might clear up (offline, timeout, rate limit) leaves the version unrecorded, so the notes are waiting on a later launch that has a network. A development build never auto-shows the what's-new screen, since there's no published release to describe.

Closed one too fast? Both screens are reachable on demand: `show-welcome-screen` and `show-release-notes` in the command palette, or **Welcome screen…** and **What's new…** in the web UI's cog menu. The version you've seen is stored once and shared, so dismissing on either surface settles it for both.

Both automatic screens are opt-out:

```toml
[ui]
disable_automated_welcome_screen = false  # suppress the first-run welcome screen
disable_release_notes            = false  # suppress the what's-new screen and the launch-time fetch
```

Each one suppresses only the *automatic* appearance. `disable_release_notes` additionally skips the startup network request entirely. Opening either screen yourself still works, and the release-notes command still fetches: the setting controls the automatic screen, not what the screen is allowed to show. In the web UI both are rows in the cog menu's **Preferences…** dialog, phrased the positive way round.

### Command Palette

Press the palette key and you get searchable access to every action in yaran, including features that don't have dedicated keybindings. Sort agents, toggle UI elements, open the resource monitor, rename sessions, edit macros, and more. If you forget a keybinding, just open the palette. Type the words in any order and only as much of each as you like: exact phrase matches come first, and anything looser the words still reach follows them in the same list.

### Server Mode

Everything yaran does in your terminal, it can do in a browser:

```bash
yaran server
```

That serves the same workspace, not a copy of it and not a dashboard bolted on the side: the same `config.toml`, the same projects, the same agents on the same worktrees. Nothing is mirrored or re-synced, because there is nothing to mirror to. Only one yaran runs against a yaran directory at a time, so what the browser shows you is that same yaran and the terminals it is driving, live, not a snapshot of them. Open the URL from your laptop, your phone, or a tablet on the couch. Start an agent at your desk, walk away, pick that exact session up somewhere else.

You get the workspace, not a read-only view of it: attach to any agent's terminal and its provider tabs, spawn companion and project terminals, create, fork and adopt agents, stage and commit and push, review diffs, edit any file in a worktree with a real editor right in the page (your `config.toml` included), add a project by browsing the server's filesystem, and get desktop notifications when an agent wants you.

Already in the TUI with agents running? You don't need a second yaran, and you couldn't have one anyway: only one yaran can use your yaran directory at a time, so a second one refuses to start and says another yaran is already running there. The one you have can serve the browser two ways instead. Run the `start-web-server` palette command and your running TUI starts serving in place: your agents keep running, no relaunch and no lost conversations; your terminal becomes a status screen, and leaving that screen drops you back into the TUI with everything still running. Or set `serve_while_tui = true` under `[server]` (the `start-background-server` palette command does it live, and `stop-background-server` stops it again with your agents still running) and yaran serves the browser *behind* the TUI, so the same workspace is on your terminal and your phone at once. The TUI then joins the same one-driver-at-a-time model the browsers use: one device drives a terminal, everyone else watches the live output, nothing passive ever takes it away or hands it back, and a card covers any terminal that is not yours to type into, naming the device that is driving it or saying Running in the background when nobody is. Its Take over button is the one way to claim it. The terminal and the browser show the same card. The top bar says `● serving :3890` for as long as the listener is up, growing a `· 2 connected` when browsers are on it, which, since there is no login, is worth knowing.

**How it binds.** By default `yaran server` binds `127.0.0.1:3890`, loopback only, so nothing leaves the machine. If the `tailscale` CLI is around it also binds this machine's Tailscale address on the same port, so your own tailnet devices reach yaran over WireGuard. On the default `tailscale = "auto"` that leg follows the interface: yaran binds it whenever your tailnet address is there, drops that one listener when it goes away, and binds it again when it comes back, all while serving. Set `tailscale = "yes"` under `[server]` to look once and keep what it finds, `"no"` (or `--no-tailscale` for a single run) to skip it, and when Tailscale isn't there yaran warns and serves the configured host only. You can change the mode while yaran is serving, from the TUI palette's `set-tailscale-mode` or the browser's Preferences dialog: it moves the listener there and then and saves your choice. `--bind <ADDR:PORT>` sets an exact address and port, and it wants an IP literal and a port (`0.0.0.0:3890`, `127.0.0.1:9000`): hostnames are not resolved, and the flag may be given only once. `--port <PORT>` overrides just the port, and is ignored when `--bind` is set. A required address that can't bind is fatal and says so; the Tailscale leg failing to bind is only a warning. Both of the in-app ways, `start-web-server` and `serve_while_tui`, always serve loopback plus Tailscale and never a custom host, so reach for `yaran server` when you need a specific interface.

**And there is no login.** None: no password, no token, no user accounts. yaran is a single-tenant, trusted-access tool, and server mode is honest about that instead of pretending otherwise. Everyone who can reach the address shares one workspace: they can drive any agent or terminal, browse the server's filesystem, edit files in your worktrees, and see every session. That's deliberate, and it means access control is entirely a question of where you bind.

The safe shapes are loopback (the default), your own tailnet, or a reverse proxy you put in front and authenticate yourself, which is also where TLS would live, since yaran itself serves plain HTTP. The shape that isn't safe is a LAN or public address, `--bind 0.0.0.0:3890` and friends: that puts your agents and your worktrees in reach of anyone who can hit it. yaran prints a loud warning before it does that, but the warning is the only thing standing there. Don't serve it to anyone you wouldn't hand a shell on that machine.

Two defenses do always run, and they're about hostile web pages rather than about users: a Host-header allowlist, so a malicious site can't DNS-rebind your browser into the server, and a same-origin check on every live terminal connection and every request that changes something, so another site can't ride along. Both are automatic. If you reach yaran by a name rather than an IP literal, a tailnet MagicDNS name or a proxy hostname, add it to `allowed_hosts` under `[server]` or the host guard answers `403`. That one is read when serving starts, so it takes a server restart rather than a config reload; yaran says so when you reload a config that changed it.

The rest of `[server]` tunes presentation and limits: console color, the per-request access log, the shutdown grace period, and how many live connections of each kind yaran accepts at once. As ever, each key explains itself inline in your config file.

### Configuration

The config file at `~/.config/yaran/config.toml` (Linux, or `$XDG_CONFIG_HOME/yaran/config.toml` when you have set that variable to an absolute path) or `~/.yaran/config.toml` (macOS) is exhaustively commented. Every setting is explained inline, so you should never need to leave the file to understand an option. Every keybinding is rebindable. Every pane width, scrollback limit, default provider, and startup agent reopening behavior is configurable.

```bash
yaran config path          # Print the config file path
yaran config diff          # Show what you've changed from defaults
yaran config diff --raw    # Unified diff against the default config (prints [env])
yaran config reset         # Remove config and logs (keeps agents)
yaran config reset --all   # Full factory reset
yaran config regenerate    # Preview a fresh default config
yaran config restore-docs  # Preview re-adding the comments, keeping your values
```

`yaran config diff` is derived from the config structure rather than from a list
somebody has to remember to update, so a new setting shows up in it the day it
ships. It summarizes instead of printing two things: `[env]` reports only that it
changed, and `[[projects]]` reports only a count. That keeps tokens and local
paths out of the output, which makes the summary safe to paste into a bug report.
`--raw` is the opposite: it prints your whole config, `[env]` values and all, so
redact it before you share it.

If your `config.toml` is missing its explanatory comments (older versions could
create one without them), `yaran config restore-docs` puts them back without
touching a single value. It previews the change by default; `--yes` applies it
and writes a timestamped backup first.

Override the config directory with the `YARAN_HOME` environment variable.

`yaran config reset --all` fails closed: it needs a loadable config, a valid store ID and project inventory, and a loadable session and tombstone database when one exists. If it aborts, repair the item it names (regenerate the config, restore `sessions.sqlite3.bak`, or restore the original store ID) and retry. If the identity cannot be restored, verify and remove the associated data by hand before deleting metadata, because a replacement store ID cannot prove old AMQ ownership.

### Data Lifecycle

yaran keeps per-session data in several places: the worktree on disk, a row in `sessions.sqlite3`, the exact-owner AMQ inbox (`agents/<agent_handle>/` under the AMQ root), the provider's own chat history for that directory, and log lines tagged with the session. Most workflows leave all of it alone. `yaran config reset --all` is the holistic factory reset described above.

For a right-to-erasure request (GDPR Art. 17), or just "delete this customer's data", use `yaran session purge`:

```bash
# Preview the cascade; nothing is changed.
yaran session purge --hard <uuid-handle-or-branch> --dry-run

# Real run. Asks for the confirmation phrase 'PURGE <branch>'.
yaran session purge --hard <uuid-handle-or-branch>

# Skip the prompt, for scripts.
yaran session purge --hard <uuid-handle-or-branch> --yes

# Shared workspace: erase owned records and accept that provider transcripts
# remain, because the provider directory is shared.
yaran session purge --hard <uuid-or-handle> --accept-residual-data

# Shared workspace: purge provider history for the whole checkout. This purges
# every yaran session on that checkout.
yaran session purge --hard <uuid-or-handle> --workspace-wide-provider-history

# Bulk: erase owned data for every session.
yaran session purge-all --dry-run
yaran session purge-all --yes
```

For isolated worktree sessions the cascade runs in a fixed order (worktree, provider chat directories, exact-owner AMQ inbox, log redaction, SQLite row) so a failure leaves a recoverable record in `sessions.sqlite3`. A shared-session purge never removes the registered checkout. Its provider history is reported as `INCOMPLETE` and the row is kept unless you accept residual transcripts or confirm a workspace-wide purge, which also deletes non-yaran conversations the provider stored for that path, since providers do not separate them by yaran session. A branch that matches several sessions is rejected; use the UUID or the immutable handle instead.

Deleting a whole worktree or reset root is also blocked whenever the target is an ancestor or descendant of any registered project path. This guard does not apply to ordinary operations on files inside a worktree, such as discarding an untracked directory.

### Operations Settings

The fork also ships settings for running many agents on one host. Each is documented inline in `config.toml`:

```toml
[limits]
max_panes = 0                                # hard cap on live agent panes, 0 means no cap
max_panes_soft_warn = 16                     # warn (do not block) at this many live panes, 0 silences
max_companion_terminals = 0                  # hard cap on companion terminals, 0 means no cap
max_total_scrollback_mb = 256                # scrollback memory budget, acted on only with the flag below
enable_scrollback_overflow_autodetach = false # stop the oldest agent over the budget (off by default)
disk_high_water_pct = 95                     # refuse new agents at this disk usage

[auto_resume]
concurrency = 4   # max parallel startup launches, 0 is treated as 1
stale_days = 30   # skip agents untouched for this many days, 0 disables
stagger_ms = 250  # minimum gap between two startup launches

[storage]
backup_interval_minutes = 30  # periodic sessions.sqlite3.bak, 0 disables
```

`disk_warn_pct` (default 80) rounds out `[limits]`: a status-line warning
before the high-water refusal. Every guard defaults to off or to a warning,
because yaran never refuses to start an agent unless the user asked for a hard
cap; the one refusal on by default is a nearly full disk.

`yaran doctor` (`--json`, `--anonymize`) prints a read-only triage dump to attach to a support thread: database integrity and session counts from the Rust side, plus versions, disk usage, AMQ queue health and recent errors from the `yaran-amq-doctor` script when the `yaran-amq` overlay is installed. For encrypting agent state at rest, see [docs/operations/encryption-at-rest.md](docs/operations/encryption-at-rest.md); for the threat model, [docs/operations/threat-model.md](docs/operations/threat-model.md).

### Themes

yaran writes `config.toml` the first time it launches, so theme setup starts from a real, editable file instead of a guessing game. The generated config includes `[ui].theme = "yaran_dark"`, plus comments with built-in theme examples. Edit that value, or use the `change-theme` command from the palette to preview and save a theme from inside the app.

Custom themes live next to the config file:

```text
~/.config/yaran/themes/my_theme.toml  # Linux
~/.yaran/themes/my_theme.toml         # macOS
```

Then set:

```toml
[ui]
theme = "my_theme"
```

Theme names resolve in this order: your `themes/<name>.toml` file wins first, then the bundled `yaran_dark`, then built-in [Opaline](https://github.com/hyperb1iss/opaline) themes such as `catppuccin_mocha`, `nord`, `dracula`, `gruvbox_dark`, `tokyo_night`, `solarized_dark`, `one_dark`, and `rose_pine`. If the name cannot be loaded, yaran falls back to `yaran_dark` and writes a warning to the log.

Themes use the [Opaline](https://github.com/hyperb1iss/opaline) TOML format. A small theme only needs semantic tokens; yaran derives its app-specific `yaran.*` colors from those so you do not have to define every button, gutter, and diff color by hand:

```toml
[meta]
name = "cyber_peacock"
author = "you"
variant = "dark"
description = "A vivid dark theme for yaran."

[palette]
base = "#101018"
panel = "#171725"
highlight = "#24243a"
active = "#303050"
text = "#f4f7ff"
muted = "#aab2d5"
dim = "#6f7899"
accent = "#00d4ff"
accent_secondary = "#ff4fd8"
border = "#5b6ee1"
success = "#4ade80"
error = "#fb7185"
warning = "#facc15"
info = "#38bdf8"

[tokens]
"text.primary" = "text"
"text.muted" = "muted"
"text.dim" = "dim"
"bg.base" = "base"
"bg.panel" = "panel"
"bg.highlight" = "highlight"
"bg.active" = "active"
"accent.primary" = "accent"
"accent.secondary" = "accent_secondary"
"border.focused" = "border"
"border.unfocused" = "dim"
success = "success"
error = "error"
warning = "warning"
info = "info"
```

Want full control? Add explicit `yaran.*` tokens. The bundled `assets/themes/yaran_dark.toml` is the complete reference, including header chrome, overlays, hints, diffs, help, inputs, and PR colors. PR state colors intentionally default to GitHub-style green, purple, and red so merged, open, and closed states stay recognizable, but you can override `yaran.pr_*` tokens too.

### Keybindings

All keybindings live in the `[keys]` section of the config. Key format supports single characters (`"j"`), special names (`"enter"`, `"pageup"`, `"shift-tab"`), and modifier combos (`"ctrl-d"`, `"ctrl-p"`). Each action takes an array of key combos:

```toml
[keys]
quit = ["ctrl-q"]
open_palette = ["ctrl-p", "ctrl-space"]
```

Press `?` in the app for the full keybinding reference. The help overlay is the authoritative source. This README intentionally doesn't list individual bindings because they're yours to change.

### Logging

Logs go to `yaran.log` in the config directory. Control the level in your config:

```toml
[logging]
level = "info"        # "error", "warn", "info", or "debug"
path = "yaran.log"      # relative to config dir, or use an absolute path
max_bytes = 10485760  # rotate at 10 MiB; rotation is by size only, 0 never rotates
keep = 5              # rotated copies kept as yaran.log.1, yaran.log.2 and so on (max 1000)
compress = true       # gzip them, so they are named yaran.log.1.gz and so on
```

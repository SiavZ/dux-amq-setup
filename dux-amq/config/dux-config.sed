# sed program that install.sh runs over $STATE_ROOT/dux/config.toml
# (`sed -i --follow-symlinks -f dux-config.sed`). It wires each stock provider
# through its AMQ wrapper and applies the overlay's provider tuning.
#
# Each rule rewrites ONE exact line inside ONE provider table, so it only ever
# touches a value that still reads exactly as dux's renderer emits it: a value
# the user already changed no longer matches and is left alone, and a second
# run is a no-op. That is also why the patterns are literal text: when dux
# changes how it renders a default, the rule silently stops matching. The
# `overlay_config` integration test in crates/dux/tests/ runs this file over a
# freshly regenerated config and fails when any rule has gone dead.
#
# dux-config-changes.toml documents the intended end state of these rules in
# config form; keep the two in step.

# [defaults] needs no rule: upstream's stock
# `enable_randomized_pet_name_by_default = false` already starts the new-agent
# prompt empty, which is what the overlay wants (the typed branch name becomes
# the AMQ identity). The fork used to flip `prompt_for_name` for this.

# Route every stock provider through its wrapper.
/^\[providers\.claude\]$/,/^\[/ s|^command = "claude"$|command = "claude-amq"|
/^\[providers\.codex\]$/,/^\[/ s|^command = "codex"$|command = "codex-amq"|
/^\[providers\.jcode\]$/,/^\[/ s|^command = "jcode"$|command = "jcode-amq"|
# gemini is no longer a stock provider upstream (config_migrate prunes an
# untouched stock block), so this only fires for a gemini block the user kept.
/^\[providers\.gemini\]$/,/^\[/ s|^command = "gemini"$|command = "gemini-amq"|

# claude: resume forks the session so deferred tools do not block, and the
# wheel always goes to Claude's own renderer. forward_scroll is tri-state now:
# the stock file leaves it commented out (auto), so uncomment it pinned true.
/^\[providers\.claude\]$/,/^\[/ s|^resume_args = \["--continue"\]$|resume_args = ["--continue", "--fork-session"]|
/^\[providers\.claude\]$/,/^\[/ s|^# forward_scroll = true$|forward_scroll = true|
/^\[providers\.claude\]$/,/^\[/ s|^forward_scroll = false$|forward_scroll = true|
# forward_mouse = false keeps plain drags in dux so text in the pane can be
# selected and copied (bc3a9eec). Current dux renders it false for claude, and
# fills an absent key with that on load; only an explicit `true` in a kept
# config needs rewriting.
/^\[providers\.claude\]$/,/^\[/ s|^forward_mouse = true$|forward_mouse = false|

# codex: run inline (no alt screen) so its output lands in dux's host
# scrollback, and therefore never forward scroll to it.
/^\[providers\.codex\]$/,/^\[/ s|^args = \[\]$|args = ["--no-alt-screen"]|
# codex's targeted resume must keep --no-alt-screen, because resume_by_id_args
# replaces args. dux renders a stock line for it, so this rewrites that line in
# place like every other rule. (It used to be appended, back when dux shipped
# no codex default; appending now would write the key twice, the file would no
# longer parse, and dux would silently fall back to its built-in defaults.)
/^\[providers\.codex\]$/,/^\[/ s|^resume_by_id_args = \["resume", "{session_id}"\]$|resume_by_id_args = ["--no-alt-screen", "resume", "{session_id}"]|
/^\[providers\.codex\]$/,/^\[/ s|^resume_args = \["resume", "--last"\]$|resume_args = ["--no-alt-screen", "resume", "--last"]|
/^\[providers\.codex\]$/,/^\[/ s|^# forward_scroll = true$|forward_scroll = false|
/^\[providers\.codex\]$/,/^\[/ s|^forward_scroll = true$|forward_scroll = false|
# forward_mouse = false keeps plain drags in dux so text in the pane can be
# selected and copied (bc3a9eec). Current dux renders it false for codex, and
# fills an absent key with that on load; only an explicit `true` in a kept
# config needs rewriting.
/^\[providers\.codex\]$/,/^\[/ s|^forward_mouse = true$|forward_mouse = false|

/^\[providers\.gemini\]$/,/^\[/ s|^forward_scroll = false$|forward_scroll = true|

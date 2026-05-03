# Append these to ~/.bashrc to wire dux + AMQ into your shell.

# === dux + AMQ ===
export DUX_HOME="/data/state/dux"
export AMQ_GLOBAL_ROOT="/data/state/amq"
# Co-op aliases (amc=claude, amx=codex) for ad-hoc shell use outside dux.
if command -v amq >/dev/null 2>&1; then
  eval "$(amq shell-setup)"
fi
# Optional YOLO toggle for dux panes:
# export CLAUDE_YOLO=1
#
# Per-pane safe-mode opt-outs (defaults are bypass-all; see README "Security model"):
#   export CLAUDE_AMQ_SAFE=1   # claude-amq: drop --dangerously-skip-permissions
#   export CODEX_AMQ_SAFE=1    # codex-amq:  drop --dangerously-bypass-approvals-and-sandbox
# === end dux + AMQ ===

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
# === end dux + AMQ ===

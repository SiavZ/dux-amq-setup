# Append these to ~/.bashrc to wire dux + AMQ into your shell.
# install.sh substitutes REPLACE_AT_INSTALL with the overlay version
# (audit01 Phase 12) before appending; the >>>/<<< markers are then used
# by strip_block() to delete-and-rewrite on every re-install, so version
# bumps actually propagate.

# >>> dux-amq vREPLACE_AT_INSTALL >>>
export DUX_HOME="/data/state/dux"
export AMQ_GLOBAL_ROOT="/data/state/amq"
# Co-op aliases (amc=claude, amx=codex) for ad-hoc shell use outside dux.
if command -v amq >/dev/null 2>&1; then
  eval "$(amq shell-setup)"
fi
# Optional YOLO toggle for dux panes:
# export CLAUDE_YOLO=1
# <<< dux-amq vREPLACE_AT_INSTALL <<<

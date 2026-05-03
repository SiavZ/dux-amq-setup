#!/usr/bin/env bash
# Finalize migration of ~/.claude and ~/.agents onto persistent disk /data.
# Run this AFTER you've closed every running `claude` process on this VM.
# Idempotent: safe to re-run.
#
# Safety hardening (audit01 P0-4 / audit02 phase 11):
# - Single-instance guard via flock(1) on /tmp/dux-amq-finalize.lock so two
#   simultaneous runs cannot interleave rsync/swap and corrupt state.
# - `ensure_no_claude` is invoked before EVERY destructive operation, not
#   just once at the top — closes the TOCTOU window between the initial
#   pgrep and the actual rsync/mv. The regex matches both `claude` and
#   `claude-amq` so the wrappers count too.
# - rsync no longer passes `--delete` by default. The migration is purely
#   additive: anything that already lives at the persistent destination
#   (e.g. files restored from backup, a co-tenant's content) is preserved.
#   Pass FINALIZE_FORCE_DELETE=1 to opt into the old destructive behaviour.
# - The symlink swap uses the stage-then-rename pattern: `ln -sfn` writes
#   $HOME/.claude.new, then `mv -Tn` atomically replaces $HOME/.claude.
#   On Linux, rename(2) is atomic when both source and target are symlinks
#   in the same directory — this is the only POSIX-portable way to swap
#   a populated tree without a transient missing-symlink window.
#   (macOS `mv` for directories is not atomic the same way, but we are
#   only ever swapping symlinks here, so the technique is portable.)
# - A first-time migration (where ~/.claude is a real directory) is moved
#   aside to ~/.claude.bak.<ts> before the new symlink is installed; we
#   never overwrite live data in place.

set -euo pipefail

# --- 11.1: single-instance guard --------------------------------------------
# `flock -n` returns immediately if another process holds the lock. The fd
# stays open for the lifetime of the script; closing it (script exit) drops
# the lock automatically, so the explicit `rm -f` in the trap is purely
# cosmetic — it just keeps /tmp tidy.
#
# Note: `flock` is util-linux. The persistent-disk migration this script
# performs is documented as Linux-only in dux-amq/README.md, so requiring
# util-linux here is acceptable. macOS users would need `brew install flock`.
LOCK_FILE="/tmp/dux-amq-finalize.lock"
exec 9>"$LOCK_FILE"
if ! flock -n 9; then
  echo "[finalize] another instance is already running (lock: $LOCK_FILE)" >&2
  exit 1
fi
trap 'rm -f "$LOCK_FILE"' EXIT

# --- 11.2: re-checkable claude-running guard --------------------------------
# `pgrep -x` matches the exact process name. The regex `^claude(-amq)?$`
# catches both the `claude` CLI and the `claude-amq` wrapper that dux-amq
# ships in dux-amq/wrappers/. Without the `-amq` alternation a wrapper
# session would slip past this gate.
ensure_no_claude() {
  if pgrep -x 'claude(-amq)?' >/dev/null 2>&1; then
    echo "[finalize] a claude/claude-amq process is running — aborting" >&2
    pgrep -af 'claude(-amq)?' >&2 || true
    exit 1
  fi
}

migrate_dir() {
  local src="$1" dst="$2"
  local bak
  bak="${src}.bak.$(date +%Y%m%d-%H%M%S)"

  # If src is already a symlink, we have nothing to migrate.
  if [[ -L "$src" ]]; then
    echo "[finalize] $src is already a symlink -> $(readlink "$src"). Skipping."
    return 0
  fi

  # Fresh install: src does not exist at all. Just create the destination
  # and a symlink to it. No rsync, no swap.
  if [[ ! -e "$src" ]]; then
    echo "[finalize] $src does not exist. Creating fresh symlink -> $dst."
    mkdir -p "$dst"
    ensure_no_claude
    ln -s "$dst" "$src"
    return 0
  fi

  # First-time migration path: src is a real directory and dst may already
  # contain content (e.g. partial earlier run, restored backup).
  echo "[finalize] delta sync $src/ -> $dst/"
  mkdir -p "$dst"
  ensure_no_claude

  # --- 11.3: drop --delete by default -------------------------------------
  # Default behaviour is now additive: rsync copies new/changed files but
  # never removes anything already present at the destination. Set
  # FINALIZE_FORCE_DELETE=1 to restore the legacy destructive behaviour.
  local rsync_delete=()
  if [[ "${FINALIZE_FORCE_DELETE:-0}" == "1" ]]; then
    rsync_delete+=(--delete)
    echo "[finalize] FINALIZE_FORCE_DELETE=1 -> rsync will delete extra files in $dst" >&2
  fi
  rsync -aH "${rsync_delete[@]}" "$src/" "$dst/"

  # --- 11.4: atomic symlink swap ------------------------------------------
  # Re-check immediately before the swap: a claude process that started
  # during rsync would otherwise be holding open files inside the soon-to-
  # be-renamed directory.
  ensure_no_claude

  # Step 1: back up the live directory to a timestamped sibling so it is
  # not destroyed if anything below this point fails.
  if [[ -d "$src" && ! -L "$src" ]]; then
    echo "[finalize] backing up $src -> $bak"
    mv "$src" "$bak"
  fi

  # Step 2: stage the new symlink at $src.new. `ln -sfn` overwrites a stale
  # staged link from a prior aborted run; it is non-atomic on its own,
  # which is fine because the staged link is not yet $src.
  local staged="${src}.new"
  ln -sfn "$dst" "$staged"

  # Step 3: atomically rename the staged link onto $src. On Linux,
  # rename(2) of a symlink within the same directory is atomic, so any
  # observer either sees the old $src (nonexistent at this point because
  # of step 1, but the principle holds for re-runs) or the new symlink —
  # never an in-between state. `-T` forbids GNU mv's "move into directory"
  # interpretation; `-n` refuses to overwrite an existing target so we
  # fail closed if a concurrent operator beat us to it.
  mv -Tn "$staged" "$src"

  echo "[finalize] $src -> $dst (backup at $bak)"
  echo "[finalize] verify the backup, then: rm -rf $bak"
}

# Initial gate. Re-checks happen inside migrate_dir before each mutate.
ensure_no_claude

migrate_dir "$HOME/.claude"  "/data/state/claude"
migrate_dir "$HOME/.agents"  "/data/state/agents"

# The skills CLI creates RELATIVE symlinks under ~/.claude/skills/ pointing
# to ~/.agents/skills/ via "../../.agents/...". After migration, ~/.claude
# is a symlink to /data/state/claude, so the relative path resolves to
# /data/state/.agents/... — which doesn't exist. Solve once with a sibling
# symlink so present and future skill installs Just Work.
if [[ ! -e /data/state/.agents ]]; then
  echo "[finalize] creating /data/state/.agents -> /data/state/agents (relative-path bridge for skills)"
  ln -s /data/state/agents /data/state/.agents
fi

echo
echo "[finalize] done. ~/.claude and ~/.agents now live on /data/state."

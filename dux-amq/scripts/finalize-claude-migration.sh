#!/usr/bin/env bash
# Finalize migration of ~/.claude and ~/.agents onto persistent disk /data.
# Run this AFTER you've closed every running `claude` process on this VM.
# Idempotent: safe to re-run.
#
# Audit01 P0-4 hardening:
#   - whole script wrapped in flock(1) so concurrent runs are detected;
#   - `claude` re-checked immediately before every destructive op;
#   - rsync is non-destructive by default (use `--force` for `--delete`);
#   - the symlink swap is two same-fs rename(2) calls so ~/.claude is never
#     missing — it is either the original directory, the backup, or the new
#     symlink, never absent;
#   - the /data/state/.agents bridge is checked with `-L` (symlink) not `-e`
#     so we don't no-op when a stale regular file shadows it.

set -euo pipefail

LOCK="${DUX_AMQ_FINALIZE_LOCK:-/tmp/dux-amq-finalize.lock}"
exec 9>"$LOCK"
if ! flock -n 9; then
  echo "ERROR: another finalize/claude run holds $LOCK; aborting." >&2
  exit 1
fi

FORCE_DELETE=0
if [[ "${1:-}" == "--force" ]]; then
  FORCE_DELETE=1
  shift
fi

recheck_no_claude() {
  if pgrep -u "$USER" -fa '(^|/)claude( |$)' >/dev/null; then
    echo "ERROR: a 'claude' process started during migration; aborting." >&2
    pgrep -u "$USER" -fa '(^|/)claude( |$)' >&2
    exit 1
  fi
}

migrate_dir() {
  local src="$1" dst="$2"
  local bak tmp_link ts
  ts="$(date +%Y%m%d-%H%M%S)"
  bak="${src}.bak.${ts}"
  tmp_link="${src}.new-${ts}-$$"

  if [[ -L "$src" ]]; then
    echo "✓ $src is already a symlink → $(readlink "$src"). Skipping."
    return 0
  fi
  if [[ ! -e "$src" ]]; then
    echo "ℹ $src does not exist. Creating fresh symlink → $dst."
    mkdir -p "$dst"
    recheck_no_claude
    ln -s "$dst" "$src"
    return 0
  fi

  echo "→ Final delta rsync $src/ → $dst/"
  mkdir -p "$dst"
  local rsync_flags=(-aH)
  if (( FORCE_DELETE )); then
    rsync_flags+=(--delete)
    echo "  (--force: rsync will use --delete; pre-existing $dst content may be removed)" >&2
  fi
  recheck_no_claude
  rsync "${rsync_flags[@]}" "$src/" "$dst/"

  echo "→ Staging new symlink at $tmp_link → $dst"
  ln -s "$dst" "$tmp_link"

  echo "→ Atomic swap: $src → $bak, $tmp_link → $src"
  recheck_no_claude
  # Two rename(2) calls on the same filesystem; mv -T refuses to follow a
  # symlink-to-dir, ensuring the directory itself is moved.
  mv -T "$src" "$bak"
  if ! mv -T "$tmp_link" "$src"; then
    # Best-effort rollback so we don't leave $src missing.
    echo "ERROR: second rename failed; rolling back $bak → $src" >&2
    mv -T "$bak" "$src" || true
    rm -f "$tmp_link" || true
    exit 1
  fi
  echo "  Backup retained at $bak — delete after verifying:"
  echo "    rm -rf $bak"
}

recheck_no_claude

migrate_dir "$HOME/.claude"  "/data/state/claude"
migrate_dir "$HOME/.agents"  "/data/state/agents"

# The skills CLI creates RELATIVE symlinks under ~/.claude/skills/ pointing to
# ~/.agents/skills/ via "../../.agents/...". After migration, ~/.claude is a
# symlink to /data/state/claude, so the relative path resolves to
# /data/state/.agents/... — which doesn't exist. Solve once with a sibling
# symlink so present and future skill installs Just Work. Use `-L` not `-e`
# so we re-create the symlink if it has been replaced by a regular file.
if [[ ! -L /data/state/.agents ]]; then
  echo "→ Creating /data/state/.agents → /data/state/agents (relative-path bridge for skills)"
  rm -f /data/state/.agents 2>/dev/null || true
  ln -s /data/state/agents /data/state/.agents
fi

echo
echo "✓ Done. ~/.claude and ~/.agents now live on /data/state."

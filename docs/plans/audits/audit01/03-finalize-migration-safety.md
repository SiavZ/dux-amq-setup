# Phase 03: Finalize-migration safety — flock, atomic swap, drop `--delete`

> Maps to audit findings: P0-4

## Goal
Make `finalize-claude-migration.sh` re-entrant-safe and crash-safe. The
one-shot `pgrep claude` check at `:34` then multi-second `rsync` + `mv` +
`ln -s` window can leave `~/.claude` missing or pointing nowhere on a
parallel `claude` spawn or Ctrl-C / spot preempt; `rsync --delete` silently
destroys pre-existing `/data/state/claude` content.

## Pre-conditions
- Phase 00 baseline.
- Test VM/container where we can simulate Ctrl-C mid-migration.

## Files to touch
- `dux-amq/scripts/finalize-claude-migration.sh` — modify.
- `dux-amq/tests/finalize_migration.bats` — create.

## Steps
1. Wrap the script in an `flock` so concurrent runs and concurrent claude
   spawns are detected:
   ```bash
   LOCK="/tmp/dux-amq-finalize.lock"
   exec 9>"$LOCK"
   flock -n 9 || { echo "another finalize/claude run holds $LOCK; aborting" >&2; exit 1; }
   ```
   Document that `claude-amq` should `flock -s 9` shared on the same path
   before destructive ops (future work; for now we rely on user behavior).
2. Re-check before each destructive op (not only at the top):
   ```bash
   recheck_no_claude() {
     pgrep -u "$USER" -fa '(^|/)claude( |$)' >/dev/null && {
       echo "ERROR: claude started during migration; abort." >&2; exit 1; }
   }
   ```
   Call before `rsync`, before `mv`, before `ln -s`.
3. Drop `--delete`. One-shot migration doesn't need it; keep it gated:
   ```bash
   RSYNC_FLAGS=(-aH); [[ "${1:-}" == "--force" ]] && RSYNC_FLAGS+=(--delete)
   rsync "${RSYNC_FLAGS[@]}" "$src/" "$dst/"
   ```
4. Make the symlink swap atomic via two `rename(2)` syscalls on the same fs:
   ```bash
   tmp_link="${src}.new-$(date +%Y%m%d-%H%M%S)-$$"
   ln -s "$dst" "$tmp_link"
   mv -T "$src" "$bak"
   mv -T "$tmp_link" "$src"
   ```
   The remaining tiny window between renames is closed by `flock` +
   `recheck_no_claude` against any local actor.
5. Tighten the bridge symlink: `[[ -L /data/state/.agents ]]` not `-e`.
6. bats: fake `pgrep` returning true on the second call → `recheck_no_claude`
   aborts; `--delete` only via `--force`; SIGKILL between renames →
   `~/.claude` is either symlink or backup, never absent.

## Validation
- `bats dux-amq/tests/finalize_migration.bats` green.
- Manual: pre-existing non-empty `/data/state/claude` survives migration.
- Two parallel runs: second exits 1 with lock message.
- SIGKILL between renames: `~/.claude` always exists.

## Acceptance criteria
- [ ] `flock -n 9` guards the whole script.
- [ ] `recheck_no_claude` before each destructive op.
- [ ] `--delete` only when `--force` passed.
- [ ] Symlink swap uses two `mv -T` against staged tmp targets.
- [ ] `[[ -L /data/state/.agents ]]` check, not `-e`.
- [ ] bats covers lock contention, mid-flight abort, atomicity.

## References
- Audit P0-4.
- `man 2 rename` — atomic on the same filesystem.
- `flock(1)`: https://man7.org/linux/man-pages/man1/flock.1.html

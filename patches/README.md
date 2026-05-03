# `patches/` — overlay-on-upstream Rust patches

This directory keeps our four downstream patches as standalone diffs so the
fork can rebase cleanly on top of `upstream/main` without losing them. The
`.github/workflows/upstream-sync.yml` workflow (Phase 06) opens a weekly
draft PR that merges upstream; if the merge conflicts in any of these
files, regenerate the relevant patch from `HEAD` after resolving.

## Patches

| File | Origin | Audit |
|---|---|---|
| `0001-clipboard-osc52.diff`        | OSC52 / wl-copy fallback in `src/clipboard.rs` | audit01 |
| `0002-auto-resume-on-start.diff`   | Auto-resume sessions on TUI start in `src/app/mod.rs` | audit01 (Phase 09) |
| `0003-scrollbar.diff`              | Scrollbar math + render fix in `src/app/render.rs` | audit01 (Phase 10) |
| `0004-config-auto-resume-field.diff` | `auto_resume` config field in `src/config.rs` | audit01 |

## Rebase recipe (post upstream-sync merge)

When the weekly upstream-sync PR conflicts, the simplest recovery is:

```bash
# 1. Revert our patched files to the upstream version.
git checkout upstream/main -- src/clipboard.rs src/app/mod.rs \
                              src/app/render.rs src/config.rs

# 2. Re-apply each patch.
for p in patches/000*.diff; do
  git apply --3way "$p" || {
    echo "Conflict in $p — open the file, fix, then:"
    echo "  git diff -- <file> > $p"
    exit 1
  }
done

# 3. Stage and commit.
git add src/clipboard.rs src/app/mod.rs src/app/render.rs src/config.rs
git commit -m "rebase: re-apply downstream patches on upstream/main@<sha>"
```

## Regenerating a patch from HEAD

After resolving a conflict by hand:

```bash
# Example for the clipboard patch.
git diff upstream/main..HEAD -- src/clipboard.rs > patches/0001-clipboard-osc52.diff
```

## Verification

CI verifies `git apply --check patches/000*.diff` clean against a tree
where the four files are at `upstream/main`. If any patch fails the check,
it must be regenerated.

Phases 09 (auto-resume) and 10 (scrollbar) regenerate their respective
patches whenever the underlying Rust code changes.

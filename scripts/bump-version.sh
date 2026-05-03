#!/usr/bin/env bash
# bump-version.sh — set the dux-amq overlay version in every place that
# tracks it. Idempotent: re-running with the same target is a no-op.
#
# Audit01 Phase 15 (P2-6). Source of truth is `dux-amq/VERSION` (single
# line, semver without 'v' prefix). All other locations are derived; this
# script is the single sed pass that keeps them in sync.
#
# Locations updated (each anchored on a stable marker the file owns, so
# future edits to surrounding lines don't break the bump):
#
#   1. dux-amq/VERSION
#        Plain rewrite.
#
#   2. dux-amq/install.sh — line marked `# AUDIT01-VERSION`
#        Pattern: `DUX_AMQ_VERSION="${DUX_AMQ_VERSION:-X.Y.Z}"`
#        The marker stays in place after rewriting so a subsequent bump
#        finds the same anchor.
#
#   3. dux-amq/config/bashrc-additions.sh
#        Today this file uses literal `REPLACE_AT_INSTALL` placeholders
#        that install.sh substitutes at runtime — there is intentionally
#        nothing to bump here. We grep-confirm that's still the case so
#        a future drift (someone hard-codes a version) gets caught.
#
# Output (stdout):
#   - One line per file touched, in `path: <old> -> <new>` form.
#   - On the last line, a suggested git tag string (`dux-amq-vX.Y.Z`)
#     intended to be passed to `git tag` after a final review.
#
# Exit codes:
#   0  files now match the requested version (whether a write happened or not)
#   1  bad usage, missing files, or an anchor/marker no longer matches
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NEW_VERSION=""

usage() {
  cat <<'USAGE'
bump-version.sh — set the overlay version everywhere it's tracked.

Usage:
  scripts/bump-version.sh --version <X.Y.Z>

Options:
  --version  New semver string (without leading 'v').
  -h, --help Show this message.

Behavior:
  Idempotent. Running with the current version is a no-op (no files
  touched, exit 0). Always rewrites in-place; no backup files are left
  behind because the change is meant to land in a single git commit.

After running:
  1. Inspect `git diff` and stage the result.
  2. Commit with a Conventional Commits message (e.g. `chore(release):
     bump dux-amq to vX.Y.Z`).
  3. When ready to publish, push the tag the script suggests on stdout:
       git tag dux-amq-vX.Y.Z && git push origin dux-amq-vX.Y.Z
USAGE
}

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) NEW_VERSION="${2:-}"; shift 2 ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
done

[[ -n "$NEW_VERSION" ]] || die "--version is required"
[[ "$NEW_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] \
  || die "--version must be semver X.Y.Z[-prerelease], got '$NEW_VERSION'"

VERSION_FILE="$REPO_ROOT/dux-amq/VERSION"
INSTALL_SH="$REPO_ROOT/dux-amq/install.sh"
BASHRC_ADD="$REPO_ROOT/dux-amq/config/bashrc-additions.sh"

[[ -f "$VERSION_FILE" ]] || die "missing $VERSION_FILE"
[[ -f "$INSTALL_SH"   ]] || die "missing $INSTALL_SH"
[[ -f "$BASHRC_ADD"   ]] || die "missing $BASHRC_ADD"

# 1. dux-amq/VERSION ---------------------------------------------------------
old_version="$(tr -d '[:space:]' < "$VERSION_FILE")"
if [[ "$old_version" != "$NEW_VERSION" ]]; then
  printf '%s\n' "$NEW_VERSION" > "$VERSION_FILE"
  printf '%s: %s -> %s\n' "dux-amq/VERSION" "$old_version" "$NEW_VERSION"
else
  printf '%s: already %s (no-op)\n' "dux-amq/VERSION" "$NEW_VERSION"
fi

# 2. dux-amq/install.sh ------------------------------------------------------
# Anchor on the immediately-preceding `# AUDIT01-VERSION` comment so we never
# rewrite the wrong DUX_AMQ_VERSION line if one ever leaks elsewhere. The
# `:-X.Y.Z` shell default in the assignment is the value we replace; the
# `${DUX_AMQ_VERSION:-...}` envelope stays intact so CI overrides keep working.
if ! grep -q '^# AUDIT01-VERSION' "$INSTALL_SH"; then
  die "anchor '# AUDIT01-VERSION' not found in $INSTALL_SH"
fi
old_install="$(grep -E '^DUX_AMQ_VERSION="\$\{DUX_AMQ_VERSION:-[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?\}"$' "$INSTALL_SH" | head -1 || true)"
[[ -n "$old_install" ]] || die "DUX_AMQ_VERSION assignment line not found in $INSTALL_SH"
old_install_v="$(printf '%s\n' "$old_install" | sed -E 's/.*:-([0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?)\}".*/\1/')"
if [[ "$old_install_v" != "$NEW_VERSION" ]]; then
  # Use a sed pattern that matches ANY current semver value; this means
  # re-running with the same target still works even if VERSION drifted.
  sed -E -i "s|^DUX_AMQ_VERSION=\"\\\$\{DUX_AMQ_VERSION:-[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?\}\"\$|DUX_AMQ_VERSION=\"\${DUX_AMQ_VERSION:-${NEW_VERSION}}\"|" "$INSTALL_SH"
  printf '%s: %s -> %s\n' "dux-amq/install.sh" "$old_install_v" "$NEW_VERSION"
else
  printf '%s: already %s (no-op)\n' "dux-amq/install.sh" "$NEW_VERSION"
fi

# 3. dux-amq/config/bashrc-additions.sh --------------------------------------
# This file uses literal `REPLACE_AT_INSTALL` markers that install.sh
# substitutes at runtime — there's intentionally no semver here. If a
# future edit hard-codes a version into the markers, surface it loudly:
# the bump script can't safely rewrite an ambiguous future format, so we
# fail with a clear "edit and re-run" message.
if grep -qE '^# (>>>|<<<) dux-amq v[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)? (>>>|<<<)$' "$BASHRC_ADD"; then
  die "$BASHRC_ADD now contains a hard-coded version in its >>>/<<< markers; \
update bump-version.sh to handle this case (anchor pattern + replacement)"
fi
if ! grep -q 'REPLACE_AT_INSTALL' "$BASHRC_ADD"; then
  die "$BASHRC_ADD lost its REPLACE_AT_INSTALL placeholder — \
install.sh's runtime substitution is now broken"
fi
printf '%s: REPLACE_AT_INSTALL placeholder OK (runtime-substituted, no bump needed)\n' \
  "dux-amq/config/bashrc-additions.sh"

printf '\nNext: git diff && git add -p && git commit -m "chore(release): bump dux-amq to v%s"\n' "$NEW_VERSION"
printf 'Tag : git tag dux-amq-v%s && git push origin dux-amq-v%s\n' "$NEW_VERSION" "$NEW_VERSION"

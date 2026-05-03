#!/usr/bin/env bash
# release-overlay.sh — produce a versioned overlay release tarball locally.
#
# Audit01 Phase 15 (P2-6). This is the maintainer-side companion to
# .github/workflows/release-overlay.yml: same inputs in, byte-identical
# tarball out. A maintainer can therefore audit what the workflow will
# upload without trusting GitHub's runner — verify the sha256 published
# in the release matches what this script produces against the same git
# ref.
#
# Reproducibility levers (all set explicitly so a `tar` from a different
# distro still emits the same bytes):
#   --sort=name            stable file order (no inode/readdir randomness)
#   --owner=0 --group=0    drop builder uid/gid
#   --numeric-owner        no /etc/passwd lookup
#   --mtime='1970-01-01 …' epoch zero on every file
#   gzip -n                no embedded mtime/filename in gzip header
#   pax format             portable, deterministic header layout
#
# Inputs the user provides:
#   --version <X.Y.Z>      semver tag without the dux-amq-v prefix
#   --output  <dir>        where to drop the tarball + sha256 (default: ./dist)
#   --dry-run              print intended file list and tarball name; no I/O
#
# Inputs the script derives from the working tree:
#   git ref (HEAD)         used in the release-notes summary only
#   $REPO_ROOT/dux-amq/    overlay payload
#   $REPO_ROOT/patches/    Rust-side patch series (kept inside the tarball
#                          so vendoring `dux-amq/` alone is still buildable
#                          against an upstream `dux` checkout)
#   $REPO_ROOT/LICENSE     repo-root MIT (Patrick D'appollonio, dux upstream)
#   $REPO_ROOT/dux-amq/LICENSE  overlay MIT (SiavZ, dux-amq overlay)
#   $REPO_ROOT/docs/plans/audits/audit01/  audit trail referenced from
#                          the README "verified install" section
#
# Output:
#   <output>/dux-amq-vX.Y.Z.tar.gz
#   <output>/dux-amq-vX.Y.Z.tar.gz.sha256
#
# Exit codes:
#   0  success
#   1  bad usage / missing tools / version mismatch with dux-amq/VERSION
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT="$REPO_ROOT/dist"
VERSION=""
DRY_RUN=0

usage() {
  cat <<'USAGE'
release-overlay.sh — build the overlay release tarball.

Usage:
  scripts/release-overlay.sh --version <X.Y.Z> [--output <dir>] [--dry-run]

Options:
  --version  Semver string (without leading 'v'). Must match dux-amq/VERSION
             unless --dry-run; the workflow's tag-vs-VERSION check is mirrored
             here so local builds catch the drift before pushing a tag.
  --output   Output directory (default: ./dist).
  --dry-run  Print the tarball name and intended contents; touch nothing on disk.
  -h, --help Show this message.

Examples:
  scripts/release-overlay.sh --version 0.1.0
  scripts/release-overlay.sh --version 0.2.0 --dry-run
USAGE
}

die() { printf 'error: %s\n' "$*" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --version) VERSION="${2:-}"; shift 2 ;;
    --output)  OUTPUT="${2:-}";  shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    -h|--help) usage; exit 0 ;;
    *) die "unknown argument: $1 (try --help)" ;;
  esac
done

[[ -n "$VERSION" ]] || die "--version is required"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-[A-Za-z0-9.-]+)?$ ]] \
  || die "--version must be semver X.Y.Z[-prerelease], got '$VERSION'"

# Mirror the workflow's tag/VERSION sync check. When the user supplies a
# version that disagrees with dux-amq/VERSION, the workflow will fail fast;
# we surface that locally instead of letting them learn from a red CI run.
VERSION_FILE="$REPO_ROOT/dux-amq/VERSION"
if [[ -f "$VERSION_FILE" ]]; then
  declared="$(tr -d '[:space:]' < "$VERSION_FILE")"
  if [[ "$declared" != "$VERSION" ]]; then
    if [[ $DRY_RUN -eq 1 ]]; then
      printf 'note: --version=%s differs from dux-amq/VERSION=%s (dry-run, continuing)\n' \
        "$VERSION" "$declared" >&2
    else
      die "--version $VERSION differs from dux-amq/VERSION $declared (run scripts/bump-version.sh first)"
    fi
  fi
fi

TAG="dux-amq-v${VERSION}"
TARBALL_BASENAME="${TAG}.tar.gz"

# Stable contents list — keep in sync with .github/workflows/release-overlay.yml.
# The order here is also the order tar will see (tar's --sort=name reorders
# anyway, so this is for human review more than for the bytes).
PAYLOAD_PATHS=(
  "dux-amq"
  "patches"
  "LICENSE"
  "docs/plans/audits/audit01"
)

# Validate each payload path actually exists. Doing this up front means a
# typo or partial checkout fails before we burn time on tar.
for p in "${PAYLOAD_PATHS[@]}"; do
  [[ -e "$REPO_ROOT/$p" ]] || die "payload path missing: $p (are you in a clean checkout?)"
done

if [[ $DRY_RUN -eq 1 ]]; then
  printf 'tarball: %s/%s\n' "$OUTPUT" "$TARBALL_BASENAME"
  printf 'tag:     %s\n' "$TAG"
  printf 'contents (top-level entries inside the tarball, prefix=%s/):\n' "$TAG"
  for p in "${PAYLOAD_PATHS[@]}"; do
    printf '  %s\n' "$p"
  done
  exit 0
fi

# Hard-require GNU tar — BSD tar's --sort/--mtime are different enough to
# break reproducibility silently. Detect explicitly so the failure mode is
# "missing tool" not "different bytes on macOS".
if ! tar --version 2>/dev/null | grep -qi 'gnu tar'; then
  die "GNU tar required for reproducible archive (got: $(tar --version 2>/dev/null | head -1))"
fi
command -v sha256sum >/dev/null 2>&1 || die "sha256sum required (apt-get install coreutils)"
command -v gzip      >/dev/null 2>&1 || die "gzip required"

mkdir -p "$OUTPUT"
TARBALL="$OUTPUT/$TARBALL_BASENAME"
SHAFILE="${TARBALL}.sha256"

# Build the tarball in two stages so we can pin gzip's `-n` (no name, no
# mtime) independently from tar's flags. Piping to `gzip -n` is the only
# way to keep the gzip header deterministic — `tar -z` calls a gzip that
# may or may not pass `-n` depending on the host distro.
#
# `--transform` rewrites every entry's path so the tarball extracts to a
# `dux-amq-vX.Y.Z/` directory regardless of the cwd. Matches softprops'
# "users do `tar xzf` and get a single subdir" expectation.
# shellcheck disable=SC2054  # commas inside --pax-option/--transform are tar syntax, not array separators
TAR_FLAGS=(
  --sort=name
  --owner=0
  --group=0
  --numeric-owner
  --mtime='1970-01-01 00:00:00 UTC'
  --format=pax
  --pax-option=exthdr.name=%d/PaxHeaders/%f,delete=atime,delete=ctime
  --transform "s,^,${TAG}/,"
)

cd "$REPO_ROOT"
# shellcheck disable=SC2068  # word-splitting on TAR_FLAGS is intentional
tar "${TAR_FLAGS[@]}" -cf - "${PAYLOAD_PATHS[@]}" \
  | gzip -n -9 > "$TARBALL"

# sha256: always written as `<hash>  <basename>` so it round-trips through
# `sha256sum -c` regardless of where the user downloaded it to.
( cd "$OUTPUT" && sha256sum "$TARBALL_BASENAME" > "$TARBALL_BASENAME.sha256" )

printf 'wrote %s\n' "$TARBALL"
printf 'wrote %s\n' "$SHAFILE"
printf 'sha256: %s\n' "$(awk '{print $1}' "$SHAFILE")"

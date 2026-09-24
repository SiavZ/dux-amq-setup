#!/usr/bin/env bash
#
# Set the release version in the workspace manifest.
#
# Replaces `cargo set-version` from cargo-edit (a8ea7e79). cargo-edit is an extra
# toolchain dependency for a one-line edit, and one of its releases raised its
# MSRV past the pinned toolchain and broke every build job on the fork's first
# real release.
#
# Every crate declares `version.workspace = true`, so the version lives in
# exactly one line: `version = "..."` under [workspace.package] in the root
# Cargo.toml. This rewrites that line, refuses to guess if the manifest shape
# changes (a crate with its own version, or a second top-level version key), and
# reads the value back. Cargo.lock's workspace entries are refreshed by the build,
# which does not pass --locked.
#
# usage: set_workspace_version.sh <semver> [manifest dir, default: repo root]
set -euo pipefail

VERSION="${1:?usage: set_workspace_version.sh <semver> [dir]}"
DIR="${2:-.}"
MANIFEST="$DIR/Cargo.toml"

if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
  echo "'$VERSION' is not a semver version." >&2
  exit 1
fi

# The line must sit in [workspace.package]: print the section each
# `version = ` line belongs to.
sections="$(awk '/^\[/{s=$0} /^version = /{print s}' "$MANIFEST")"
if [ "$sections" != "[workspace.package]" ]; then
  echo "Expected exactly one top-level 'version =' in $MANIFEST, under" >&2
  echo "[workspace.package]; found it under: ${sections:-<nowhere>}." >&2
  echo "The manifest shape changed; update this script deliberately." >&2
  exit 1
fi

# Every member must inherit it, or setting the workspace version would leave
# that crate at its own number.
for crate_manifest in "$DIR"/crates/*/Cargo.toml; do
  if ! grep -q '^version\.workspace = true' "$crate_manifest"; then
    echo "$crate_manifest does not inherit the workspace version." >&2
    exit 1
  fi
done

# -i.bak keeps this portable across GNU sed (Linux) and BSD sed (macOS).
sed -i.bak -E "s/^version = \".*\"/version = \"${VERSION}\"/" "$MANIFEST"
rm -f "$MANIFEST.bak"

actual="$(grep -m1 '^version = ' "$MANIFEST" | sed -E 's/^version = "(.*)"/\1/')"
if [ "$actual" != "$VERSION" ]; then
  echo "Version rewrite failed: wanted '$VERSION', manifest reads '$actual'." >&2
  exit 1
fi
echo "Workspace version set to $actual"

#!/usr/bin/env bash
#
# Refuse a release archive that was not built from this fork's code.
#
# Why this exists (244b33c6, rustport finding F1): every upstream `v*` tag is an
# ancestor of upstream's main, so this repository's release workflow once built
# and shipped UPSTREAM code under the fork's name for four releases. Those
# binaries had none of the AMQ, peer or watch features the README documents and
# none of the Rust-side threat-model mitigations, and nothing noticed.
#
# The marker is behaviour upstream's binary does not have: the stock config this
# fork renders carries a `[providers.jcode]` table with the targeted-resume key
# `resume_by_id_args`. Upstream (checked against v0.6.0) renders neither.
#
# RUNTIME (archive executable on this host): run `dux config regenerate --yes`
#   against a throwaway DUX_HOME and require the jcode provider table and its
#   resume_by_id_args line in the result. This asserts what users will see.
# STATIC (cross-compiled leg this host cannot execute): require the
#   `resume_by_id_args` string in the binary. Weaker, but it is the same
#   fork-only key, and a skip here would ship that leg unchecked.
#
# usage: verify_fork_lineage.sh <archive.tar.gz> <rust target triple>
# Exits non-zero when the archive is not fork lineage.
set -euo pipefail

ARCHIVE="${1:?usage: verify_fork_lineage.sh <archive.tar.gz> <target>}"
TARGET="${2:?usage: verify_fork_lineage.sh <archive.tar.gz> <target>}"

fail() {
  echo "FATAL: $ARCHIVE is not fork lineage: $*" >&2
  echo "       Release tags for this fork are 'dux-amq-vX.Y.Z' and must point at" >&2
  echo "       this repository's code, not an upstream 'v*' tag." >&2
  exit 1
}

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

tar -xzf "$ARCHIVE" -C "$WORK" dux
BIN="$WORK/dux"
[ -x "$BIN" ] || fail "the archive holds no executable 'dux'"

case "$TARGET" in
  x86_64-*) want_arch="x86_64|amd64" ;;
  aarch64-*) want_arch="aarch64|arm64" ;;
  *) want_arch="" ;;
esac
case "$TARGET" in
  *-linux-*) want_os="Linux" ;;
  *-apple-darwin) want_os="Darwin" ;;
  *) want_os="" ;;
esac

runnable=1
if [ -z "$want_arch" ] || [ -z "$want_os" ] \
  || [ "$(uname -s)" != "$want_os" ] \
  || ! printf '%s\n' "$(uname -m)" | grep -Eqx "$want_arch"; then
  runnable=0
fi

if [ "$runnable" -eq 1 ]; then
  mkdir -p "$WORK/home"
  if ! DUX_HOME="$WORK/home" "$BIN" config regenerate --yes >"$WORK/regen.log" 2>&1; then
    cat "$WORK/regen.log" >&2
    fail "'dux config regenerate --yes' failed"
  fi
  config="$WORK/home/config.toml"
  [ -f "$config" ] || fail "'dux config regenerate --yes' wrote no config.toml"
  grep -q '^\[providers\.jcode\]' "$config" \
    || fail "the rendered default config has no [providers.jcode] table"
  grep -q '^resume_by_id_args = ' "$config" \
    || fail "the rendered default config has no resume_by_id_args"
  echo "OK (runtime): $ARCHIVE renders the fork's jcode provider and resume_by_id_args."
else
  # grep -a on the binary: the key is a string literal in the config renderer.
  grep -a -q 'resume_by_id_args' "$BIN" \
    || fail "the binary does not contain the fork-only 'resume_by_id_args' key"
  echo "OK (static): $TARGET cannot run on $(uname -s)/$(uname -m); the binary"
  echo "             contains the fork-only 'resume_by_id_args' key."
fi

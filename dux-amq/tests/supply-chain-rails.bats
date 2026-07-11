#!/usr/bin/env bats

setup() {
  REPO_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
}

@test "Claude Peers install checks out and verifies one exact commit" {
  local installer="$REPO_ROOT/dux-amq/install.sh"
  local revision
  # The pin is env-overridable: CLAUDE_PEERS_REV="${CLAUDE_PEERS_REV:-<sha>}".
  # Extract the 40-char default SHA regardless of the ${VAR:-...} wrapper.
  revision=$(grep -m1 '^CLAUDE_PEERS_REV=' "$installer" | grep -oE '[0-9a-f]{40}')
  [[ "$revision" =~ ^[0-9a-f]{40}$ ]]
  grep -Fq -- 'git -C "$CLAUDE_PEERS_DIR" checkout --detach "$CLAUDE_PEERS_REV"' "$installer"
  grep -Fq -- 'peers_head=$(git -C "$CLAUDE_PEERS_DIR" rev-parse HEAD' "$installer"
  grep -Fq -- '[[ "$peers_head" != "$CLAUDE_PEERS_REV" ]]' "$installer"
}

@test "every workflow cargo install has an explicit version" {
  local installs
  installs=$(grep -h 'cargo install' "$REPO_ROOT"/.github/workflows/*.yml)
  [ -n "$installs" ]
  while IFS= read -r install_line; do
    [[ "$install_line" == *"--version "* ]] || {
      printf 'unpinned cargo install: %s\n' "$install_line" >&2
      return 1
    }
  done <<<"$installs"
}

@test "release packaging pins a valid source epoch and compares two archives" {
  local workflow="$REPO_ROOT/.github/workflows/release.yml"
  grep -Fq -- 'SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)' "$workflow"
  grep -Fq -- '--mtime="@${SOURCE_DATE_EPOCH}"' "$workflow"
  grep -Fq -- 'package "${{ matrix.archive }}.first"' "$workflow"
  grep -Fq -- 'cmp "${{ matrix.archive }}.first" "${{ matrix.archive }}"' "$workflow"

  local tar_bin="tar"
  command -v gtar >/dev/null 2>&1 && tar_bin="gtar"
  "$tar_bin" --version 2>/dev/null | grep -Fq -- "GNU tar" || skip "GNU tar unavailable"
  local epoch fixture first second
  epoch=$(git -C "$REPO_ROOT" log -1 --format=%ct)
  [[ "$epoch" =~ ^[0-9]+$ ]]
  fixture="$BATS_TEST_TMPDIR/release"
  first="$BATS_TEST_TMPDIR/first.tar.gz"
  second="$BATS_TEST_TMPDIR/second.tar.gz"
  mkdir -p "$fixture"
  printf 'release fixture\n' >"$fixture/dux"
  "$tar_bin" --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$epoch" -czf "$first" -C "$fixture" dux
  "$tar_bin" --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$epoch" -czf "$second" -C "$fixture" dux
  cmp "$first" "$second"
}

@test "RustSec exceptions carry exact rationale and a review date everywhere" {
  local file
  for file in \
    "$REPO_ROOT/deny.toml" \
    "$REPO_ROOT/.github/workflows/test.yml" \
    "$REPO_ROOT/.github/workflows/pr.yml"; do
    grep -Fq -- 'RUSTSEC-2025-0141' "$file"
    grep -Fq -- 'RUSTSEC-2024-0384' "$file"
    grep -Fq -- '2026-08-01' "$file"
    grep -Eiq -- 'unmaintained|no patched|no upstream replacement' "$file"
  done
}

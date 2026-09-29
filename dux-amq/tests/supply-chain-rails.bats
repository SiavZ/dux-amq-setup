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
  # 4f572aac: without `gzip -n` the gzip header carries wall-clock time.
  grep -Fq -- "--use-compress-program='gzip -n'" "$workflow"
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
    --mtime="@$epoch" --use-compress-program='gzip -n' -cf "$first" -C "$fixture" dux
  sleep 1.1
  "$tar_bin" --sort=name --owner=0 --group=0 --numeric-owner \
    --mtime="@$epoch" --use-compress-program='gzip -n' -cf "$second" -C "$fixture" dux
  cmp "$first" "$second"
}

# 244b33c6 / a8ea7e79: the release strips the fork's `dux-amq-` tag prefix,
# sets the version without cargo-edit, and refuses to publish a non-fork build.
@test "release builds this fork's code under dux-amq tags, without cargo-edit" {
  local workflow="$REPO_ROOT/.github/workflows/release.yml"
  grep -Fq -- 'VERSION="${VERSION#dux-amq-}"' "$workflow"
  ! grep -Ev '^[[:space:]]*#' "$workflow" | grep -q -- 'cargo-edit\|cargo set-version' || false
  grep -Fq -- '.github/scripts/set_workspace_version.sh' "$workflow"
  grep -Fq -- '.github/scripts/verify_fork_lineage.sh' "$workflow"
  grep -Fq -- 'actions/attest-build-provenance@' "$workflow"
  grep -Fq -- 'cargo auditable build --release' "$workflow"
  # The retired macos-13 label queues forever instead of failing (6e5a3a6c).
  ! grep -q -- 'macos-13' "$workflow" || false
}

@test "set_workspace_version rewrites the single workspace version" {
  local ws="$BATS_TEST_TMPDIR/ws"
  mkdir -p "$ws/crates/a" "$ws/crates/b"
  printf '[workspace]\nmembers = ["crates/a"]\n\n[workspace.package]\nversion = "0.1.0"\nrust-version = "1.88"\n\n[workspace.dependencies]\nfoo = { version = "1" }\n' >"$ws/Cargo.toml"
  printf '[package]\nname = "a"\nversion.workspace = true\n' >"$ws/crates/a/Cargo.toml"
  printf '[package]\nname = "b"\nversion.workspace = true\n' >"$ws/crates/b/Cargo.toml"

  run "$REPO_ROOT/.github/scripts/set_workspace_version.sh" 1.2.3-rc.1 "$ws"
  [ "$status" -eq 0 ]
  grep -Fxq 'version = "1.2.3-rc.1"' "$ws/Cargo.toml"
  grep -Fq 'foo = { version = "1" }' "$ws/Cargo.toml"

  run "$REPO_ROOT/.github/scripts/set_workspace_version.sh" dux-amq-v1.2.3 "$ws"
  [ "$status" -ne 0 ]

  # A crate carrying its own version would silently keep it: refuse.
  printf '[package]\nname = "b"\nversion = "9.9.9"\n' >"$ws/crates/b/Cargo.toml"
  run "$REPO_ROOT/.github/scripts/set_workspace_version.sh" 2.0.0 "$ws"
  [ "$status" -ne 0 ]
  [[ "$output" == *"does not inherit the workspace version"* ]]
  grep -Fxq 'version = "1.2.3-rc.1"' "$ws/Cargo.toml"
}

@test "set_workspace_version works on this repository's real manifest" {
  local ws="$BATS_TEST_TMPDIR/real"
  mkdir -p "$ws/crates"
  cp "$REPO_ROOT/Cargo.toml" "$ws/Cargo.toml"
  local crate
  for crate in "$REPO_ROOT"/crates/*/; do
    mkdir -p "$ws/crates/$(basename "$crate")"
    cp "$crate/Cargo.toml" "$ws/crates/$(basename "$crate")/Cargo.toml"
  done
  run "$REPO_ROOT/.github/scripts/set_workspace_version.sh" 0.2.0 "$ws"
  [ "$status" -eq 0 ]
  grep -Fxq 'version = "0.2.0"' "$ws/Cargo.toml"
}

# A fake `dux` whose rendered config does or does not carry the fork's jcode
# provider, packaged the way release.yml packages it.
make_lineage_archive() {  # $1 = archive path, $2 = "fork" | "upstream"
  local dir="$BATS_TEST_TMPDIR/pkg-$2"
  mkdir -p "$dir"
  if [[ "$2" == fork ]]; then
    cat >"$dir/dux" <<'EOF'
#!/usr/bin/env bash
mkdir -p "$DUX_HOME"
printf '[providers.jcode]\ncommand = "jcode"\nresume_by_id_args = ["--resume", "{session_id}"]\n' >"$DUX_HOME/config.toml"
EOF
  else
    cat >"$dir/dux" <<'EOF'
#!/usr/bin/env bash
mkdir -p "$DUX_HOME"
printf '[providers.claude]\ncommand = "claude"\n' >"$DUX_HOME/config.toml"
EOF
  fi
  chmod 0755 "$dir/dux"
  tar -czf "$1" -C "$dir" dux
}

host_target() {
  local arch os
  case "$(uname -m)" in x86_64|amd64) arch=x86_64 ;; *) arch=aarch64 ;; esac
  case "$(uname -s)" in Darwin) os=apple-darwin ;; *) os=unknown-linux-musl ;; esac
  printf '%s-%s\n' "$arch" "$os"
}

@test "verify_fork_lineage accepts a fork build and rejects an upstream one" {
  local script="$REPO_ROOT/.github/scripts/verify_fork_lineage.sh"
  make_lineage_archive "$BATS_TEST_TMPDIR/fork.tar.gz" fork
  make_lineage_archive "$BATS_TEST_TMPDIR/upstream.tar.gz" upstream

  run "$script" "$BATS_TEST_TMPDIR/fork.tar.gz" "$(host_target)"
  [ "$status" -eq 0 ]
  [[ "$output" == *"OK (runtime)"* ]]

  run "$script" "$BATS_TEST_TMPDIR/upstream.tar.gz" "$(host_target)"
  [ "$status" -ne 0 ]
  [[ "$output" == *"not fork lineage"* ]]

  # A leg this host cannot run falls back to the static string check, which
  # still tells the two apart.
  run "$script" "$BATS_TEST_TMPDIR/fork.tar.gz" riscv64gc-unknown-linux-gnu
  [ "$status" -eq 0 ]
  [[ "$output" == *"OK (static)"* ]]
  run "$script" "$BATS_TEST_TMPDIR/upstream.tar.gz" riscv64gc-unknown-linux-gnu
  [ "$status" -ne 0 ]
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

# main's branch protection requires these exact contexts. A required context
# that no workflow reports blocks every PR, and upstream's CI names its jobs
# differently, so an upstream merge must not silently rename them.
@test "CI emits every status check main's branch protection requires" {
  local wf="$REPO_ROOT/.github/workflows" name
  for name in "Test (ubuntu-24.04)" "Test (macos-14)" "Security"; do
    grep -Fxq -- "    name: $name" "$wf/pr.yml"
    grep -Fxq -- "    name: $name" "$wf/test.yml"
  done
  grep -Eq '^  shell:$' "$wf/overlay-ci.yml"
  ! grep -Eq '^    name:' "$wf/overlay-ci.yml" || false
}

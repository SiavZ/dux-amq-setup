#!/usr/bin/env bats
#
# Audit02 Phase 02 regressions:
#   - P0-F/P0-01: re-running install.sh must not wipe AMQ queue config
#           (`$STATE_ROOT/amq/meta/config.json` is the marker file —
#           verified by running `amq init --root /tmp/x ...` against the
#           pinned v0.34.0 binary; *not* the `agents.json` the audit02
#           plan originally assumed) or user-owned Dux configuration.
#   - N-3:  the bashrc shell-setup guard must fail *closed* when the
#           binary is present but `binary.sha256` is missing.
#
# These tests run the real install.sh with $STATE_ROOT pointed at a
# throwaway dir under $TEST_HOME, and rely on `dux`/`amq` already being
# on $PATH (they are on the dux VM and on the overlay-CI runner).

load 'lib/setup'

setup() {
  setup_isolated_home
  REPO_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  export STATE_ROOT="$TEST_HOME/state"
  # Preflight expects `/data` to exist; it does on the VM and CI runner.
}

teardown() {
  teardown_isolated_home
}

# Skip if any of the heavyweight prerequisites for an end-to-end install
# are missing (no /data mount, dux/amq not on PATH). overlay-bats on the
# VM/CI satisfies these; a developer laptop without /data does not.
require_install_env() {
  [[ -d /data ]]                 || skip "/data not mounted"
  command -v dux >/dev/null      || skip "dux not on PATH"
  command -v amq >/dev/null      || skip "amq not on PATH"
}

# Strip the `tests/fakes/` directory from $PATH for tests that drive the
# real install.sh end-to-end. `setup_isolated_home` prepends `tests/fakes/`
# so wrapper tests can shadow `amq`, but install.sh resolves `amq` via
# `command -v` and then sha256-verifies the resolved binary against the
# pinned production hash (AMQ_BINARY_SHA256). The Phase 01 fake `amq`
# obviously does not match that pin, so install.sh aborts before it can
# create `$STATE_ROOT/amq/meta/config.json` (P0-F) or pin the binary at
# `$STATE_ROOT/amq-bin/amq` (N-3).
#
# These two tests assume a real, hash-pinned amq on PATH (see
# require_install_env above); restoring PATH to that real binary is the
# only test-only mutation needed — production install.sh is untouched.
unshadow_real_amq_on_path() {
  local fakes="$BATS_TEST_DIRNAME/fakes"
  # Drop `$fakes` (and any duplicate occurrences) from $PATH while
  # preserving every other entry's order.
  local IFS=':' new=() entry
  for entry in $PATH; do
    [[ "$entry" == "$fakes" ]] && continue
    new+=("$entry")
  done
  PATH="$(IFS=':'; printf '%s' "${new[*]}")"
  export PATH
}

@test "P0-01: existing Dux config survives a hermetic installer rerun" {
  [[ "$(uname -s)" == "Linux" ]] || skip "Linux installer fixture"
  cd "$REPO_ROOT"

  local fake_bin="$TEST_HOME/bin"
  mkdir -p "$fake_bin" "$STATE_ROOT/amq/meta" "$STATE_ROOT/dux"
  cat >"$fake_bin/dux" <<'SH'
#!/usr/bin/env bash
if [[ "${1:-}" == "config" ]]; then
  printf 'dux config must not run for an existing config\n' >&2
  exit 97
fi
printf 'dux fixture\n'
SH
  chmod +x "$fake_bin/dux"

  local config_target="$STATE_ROOT/dux/user-config.toml"
  cat >"$config_target" <<'TOML'
# user-owned header comment
prompt_for_name = true

[providers.claude]
command = "claude-amq"
args = ["--custom-claude-arg"]
resume_args = ["--continue", "--fork-session"]
forward_scroll = true
forward_mouse = false

[providers.codex]
command = "codex-amq"
args = ["--no-alt-screen", "--model", "audit03-custom"]
forward_scroll = false
forward_mouse = false

[providers.gemini]
command = "gemini-amq"
args = []
forward_scroll = true

[macros]
"Audit03" = { text = "keep me", surface = "both" }

[[projects]]
id = "audit03-project"
path = "/tmp/audit03-project"
name = "Audit03 Project"

[audit03_unknown]
custom_key = "preserve-me"
prompt_for_name = false

[providers.audit03]
command = "claude"
args = ["--custom", "preserve"]
resume_args = ["--continue"]
TOML
  ln -s "$config_target" "$STATE_ROOT/dux/config.toml"
  cp "$config_target" "$STATE_ROOT/dux/config.toml.expected"
  printf '{"agents":[]}\n' >"$STATE_ROOT/amq/meta/config.json"

  # Keep the production script on normal Linux tools while hiding optional
  # host npx/bun/claude installations that would add unrelated network work.
  export PATH="$fake_bin:$BATS_TEST_DIRNAME/fakes:/usr/local/bin:/usr/bin:/bin"
  local fake_amq_sha
  fake_amq_sha="$(sha256sum "$BATS_TEST_DIRNAME/fakes/amq" | awk '{print $1}')"
  run env AMQ_BINARY_SHA256="$fake_amq_sha" ./dux-amq/install.sh
  [ "$status" -eq 0 ] || {
    printf 'hermetic installer failed:\n%s\n' "$output" >&2
    return 1
  }

  [ -L "$STATE_ROOT/dux/config.toml" ]
  cmp "$STATE_ROOT/dux/config.toml.expected" "$config_target"
  ! grep -Fq -- "/data/state" "$HOME/.bashrc"
  run env -u STATE_ROOT -u DUX_HOME -u AMQ_GLOBAL_ROOT \
    HOME="$HOME" bash -c 'source "$HOME/.bashrc"; printf "%s|%s|%s\n" "$STATE_ROOT" "$DUX_HOME" "$AMQ_GLOBAL_ROOT"'
  [ "$status" -eq 0 ]
  [[ "$output" == *"$STATE_ROOT|$STATE_ROOT/dux|$STATE_ROOT/amq"* ]]
}

@test "P0-F/P0-01: second install preserves AMQ and user-owned Dux config" {
  require_install_env
  unshadow_real_amq_on_path
  cd "$REPO_ROOT"
  ./dux-amq/install.sh >/dev/null
  [ -f "$STATE_ROOT/amq/meta/config.json" ]
  # Tag the existing config with a sentinel an honest re-run must preserve.
  # `amq init --force` would overwrite this; the gated install must not.
  python3 - <<'PY'
import json, os
p = os.path.join(os.environ["STATE_ROOT"], "amq", "meta", "config.json")
with open(p) as fh: cfg = json.load(fh)
cfg["custom_marker"] = "preserved-by-idempotent-install"
with open(p, "w") as fh: json.dump(cfg, fh)
PY
  # Exercise every user-owned class called out by P0-01: comments, unknown
  # keys, projects, macros, and custom provider arguments. The install-owned
  # settings were already patched on the first run, so the entire file must be
  # byte-for-byte stable across the second run.
  python3 - <<'PY'
import os

p = os.path.join(os.environ["STATE_ROOT"], "dux", "config.toml")
with open(p) as fh:
    text = fh.read()

start = text.index("[providers.codex]")
end = text.find("\n[", start + 1)
section = text[start:] if end == -1 else text[start:end]
assert 'args = ["--no-alt-screen"]' in section
custom = section.replace(
    'args = ["--no-alt-screen"]',
    'args = ["--no-alt-screen", "--model", "audit03-custom"]',
    1,
)
text = text[:start] + custom + ("" if end == -1 else text[end:])

assert "[macros]\n" in text
text = text.replace(
    "[macros]\n",
    '[macros]\n"Audit03" = { text = "keep me", surface = "both" }\n',
    1,
)
text += '''
# audit03 user comment
[[projects]]
id = "audit03-project"
path = "/tmp/audit03-project"
name = "Audit03 Project"

[audit03_unknown]
custom_key = "preserve-me"
prompt_for_name = false

[providers.audit03]
command = "claude"
args = ["--custom", "preserve"]
resume_args = ["--continue"]
'''
with open(p, "w") as fh:
    fh.write(text)
with open(p + ".expected", "w") as fh:
    fh.write(text)
PY
  ./dux-amq/install.sh >/dev/null
  grep -q "preserved-by-idempotent-install" "$STATE_ROOT/amq/meta/config.json"
  cmp "$STATE_ROOT/dux/config.toml.expected" "$STATE_ROOT/dux/config.toml"
}

@test "N-3: shell-setup guard refuses to eval when binary.sha256 record is removed" {
  require_install_env
  unshadow_real_amq_on_path
  cd "$REPO_ROOT"
  ./dux-amq/install.sh >/dev/null
  rm -f "$STATE_ROOT/amq/binary.sha256"
  # Run a fresh non-interactive bash that sources the additions file with
  # AMQ_BIN/AMQ_GLOBAL_ROOT pointed at the just-installed pin. The guard
  # must `return 1` (i.e. the trailing `_amq_shell_setup_guarded` call
  # must non-zero out, propagated as the script's exit status under
  # `set -e`).
  run env \
    AMQ_BIN="$STATE_ROOT/amq-bin/amq" \
    AMQ_GLOBAL_ROOT="$STATE_ROOT/amq" \
    bash -c 'set -e; source dux-amq/config/bashrc-additions.sh'
  [ "$status" -ne 0 ]
}

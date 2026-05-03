#!/usr/bin/env bash
# install.sh — set up dux + AMQ on a Linux VM with a persistent disk at /data.
# Idempotent: re-run at will. Won't move files on the boot disk if /data is
# missing — bails early.
#
# Supply-chain pins (audit01 / P0-2). Update these together when bumping a
# dependency; recompute hashes against fresh downloads (see Validation section
# in docs/plans/audits/audit01/01-supply-chain-hardening.md).
#
#   dux        v0.4.0
#     tarball: dux-linux-amd64.tar.gz
#     sha256:  a1c449989e9c4dd53b260d75d29d0d5d6832b3852cf5327f3725b5e7bb881102
#
#   amq        v0.34.0   (commit 6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54)
#     tarball: amq_0.34.0_linux_amd64.tar.gz
#     sha256:  cba940987d00a3d072f395c7ec7a648e47d652f1ff503abf46da538595510d7a
#
#   skills     1.5.3 (npm)
#     skills-rev (avivsinai/agent-message-queue commit pinned for `skills add`)
#                6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54
#
# dux-amq lib distribution (audit01 Stage 3a / wrapper-chain integration).
# The wrappers source path-encode.sh and wake-launch.sh from a search path.
# Resolution order (first hit wins, see _dux_amq_lib_locate in each wrapper):
#   1. $DUX_AMQ_LIB           — env var override (e.g. dev checkouts)
#   2. <wrapper>/../lib       — sibling of the bin dir (matches dev tree)
#   3. <wrapper>/../share/dux-amq/lib  — XDG-style install layout (default)
#   4. /data/state/dux-amq/lib         — system-wide on the persistent disk
#   5. /usr/local/share/dux-amq/lib    — FHS fallback for root-managed installs
# This script installs the lib files to slot 3 (under $LOCAL_BIN/../share/...)
# so a non-root install lands at a path the wrapper will resolve without needing
# DUX_AMQ_LIB to be set. Override via DUX_AMQ_LIB_DEST if you need a different
# target (e.g. /usr/local/share/dux-amq/lib for a root install).
set -euo pipefail

STATE_ROOT="${STATE_ROOT:-/data/state}"
LOCAL_BIN="${HOME}/.local/bin"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Pinned versions + sha256 (overrideable for testing only; CI must use defaults).
DUX_TAG="${DUX_TAG:-v0.4.0}"
DUX_SHA256="${DUX_SHA256:-a1c449989e9c4dd53b260d75d29d0d5d6832b3852cf5327f3725b5e7bb881102}"
AMQ_TAG="${AMQ_TAG:-v0.34.0}"
AMQ_VERSION="${AMQ_VERSION:-0.34.0}"
AMQ_SHA256="${AMQ_SHA256:-cba940987d00a3d072f395c7ec7a648e47d652f1ff503abf46da538595510d7a}"
SKILLS_PIN="${SKILLS_PIN:-1.5.3}"
SKILLS_REV="${SKILLS_REV:-6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54}"

# Expected sha256 of the extracted amq binary (audit01 P1-8). Cross-checked
# against the file inside amq_${AMQ_VERSION}_linux_amd64.tar.gz at install
# time so a tampered-with binary already in $PATH is rejected before being
# pinned at $STATE_ROOT/amq-bin/amq.
AMQ_BINARY_SHA256="${AMQ_BINARY_SHA256:-eb78901f3dd13534884923e02ad9c6852be1b0a4c7f452fe52b8bcd795e3556b}"

# AUDIT01-VERSION — overlay version; gates idempotent config-block rewrites
# (Phase 12). Phase 15's release pipeline rewrites this line on tag.
DUX_AMQ_VERSION="${DUX_AMQ_VERSION:-0.1.0}"

say()  { printf '\033[1;34m→\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[1;32m✓\033[0m %s\n' "$*"; }

# Verify a downloaded artifact's sha256 against an expected value. Bails out
# on mismatch — the calling install branch must not proceed.
verify_sha256() {
  local file="$1" expected="$2" label="$3" actual
  actual=$(sha256sum "$file" | awk '{print $1}')
  if [[ "$actual" != "$expected" ]]; then
    warn "$label sha256 mismatch: got $actual, expected $expected"
    exit 1
  fi
  ok "$label sha256 verified ($actual)"
}

# Audit01 P1-7: strip any prior dux-amq versioned block from a config file so
# the next append always lands a clean current-version block. Also migrates
# the legacy unversioned `=== dux + AMQ ===`/`Multi-agent environment (AMQ +
# dux)` blocks. `kind` selects the marker style:
#   sh  → `# >>> dux-amq vN.M.K >>>` … `# <<< dux-amq vN.M.K <<<`
#   md  → `<!-- >>> dux-amq vN.M.K >>> -->` … `<!-- <<< dux-amq vN.M.K <<< -->`
strip_block() {
  local file="$1" kind="${2:-sh}"
  [[ -f "$file" ]] || return 0
  local tmp; tmp=$(mktemp "${file}.dux-amq.XXXXXX")
  case "$kind" in
    sh)
      awk '
        /^# >>> dux-amq v[^ ]+ >>>$/ {s=1; next}
        /^# <<< dux-amq v[^ ]+ <<<$/ {s=0; next}
        # Legacy (audit01 pre-Phase-12) markers — migrate by stripping.
        /^# === dux \+ AMQ ===$/        {s=1; next}
        /^# === end dux \+ AMQ ===$/    {s=0; next}
        !s
      ' "$file" > "$tmp"
      ;;
    md)
      awk '
        /^<!-- >>> dux-amq v[^ ]+ >>> -->$/ {s=1; next}
        /^<!-- <<< dux-amq v[^ ]+ <<< -->$/ {s=0; next}
        # Legacy: section heading through end of file is the entire block.
        /^## Multi-agent environment \(AMQ \+ dux\)$/ {s=1; next}
        !s
      ' "$file" > "$tmp"
      ;;
    *) warn "strip_block: unknown kind: $kind"; rm -f "$tmp"; return 1 ;;
  esac
  mv "$tmp" "$file"
}

# 1. preflight ---------------------------------------------------------------
[[ -d /data ]] || { warn "/data not mounted — set up a persistent disk first."; exit 1; }
# Audit01 P1-6: hard-fail on missing tools instead of letting individual install
# branches discover them later (with confusing errors). `jq` was a soft dep at
# the VSCode-settings step; promote to required so we can drop the non-portable
# `grep -oP` PCRE scrape entirely.
for _tool in curl jq sha256sum tar install git rsync awk sed; do
  command -v "$_tool" >/dev/null 2>&1 || {
    warn "missing required tool: $_tool (Debian/Ubuntu: apt-get install -y curl jq tar coreutils git rsync gawk sed)"
    exit 1
  }
done
unset _tool
mkdir -p "$STATE_ROOT"/{claude,agents,codex,gemini,dux,amq,worktrees,scripts} "$LOCAL_BIN"
ok "state dirs ready under $STATE_ROOT"

# 2. dux ---------------------------------------------------------------------
if ! command -v dux >/dev/null 2>&1; then
  say "installing dux $DUX_TAG"
  TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
  curl -fsSL -o "$TMP/dux.tar.gz" \
    "https://github.com/patrickdappollonio/dux/releases/download/${DUX_TAG}/dux-linux-amd64.tar.gz"
  verify_sha256 "$TMP/dux.tar.gz" "$DUX_SHA256" "dux ${DUX_TAG}"
  tar -xzf "$TMP/dux.tar.gz" -C "$TMP"
  install -m 0755 "$TMP/dux" "$LOCAL_BIN/dux"
  rm -rf "$TMP"; trap - EXIT
fi
ok "dux: $(dux --help 2>&1 | head -1 || echo installed)"

# 3. AMQ ---------------------------------------------------------------------
# Bypass the upstream `curl … | bash` install script entirely: download the
# pinned release tarball, verify sha256, install the binary directly. The
# upstream installer's behavior (paths, side effects) is then irrelevant to
# our trust boundary. Install log is teed to $STATE_ROOT/amq/install.log so
# stderr is never silenced.
if ! command -v amq >/dev/null 2>&1; then
  say "installing amq $AMQ_TAG"
  AMQ_LOG="$STATE_ROOT/amq/install.log"
  : > "$AMQ_LOG"
  TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
  {
    echo "[$(date -u +%FT%TZ)] downloading amq ${AMQ_TAG}"
    curl -fsSL -o "$TMP/amq.tar.gz" \
      "https://github.com/avivsinai/agent-message-queue/releases/download/${AMQ_TAG}/amq_${AMQ_VERSION}_linux_amd64.tar.gz"
    verify_sha256 "$TMP/amq.tar.gz" "$AMQ_SHA256" "amq ${AMQ_TAG}"
    tar -xzf "$TMP/amq.tar.gz" -C "$TMP"
    install -m 0755 "$TMP/amq" "$LOCAL_BIN/amq"
    echo "[$(date -u +%FT%TZ)] amq installed to $LOCAL_BIN/amq"
  } 2>&1 | tee -a "$AMQ_LOG"
  rm -rf "$TMP"; trap - EXIT
fi
amq init --root "$STATE_ROOT/amq" --agents claude,codex,gemini --force >/dev/null
chmod 700 "$STATE_ROOT/amq"
ok "amq queue at $STATE_ROOT/amq"

# Audit01 P1-8: pin amq at a controlled absolute path under $STATE_ROOT and
# record its sha256, so the bashrc guard (in bashrc-additions.sh) can refuse
# to source `amq shell-setup` if the binary on disk no longer matches.
# Without this guard, every interactive shell start would `eval` whatever the
# `amq` binary in PATH chose to print — a much larger trust radius than the
# install-time pin we just verified above.
#
# Before pinning, verify the binary about to be copied matches AMQ_BINARY_SHA256
# (cross-checked against the extracted tarball). This catches the case where
# the user already has a tampered `amq` in PATH from an earlier untrusted
# install and the Phase 01 tarball-download branch was skipped.
AMQ_BIN_DIR="$STATE_ROOT/amq-bin"
AMQ_BIN_PINNED="$AMQ_BIN_DIR/amq"
AMQ_BIN_SOURCE="$(command -v amq)"
mkdir -p "$AMQ_BIN_DIR"
verify_sha256 "$AMQ_BIN_SOURCE" "$AMQ_BINARY_SHA256" "amq binary"
install -m 0755 "$AMQ_BIN_SOURCE" "$AMQ_BIN_PINNED"
sha256sum "$AMQ_BIN_PINNED" > "$STATE_ROOT/amq/binary.sha256"
chmod 0644 "$STATE_ROOT/amq/binary.sha256"
ok "amq binary pinned at $AMQ_BIN_PINNED ($(awk '{print $1}' "$STATE_ROOT/amq/binary.sha256"))"

# 4. AMQ skills (gives Claude/etc. native knowledge of amq) ------------------
# Pin the npm package version, pin the skills-source git ref, block postinstall
# scripts (--ignore-scripts), and tee the full output to a log. Failure is
# non-fatal — the AMQ binary alone is enough to operate.
if command -v npx >/dev/null 2>&1; then
  SKILLS_LOG="$STATE_ROOT/amq/skills-install.log"
  : > "$SKILLS_LOG"
  npx --yes --ignore-scripts "skills@${SKILLS_PIN}" add \
    "avivsinai/agent-message-queue#${SKILLS_REV}" -g -y \
    2>&1 | tee -a "$SKILLS_LOG" || \
    warn "npx skills add failed; see $SKILLS_LOG"
fi

# 5. install wrappers --------------------------------------------------------
say "installing wrappers to $LOCAL_BIN"
install -m 0755 "$HERE/wrappers/claude-amq"  "$LOCAL_BIN/claude-amq"
install -m 0755 "$HERE/wrappers/codex-amq"   "$LOCAL_BIN/codex-amq"
install -m 0755 "$HERE/wrappers/gemini-amq"  "$LOCAL_BIN/gemini-amq"
install -m 0755 "$HERE/scripts/finalize-claude-migration.sh" "$STATE_ROOT/scripts/finalize-claude-migration.sh"

# 5b. install shared lib (path-encode.sh, wake-launch.sh) --------------------
# Audit01 Stage 3a: the wrappers expect a sibling lib dir. Default target is
# $LOCAL_BIN/../share/dux-amq/lib so a non-root install resolves it via the
# wrapper's "<bin>/../share/dux-amq/lib" search rule (see header comment).
DUX_AMQ_LIB_DEST="${DUX_AMQ_LIB_DEST:-$(cd "$LOCAL_BIN/.." && pwd)/share/dux-amq/lib}"
say "installing dux-amq lib to $DUX_AMQ_LIB_DEST"
mkdir -p "$DUX_AMQ_LIB_DEST"
install -m 0644 "$HERE/lib/path-encode.sh" "$DUX_AMQ_LIB_DEST/path-encode.sh"
install -m 0644 "$HERE/lib/wake-launch.sh" "$DUX_AMQ_LIB_DEST/wake-launch.sh"
ok "dux-amq lib installed (path-encode.sh, wake-launch.sh)"

# 6. dux config --------------------------------------------------------------
# Audit01 P2-8: never blow away a hand-edited config.toml on re-run. We detect
# "user content" via two markers: a `projects = […]` line (only appears once
# the user has added at least one project) or any `[macros.…]` section
# (the upstream defaults ship zero macros). Either is a strong "this file
# is now mine" signal. On a fresh install (no config.toml yet) we still
# regenerate exactly as before. `FORCE_REGEN=1` is the explicit override
# for operators who want to reset to defaults intentionally; the existing
# file is moved aside as `config.toml.bak.<timestamp>` instead of deleted,
# so a mistaken FORCE_REGEN is recoverable.
DUX_CONFIG="$STATE_ROOT/dux/config.toml"
SKIP_REGEN=
if [[ -f "$DUX_CONFIG" ]] \
   && grep -qE '^projects[[:space:]]*=|^\[macros\.' "$DUX_CONFIG" \
   && [[ "${FORCE_REGEN:-}" != "1" ]]; then
  warn "config.toml has user content; skipping regenerate (FORCE_REGEN=1 to overwrite)"
  SKIP_REGEN=1
fi
if [[ -n "$SKIP_REGEN" ]]; then
  ok "preserved $DUX_CONFIG (use FORCE_REGEN=1 to reset)"
elif [[ -f "$DUX_CONFIG" && "${FORCE_REGEN:-}" == "1" ]]; then
  _bak="${DUX_CONFIG}.bak.$(date -u +%Y%m%dT%H%M%SZ)"
  cp -p "$DUX_CONFIG" "$_bak"
  warn "FORCE_REGEN=1 — backed up existing config.toml to $_bak"
  DUX_HOME="$STATE_ROOT/dux" dux config regenerate --yes >/dev/null
else
  DUX_HOME="$STATE_ROOT/dux" dux config regenerate --yes >/dev/null
fi
say "patching $DUX_CONFIG"
# The sed patch block stays unconditional. Each `s|…|…|` is hash-keyed on
# the upstream-default LHS, so re-running on a file already patched
# (matches replaced) is a no-op — and re-running on a hand-edited file
# only touches lines that still hold the literal upstream defaults.
sed -i \
  -e 's|^prompt_for_name = false$|prompt_for_name = true|' \
  -e 's|^command = "claude"$|command = "claude-amq"|' \
  -e 's|^command = "codex"$|command = "codex-amq"|' \
  -e 's|^command = "gemini"$|command = "gemini-amq"|' \
  -e 's|^resume_args = \["--continue"\]$|resume_args = ["--continue", "--fork-session"]|' \
  "$DUX_CONFIG"

# 7. shell rc ----------------------------------------------------------------
# Audit01 P1-7: delete-then-rewrite (the pyenv/sdkman pattern). On every
# install we strip any prior `# >>> dux-amq vN.M.K >>>` block AND the legacy
# unversioned `# === dux + AMQ ===` block, then append the current version.
# That way version bumps actually propagate instead of being no-ops.
say "rewriting ~/.bashrc dux-amq stanza (v$DUX_AMQ_VERSION)"
touch "$HOME/.bashrc"
strip_block "$HOME/.bashrc" sh
sed "s|REPLACE_AT_INSTALL|$DUX_AMQ_VERSION|g" "$HERE/config/bashrc-additions.sh" >> "$HOME/.bashrc"

# 8. global CLAUDE.md --------------------------------------------------------
mkdir -p "$HOME/.claude"
touch "$HOME/.claude/CLAUDE.md"
say "rewriting ~/.claude/CLAUDE.md dux-amq stanza (v$DUX_AMQ_VERSION)"
strip_block "$HOME/.claude/CLAUDE.md" md
{
  printf '\n<!-- >>> dux-amq v%s >>> -->\n\n' "$DUX_AMQ_VERSION"
  cat "$HERE/config/claude-md-additions.md"
  printf '\n<!-- <<< dux-amq v%s <<< -->\n' "$DUX_AMQ_VERSION"
} >> "$HOME/.claude/CLAUDE.md"

# 9. VSCode Remote-SSH machine settings (best-effort) ------------------------
# Free Ctrl-G in the integrated terminal so dux's `exit_interactive` works.
# Workbench-level settings like commandsToSkipShell are typically resolved
# on the LOCAL machine, so this VM-side write may or may not propagate. We
# do it anyway because it's harmless when ineffective and helpful otherwise.
# The User-settings copy-paste printed below is the authoritative fix.
configure_vscode_remote() {
  local f="$HOME/.vscode-server/data/Machine/settings.json"
  [[ -d "$(dirname "$f")" ]] || return 0
  if ! command -v jq >/dev/null 2>&1; then
    warn "jq not installed; skipping VM-side VSCode settings merge"
    return 0
  fi
  local entries='["-workbench.action.gotoLine","-workbench.action.terminal.goToRecentDirectory"]'
  if [[ ! -f "$f" ]]; then
    # First write — explicit mode 0644 so re-runs that hit the merge
    # branch below have a deterministic mode to preserve. (P2-9.)
    printf '%s\n' "{
  \"terminal.integrated.commandsToSkipShell\": $entries
}" > "$f"
    chmod 0644 "$f"
    ok "wrote $f"
    return 0
  fi
  # Audit01 P2-9: preserve the original file's mode across the merge. The
  # plain `jq … > f.tmp && mv f.tmp f` pattern produces a tmp file with
  # the user's umask (typically 0644 but possibly 0600 on hardened hosts),
  # then mv overwrites the original — losing whatever mode the user set.
  # `install -m <mode>` rewrites with an explicit mode so the file ends up
  # with the *original*'s permissions regardless of umask. We capture the
  # mode before running jq so a partial-failure midway doesn't leave a
  # half-permissioned tmp file behind. Idempotent: re-running install.sh
  # observes the same 0644 (or whatever the operator set) every time.
  local mode
  mode=$(stat -c '%a' "$f")
  if ! jq --argjson new "$entries" '
    .["terminal.integrated.commandsToSkipShell"] = (
      ((.["terminal.integrated.commandsToSkipShell"] // []) + $new) | unique
    )
  ' "$f" > "$f.tmp"; then
    warn "jq merge failed for $f"
    rm -f "$f.tmp"
    return 1
  fi
  if install -m "$mode" "$f.tmp" "$f"; then
    rm -f "$f.tmp"
    ok "merged Ctrl-G passthrough into $f (mode $mode preserved)"
  else
    warn "could not install merged $f (mode preserve failed)"
    rm -f "$f.tmp"
    return 1
  fi
}
configure_vscode_remote

ok "install complete"
echo
echo "Next steps:"
echo "  1. exec bash -l                  # pick up new env"
echo "  2. (optional) $STATE_ROOT/scripts/finalize-claude-migration.sh"
echo "     # ONLY after closing every running 'claude' process"
echo "  3. dux                            # launch"
echo
echo "─── VSCode Remote-SSH (Windows / macOS local) ───"
echo "If Ctrl-G still opens VSCode's 'Go to Recent Directory' picker after"
echo "restarting dux, the workbench setting must live on your LOCAL machine."
echo "Open VSCode → Cmd/Ctrl+Shift+P → 'Preferences: Open User Settings (JSON)'"
echo "and merge into the existing terminal.integrated.commandsToSkipShell"
echo "array (or add the key if absent):"
echo
cat <<'JSON'
    "terminal.integrated.commandsToSkipShell": [
      "-workbench.action.gotoLine",
      "-workbench.action.terminal.goToRecentDirectory"
    ]
JSON
echo
echo "Both entries are needed: the first frees Ctrl-G in editors, the"
echo "second frees Ctrl-G inside the integrated terminal (which is the"
echo "one that bites in dux)."

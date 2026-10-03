#!/usr/bin/env bats
#
# The root install.sh must install THIS fork's release (244b33c6, audit03
# P1-07/P1-08). Upstream's installer points at patrickdappollonio/dux, whose
# builds lack every AMQ/peer/watch feature this repository documents, so a
# regression here silently installs the wrong software.
#
# Offline: curl and tar are stubbed, and every URL the installer asks for is
# logged so the test can assert which repository was contacted.

load 'lib/setup'

setup() {
  setup_isolated_home
  REPO_ROOT="$(cd "$BATS_TEST_DIRNAME/../.." && pwd)"
  FAKE_BIN="$TEST_HOME/bin"
  INSTALL_DIR="$TEST_HOME/install"
  CURL_LOG="$TEST_HOME/curl.log"
  TAR_LOG="$TEST_HOME/tar.log"
  mkdir -p "$FAKE_BIN" "$INSTALL_DIR"
  export CURL_LOG TAR_LOG
  export PATH="$FAKE_BIN:$PATH"

  cat >"$FAKE_BIN/uname" <<'EOF'
#!/usr/bin/env bash
case "${1:-}" in
  -s) printf 'Linux\n' ;;
  -m) printf 'x86_64\n' ;;
  *) exit 2 ;;
esac
EOF
  chmod 0755 "$FAKE_BIN/uname"
}

teardown() {
  teardown_isolated_home
}

# Fake curl serving a release. FAKE_SUMS_MODE picks how the checksum is
# published: "sha256" (a per-archive .sha256, upstream's scheme), "sums" (only
# a combined SHA256SUMS, this fork's historical dux-amq-v0.1.x releases), or "none".
# API calls answer from FAKE_API_LATEST / FAKE_API_LIST ("404" means HTTP 404).
install_fake_curl() {
  export FAKE_CHECKSUM="$1" FAKE_SUMS_MODE="${2:-sha256}"
  cat >"$FAKE_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
dest="" write_code=""
url="${!#}"
for ((i = 1; i <= $#; i++)); do
  case "${!i}" in
    -o) next=$((i + 1)); dest="${!next}" ;;
    -w) write_code=1 ;;
  esac
done
printf '%s\n' "$url" >>"$CURL_LOG"
answer() {  # $1 = http code, $2 = body (written to dest or stdout)
  if [[ -n "$dest" ]]; then printf '%s' "$2" >"$dest"; else printf '%s' "$2"; fi
  if [[ -n "$write_code" ]]; then printf '%s' "$1"; exit 0; fi
  [[ "$1" == 2* ]] || exit 22
  exit 0
}
case "$url" in
  */releases/latest)
    [[ "${FAKE_API_LATEST:-404}" == 404 ]] && answer 404 ""
    answer 200 "{\"tag_name\": \"${FAKE_API_LATEST}\"}" ;;
  */releases\?per_page=1)
    [[ "${FAKE_API_LIST:-404}" == 404 ]] && answer 404 ""
    answer 200 "[{\"tag_name\": \"${FAKE_API_LIST}\"}]" ;;
  *.tar.gz.sha256)
    [[ "$FAKE_SUMS_MODE" == sha256 ]] || answer 404 ""
    archive="${url##*/}"
    answer 200 "$FAKE_CHECKSUM  ${archive%.sha256}
" ;;
  */SHA256SUMS)
    [[ "$FAKE_SUMS_MODE" == sums ]] || answer 404 ""
    answer 200 "1111111111111111111111111111111111111111111111111111111111111111  yaran-darwin-arm64.tar.gz
$FAKE_CHECKSUM  yaran-linux-amd64.tar.gz
$FAKE_CHECKSUM  dux-linux-amd64.tar.gz
" ;;
  *.tar.gz) answer 200 "release archive bytes
" ;;
  *) answer 404 "" ;;
esac
EOF
  chmod 0755 "$FAKE_BIN/curl"
}

install_fake_tar() {
  cat >"$FAKE_BIN/tar" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'called\n' >>"$TAR_LOG"
binary="yaran"
for arg in "$@"; do
  case "$arg" in */dux-*.tar.gz) binary="dux" ;; esac
done
dest=""
while (($#)); do
  if [[ "$1" == "-C" ]]; then
    dest="$2"
    shift 2
  else
    shift
  fi
done
printf '#!/usr/bin/env bash\nprintf "fixture\\n"\n' >"$dest/$binary"
chmod 0755 "$dest/$binary"
EOF
  chmod 0755 "$FAKE_BIN/tar"
}

archive_checksum() {
  local archive="$TEST_HOME/archive"
  printf 'release archive bytes\n' >"$archive"
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$archive" | awk '{print $1}'
  else
    shasum -a 256 "$archive" | awk '{print $1}'
  fi
}

@test "root installer downloads from the releasing repository" {
  install_fake_curl "$(archive_checksum)"
  install_fake_tar

  run env YARAN_VERSION=yaran-v1.2.3 YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  [ -x "$INSTALL_DIR/yaran" ]
  grep -Fq -- "github.com/SiavZ/yaran/releases/download/yaran-v1.2.3/yaran-linux-amd64.tar.gz" "$CURL_LOG"
  grep -Fq -- "github.com/SiavZ/yaran/releases/download/yaran-v1.2.3/yaran-linux-amd64.tar.gz.sha256" "$CURL_LOG"
  ! grep -Fq -- "patrickdappollonio" "$CURL_LOG" || false
  [[ "$output" == *"Checksum verified"* ]]
}

@test "root installer rejects a checksum mismatch before extraction" {
  install_fake_curl "0000000000000000000000000000000000000000000000000000000000000000"
  install_fake_tar

  run env YARAN_VERSION=v1.2.3 YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -ne 0 ]
  [[ "$output" == *"Checksum mismatch"* ]]
  [ ! -e "$TAR_LOG" ]
  [ ! -e "$INSTALL_DIR/yaran" ]
}

@test "root installer verifies a release that publishes only SHA256SUMS" {
  install_fake_curl "$(archive_checksum)" sums
  install_fake_tar

  run env YARAN_VERSION=dux-amq-v0.1.1 YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  [ -x "$INSTALL_DIR/yaran" ]
  grep -Fxq -- "https://github.com/SiavZ/yaran/releases/download/dux-amq-v0.1.1/dux-linux-amd64.tar.gz" "$CURL_LOG"
  grep -Fq -- "github.com/SiavZ/yaran/releases/download/dux-amq-v0.1.1/SHA256SUMS" "$CURL_LOG"
  [[ "$output" == *"Checksum verified"* ]]
}

@test "root installer rejects a SHA256SUMS mismatch before extraction" {
  install_fake_curl "0000000000000000000000000000000000000000000000000000000000000000" sums
  install_fake_tar

  run env YARAN_VERSION=dux-amq-v0.1.1 YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -ne 0 ]
  [[ "$output" == *"Checksum mismatch"* ]]
  [ ! -e "$TAR_LOG" ]
}

# 244b33c6: /releases/latest answers 404 when every release is a prerelease,
# and the fork's first release was one. The installer must fall back to the
# newest release in the list instead of dying.
@test "root installer falls back to the newest release when there is no stable one" {
  install_fake_curl "$(archive_checksum)"
  install_fake_tar

  run env FAKE_API_LATEST=404 FAKE_API_LIST=yaran-v0.2.0-rc1 \
    YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  grep -Fq -- "api.github.com/repos/SiavZ/yaran/releases?per_page=1" "$CURL_LOG"
  grep -Fq -- "releases/download/yaran-v0.2.0-rc1/yaran-linux-amd64.tar.gz" "$CURL_LOG"
}

# a0f84a7f: resolve_version runs inside $(...). A progress line on stdout was
# captured into the version and spliced into the download URL. Every URL the
# installer fetched must therefore be well formed.
@test "root installer keeps progress output out of the resolved version" {
  install_fake_curl "$(archive_checksum)"
  install_fake_tar

  run env FAKE_API_LATEST=yaran-v0.3.0 YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  grep -Fxq -- "https://github.com/SiavZ/yaran/releases/download/yaran-v0.3.0/yaran-linux-amd64.tar.gz" "$CURL_LOG"
  ! grep -q -- ' ' "$CURL_LOG" || false
}

@test "root installer names the repository when no release can be resolved" {
  install_fake_curl "$(archive_checksum)"
  install_fake_tar

  run env FAKE_API_LATEST=404 FAKE_API_LIST=404 YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -ne 0 ]
  [[ "$output" == *"Could not determine the latest release for SiavZ/yaran"* ]]
  [[ "$output" == *"YARAN_VERSION=dux-amq-v0.1.1"* ]]
  [ ! -e "$INSTALL_DIR/yaran" ]
}

@test "legacy installer inputs preserve the original repository and historical release tag" {
  install_fake_curl "$(archive_checksum)"
  install_fake_tar
  run env -u YARAN_VERSION -u YARAN_INSTALL_DIR -u YARAN_REPO \
    DUX_VERSION=dux-amq-v0.1.1 DUX_INSTALL_DIR="$INSTALL_DIR" DUX_REPO=SiavZ/dux-amq-setup \
    "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  [ -x "$INSTALL_DIR/yaran" ]
  grep -Fxq -- "https://github.com/SiavZ/dux-amq-setup/releases/download/dux-amq-v0.1.1/dux-linux-amd64.tar.gz" "$CURL_LOG"
}

@test "canonical installer inputs win over legacy values" {
  install_fake_curl "$(archive_checksum)"
  install_fake_tar
  run env YARAN_VERSION=yaran-v1.2.3 DUX_VERSION=dux-amq-v0.1.1 \
    YARAN_REPO=SiavZ/yaran DUX_REPO=wrong/repo \
    YARAN_INSTALL_DIR="$INSTALL_DIR" DUX_INSTALL_DIR="$TEST_HOME/unused" "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  [ -x "$INSTALL_DIR/yaran" ]
  grep -Fxq -- "https://github.com/SiavZ/yaran/releases/download/yaran-v1.2.3/yaran-linux-amd64.tar.gz" "$CURL_LOG"
  [ ! -e "$TEST_HOME/unused" ]
}

@test "empty canonical version disables the legacy version override" {
  install_fake_curl "$(archive_checksum)"
  install_fake_tar
  run env YARAN_VERSION= DUX_VERSION=dux-amq-v0.1.1 FAKE_API_LATEST=yaran-v2.0.0 \
    YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  grep -Fxq -- "https://github.com/SiavZ/yaran/releases/download/yaran-v2.0.0/yaran-linux-amd64.tar.gz" "$CURL_LOG"
}

@test "bare version selects a canonical Yaran release" {
  install_fake_curl "$(archive_checksum)"
  install_fake_tar
  run env YARAN_VERSION=1.2.3 YARAN_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  grep -Fxq -- "https://github.com/SiavZ/yaran/releases/download/yaran-v1.2.3/yaran-linux-amd64.tar.gz" "$CURL_LOG"
}

@test "legacy library guard only defines functions without making requests" {
  run env -u YARAN_INSTALL_SH_LIB DUX_INSTALL_SH_LIB=1 bash -c 'source "$1"; declare -F resolve_version' _ "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  [[ "$output" == *"resolve_version"* ]]
  [ ! -e "$CURL_LOG" ]
}

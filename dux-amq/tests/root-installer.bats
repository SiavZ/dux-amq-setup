#!/usr/bin/env bats

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

install_fake_curl() {
  export FAKE_CHECKSUM="$1"
  cat >"$FAKE_BIN/curl" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
dest=""
url="${!#}"
for ((i = 1; i <= $#; i++)); do
  if [[ "${!i}" == "-o" ]]; then
    next=$((i + 1))
    dest="${!next}"
  fi
done
printf '%s\n' "$url" >>"$CURL_LOG"
if [[ "$url" == */SHA256SUMS ]]; then
  printf '%s  dux-linux-amd64.tar.gz\n' "$FAKE_CHECKSUM" >"$dest"
else
  printf 'release archive bytes\n' >"$dest"
fi
EOF
  chmod 0755 "$FAKE_BIN/curl"
}

install_fake_tar() {
  cat >"$FAKE_BIN/tar" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
printf 'called\n' >>"$TAR_LOG"
dest=""
while (($#)); do
  if [[ "$1" == "-C" ]]; then
    dest="$2"
    shift 2
  else
    shift
  fi
done
printf '#!/usr/bin/env bash\nprintf "dux fixture\\n"\n' >"$dest/dux"
chmod 0755 "$dest/dux"
EOF
  chmod 0755 "$FAKE_BIN/tar"
}

@test "root installer downloads from the releasing repository" {
  local archive="$TEST_HOME/archive"
  printf 'release archive bytes\n' >"$archive"
  local checksum
  if command -v sha256sum >/dev/null 2>&1; then
    checksum=$(sha256sum "$archive" | awk '{print $1}')
  else
    checksum=$(shasum -a 256 "$archive" | awk '{print $1}')
  fi
  install_fake_curl "$checksum"
  install_fake_tar

  run env DUX_VERSION=v1.2.3 DUX_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -eq 0 ]
  [ -x "$INSTALL_DIR/dux" ]
  grep -Fq -- "github.com/SiavZ/dux-amq-setup/releases/download/v1.2.3/dux-linux-amd64.tar.gz" "$CURL_LOG"
  grep -Fq -- "github.com/SiavZ/dux-amq-setup/releases/download/v1.2.3/SHA256SUMS" "$CURL_LOG"
}

@test "root installer rejects a checksum mismatch before extraction" {
  install_fake_curl "0000000000000000000000000000000000000000000000000000000000000000"
  install_fake_tar

  run env DUX_VERSION=v1.2.3 DUX_INSTALL_DIR="$INSTALL_DIR" "$REPO_ROOT/install.sh"
  [ "$status" -ne 0 ]
  [[ "$output" == *"Checksum mismatch"* ]]
  [ ! -e "$TAR_LOG" ]
  [ ! -e "$INSTALL_DIR/dux" ]
}

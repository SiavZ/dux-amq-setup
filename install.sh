#!/usr/bin/env bash
set -euo pipefail

REPO="${DUX_REPO:-SiavZ/dux-amq-setup}"
BINARY="dux"

# Allow overriding the version and install directory via environment variables.
VERSION="${DUX_VERSION:-}"
INSTALL_DIR="${DUX_INSTALL_DIR:-}"

log() { printf '%s\n' "$@"; }
err() { log "$@" >&2; exit 1; }

# Progress output from a function whose stdout is captured by command
# substitution MUST go to stderr, or it lands in the captured value.
# `resolve_version` is called as `version="$(resolve_version)"`.
note() { log "$@" >&2; }

detect_os() {
    local os
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    case "$os" in
        linux)  echo "linux" ;;
        darwin) echo "darwin" ;;
        *)      err "Unsupported operating system: $os" ;;
    esac
}

detect_arch() {
    local arch
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64)       echo "amd64" ;;
        aarch64|arm64)      echo "arm64" ;;
        *)                  err "Unsupported architecture: $arch" ;;
    esac
}

has_cmd() { command -v "$1" >/dev/null 2>&1; }

http_get() {
    local url="$1"
    if has_cmd curl; then
        curl -sSfL "$url"
    elif has_cmd wget; then
        wget -qO- "$url"
    else
        err "Either curl or wget is required to download files."
    fi
}

http_download() {
    local url="$1" dest="$2"
    if has_cmd curl; then
        curl -sSfL -o "$dest" "$url"
    elif has_cmd wget; then
        wget -qO "$dest" "$url"
    else
        err "Either curl or wget is required to download files."
    fi
}

sha256_file() {
    if has_cmd sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif has_cmd shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        err "Either sha256sum or shasum is required to verify the release archive."
    fi
}

verify_archive() {
    local archive_path="$1" sums_path="$2" archive_name="$3"
    local expected actual
    expected="$(awk -v name="$archive_name" '$2 == name || $2 == "*" name { print $1; exit }' "$sums_path")"
    [ -n "$expected" ] || err "SHA256SUMS has no entry for ${archive_name}."
    actual="$(sha256_file "$archive_path")"
    [ "$actual" = "$expected" ] || \
        err "Checksum mismatch for ${archive_name}: got ${actual}, expected ${expected}."
    log "Verified ${archive_name} (${actual})"
}

# Parse the first tag_name out of a GitHub API response without requiring jq.
# Works for both the single-object shape (/releases/latest) and the array
# shape (/releases), where the newest release is first.
parse_tag() {
    printf '%s' "${1:-}" \
        | grep -o '"tag_name"[[:space:]]*:[[:space:]]*"[^"]*"' \
        | head -1 \
        | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/'
}

resolve_version() {
    if [ -n "$VERSION" ]; then
        # Fork releases are tagged `dux-amq-vX.Y.Z`; a bare `X.Y.Z` is also
        # accepted and gets the `v` prefix for backward compatibility.
        case "$VERSION" in
            dux-amq-*|v*) echo "$VERSION" ;;
            *)            echo "v$VERSION" ;;
        esac
        return
    fi

    note "Fetching latest release version..."
    local response tag

    # `/releases/latest` excludes prereleases and returns 404 when a repository
    # has only prereleases — which is exactly the state this repo was in before
    # its first stable release. Fall back to the full list (newest first) so a
    # prerelease-only repo still installs rather than dying with a bare error.
    if response="$(http_get "https://api.github.com/repos/${REPO}/releases/latest" 2>/dev/null)"; then
        tag="$(parse_tag "$response")"
    fi

    if [ -z "${tag:-}" ]; then
        note "No stable release found; falling back to the most recent release..."
        response="$(http_get "https://api.github.com/repos/${REPO}/releases?per_page=1" 2>/dev/null)" || true
        tag="$(parse_tag "${response:-}")"
    fi

    if [ -z "${tag:-}" ]; then
        err "Could not determine the latest release for ${REPO}." \
            "" \
            "The repository may have no releases yet, or the GitHub API may be" \
            "unreachable from this host (rate limit, proxy, or no network)." \
            "" \
            "Install a specific version instead:" \
            "  curl -sSfL https://raw.githubusercontent.com/${REPO}/main/install.sh \\" \
            "    | DUX_VERSION=dux-amq-v0.1.0 bash" \
            "" \
            "Available releases: https://github.com/${REPO}/releases"
    fi
    echo "$tag"
}

resolve_install_dir() {
    # 1. Explicit override.
    if [ -n "$INSTALL_DIR" ]; then
        echo "$INSTALL_DIR"
        return
    fi

    # 2. ~/.local/bin if it exists and is in PATH.
    local local_bin="$HOME/.local/bin"
    if [ -d "$local_bin" ]; then
        case ":$PATH:" in
            *":$local_bin:"*) echo "$local_bin"; return ;;
        esac
    fi

    # 3. Traditional fallback.
    echo "/usr/local/bin"
}

main() {
    local os arch version install_dir archive url sums_url tmpdir cleanup_cmd

    os="$(detect_os)"
    arch="$(detect_arch)"
    version="$(resolve_version)"
    install_dir="$(resolve_install_dir)"
    archive="${BINARY}-${os}-${arch}.tar.gz"
    url="https://github.com/${REPO}/releases/download/${version}/${archive}"
    sums_url="https://github.com/${REPO}/releases/download/${version}/SHA256SUMS"

    log "Installing ${BINARY} ${version} (${os}/${arch}) to ${install_dir}"

    tmpdir="$(mktemp -d)"
    printf -v cleanup_cmd 'rm -rf -- %q' "$tmpdir"
    # Expand now: `tmpdir` is local to main and is out of scope at EXIT.
    # shellcheck disable=SC2064
    trap "$cleanup_cmd" EXIT

    log "Downloading ${url}..."
    http_download "$url" "${tmpdir}/${archive}"
    http_download "$sums_url" "${tmpdir}/SHA256SUMS"
    verify_archive "${tmpdir}/${archive}" "${tmpdir}/SHA256SUMS" "$archive"

    tar xzf "${tmpdir}/${archive}" -C "$tmpdir"

    # Install the binary — use sudo only if the target directory is not writable.
    if [ -w "$install_dir" ]; then
        install -m 755 "${tmpdir}/${BINARY}" "${install_dir}/${BINARY}"
    else
        log "Installation directory ${install_dir} is not writable, using sudo..."
        sudo install -m 755 "${tmpdir}/${BINARY}" "${install_dir}/${BINARY}"
    fi

    log ""
    log "${BINARY} ${version} has been installed to ${install_dir}/${BINARY}"

    if ! has_cmd "$BINARY"; then
        log ""
        log "Warning: ${install_dir} is not in your PATH."
        log "Add it to your shell profile:"
        log "  export PATH=\"${install_dir}:\$PATH\""
    fi
}

main

#!/usr/bin/env bash
# amq-secret-init.sh — generate the per-VM AMQ HMAC secret.
#
# Audit02 P0-K (T2): every dux-amq pane must sign outgoing AMQ messages
# with HMAC-SHA256 so a compromised peer cannot spoof `--me <victim>`
# into another pane via the `amq wake … --inject-mode raw` injection
# path. This script writes a 32-byte (256-bit) random key, base64-encoded
# for portability, to `$HOME/.local/share/dux-amq/amq-secret`. Mode is
# 0600 so only the dux user (and root) can read it.
#
# Idempotent: if the file already exists it is left untouched. To rotate
# the secret, delete the file before re-running. Because the secret is
# per-VM, rotation invalidates all in-flight signed envelopes — all
# panes must be restarted to pick up the new key.
#
# Threat-model caveat: this protects against an attacker who has
# *filesystem-write* access to `$AMQ_GLOBAL_ROOT` but NOT shell access
# as the dux user. An attacker with arbitrary shell as the dux user can
# read `$HOME/.local/share/dux-amq/amq-secret` and forge signatures.
# That escalation is documented in docs/audits/audit02.md (T2) and
# acknowledged as out of scope for this phase.
set -euo pipefail

SECRET_PATH="${AMQ_SECRET_PATH:-$HOME/.local/share/dux-amq/amq-secret}"
mkdir -p "$(dirname "$SECRET_PATH")"

if [[ -f "$SECRET_PATH" ]]; then
  # Already initialized; verify mode 0600 and exit. We don't rotate
  # silently — that would invalidate every signed message in flight.
  printf 'amq-secret-init: %s already present (kept)\n' "$SECRET_PATH" >&2
  chmod 0600 "$SECRET_PATH" 2>/dev/null || true
  exit 0
fi

# 256 bits of entropy from /dev/urandom; base64 (no newline) for portability
# across environments where the secret is exported via env (no embedded NUL
# or whitespace). `head -c 32` is portable to busybox and macOS coreutils.
umask 077
head -c 32 /dev/urandom | base64 | tr -d '\n' >"$SECRET_PATH"
chmod 0600 "$SECRET_PATH"
printf 'amq-secret-init: wrote %s (mode 0600)\n' "$SECRET_PATH" >&2

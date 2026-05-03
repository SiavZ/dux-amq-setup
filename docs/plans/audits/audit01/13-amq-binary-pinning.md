# Phase 13: AMQ binary pinning + guarded `eval "$(amq shell-setup)"`

> Maps to audit findings: P1-8

## Goal
Reduce the trust impact of `eval "$(amq shell-setup)"` (run on every
interactive shell start, `bashrc-additions.sh:7-9`) by (a) pinning the
binary at an install-controlled absolute path, (b) recording its sha256 at
install time, (c) refusing to source `shell-setup` if the binary on disk
no longer matches the recorded hash.

## Pre-conditions
- Phase 01 (supply-chain) shipped — pinned AMQ install commit.
- Phase 12 (versioned inserts) shipped — bashrc rewritten cleanly on upgrade.

## Files to touch
- `dux-amq/install.sh` — record binary hash; copy to controlled path.
- `dux-amq/config/bashrc-additions.sh` — bare `eval` → guarded helper.
- `dux-amq/lib/amq-shell-setup-guard.sh` — create; the guard.
- `dux-amq/tests/amq_shell_setup_guard.bats` — create.

## Steps
1. Controlled binary path:
   ```bash
   AMQ_BIN_DIR="$STATE_ROOT/amq-bin"; mkdir -p "$AMQ_BIN_DIR"
   cp "$(command -v amq)" "$AMQ_BIN_DIR/amq"; chmod 0755 "$AMQ_BIN_DIR/amq"
   sha256sum "$AMQ_BIN_DIR/amq" > "$STATE_ROOT/amq/binary.sha256"
   ```
   `0755` + owner-only writable (root compromise out of scope).
2. Guarded eval in `bashrc-additions.sh`:
   ```diff
   - if command -v amq >/dev/null 2>&1; then
   -   eval "$(amq shell-setup)"
   - fi
   + [[ -r "$DUX_AMQ_LIB/amq-shell-setup-guard.sh" ]] && source "$DUX_AMQ_LIB/amq-shell-setup-guard.sh"
   ```
3. Guard:
   ```bash
   # dux-amq/lib/amq-shell-setup-guard.sh
   _amq_shell_setup_guarded() {
     local bin="${AMQ_BIN:-/data/state/amq-bin/amq}"
     local rec="${STATE_ROOT:-/data/state}/amq/binary.sha256"
     [[ -x "$bin" && -f "$rec" ]] || return 0
     local exp act
     exp=$(awk '{print $1}' "$rec")
     act=$(sha256sum "$bin" | awk '{print $1}')
     if [[ "$exp" != "$act" ]]; then
       printf '\033[1;31m!\033[0m amq binary hash mismatch (%s vs %s); shell-setup skipped\n' "$act" "$exp" >&2
       return 1
     fi
     eval "$("$bin" shell-setup)"
   }
   _amq_shell_setup_guarded
   ```
4. Export `AMQ_BIN=/data/state/amq-bin/amq` from `bashrc-additions.sh` so
   wrappers + guard pick up the same path.
5. bats: happy (fake `amq shell-setup` outputs an alias; alias set);
   mismatch (corrupt binary; alias not set; stderr "hash mismatch"); missing
   (no binary; silent return 0).

## Validation
- `bats dux-amq/tests/amq_shell_setup_guard.bats` covers all three cases.
- Manual: `echo touched >> /data/state/amq-bin/amq` → new shell shows red
  banner, no AMQ aliases. Restore → aliases come back.

## Acceptance criteria
- [ ] AMQ binary at `$STATE_ROOT/amq-bin/amq` with `0755` perms.
- [ ] Hash recorded in `$STATE_ROOT/amq/binary.sha256`.
- [ ] No bare `eval "$(amq shell-setup)"` anywhere.
- [ ] Guard refuses to eval on hash mismatch (red banner).
- [ ] bats covers happy, mismatch, missing.

## References
- Audit P1-8.
- Phase 15 explores upgrading from sha256 pinning to cosign signature verification.

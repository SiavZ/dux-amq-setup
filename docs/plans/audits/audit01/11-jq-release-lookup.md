# Phase 11: jq release lookup — drop `grep -oP`

> Maps to audit findings: P1-6

## Goal
Replace the `grep -oP '"tag_name":"\K[^"]+'` PCRE scrape in
`dux-amq/install.sh:23` with `jq -r .tag_name`. PCRE is not portable to BSD
grep, busybox, or Alpine. `jq` is already a soft dependency at
`install.sh:86` (VSCode settings merge); promote to a hard dependency.
After Phase 01 the dux release is fetched by pinned tag, so any remaining
release lookup is informational only — but should still use `jq`.

## Pre-conditions
- Phase 01 has either eliminated or pinned the release-lookup branch.

## Files to touch
- `dux-amq/install.sh` — preflight tool check + `jq` for any remaining lookup.
- `dux-amq/README.md` — Prerequisites listing required tools.

## Steps
1. Add early preflight that fails clearly on missing tools:
   ```diff
   [[ -d /data ]] || { warn "/data not mounted"; exit 1; }
   + for tool in curl jq sha256sum tar install git rsync; do
   +   command -v "$tool" >/dev/null || { warn "missing required tool: $tool"; exit 1; }
   + done
   ```
2. If any release-lookup branch survives Phase 01:
   ```diff
   - TAG=$(curl ... | grep -oP '"tag_name":\s*"\K[^"]+')
   + TAG=$(curl -fsSL https://api.github.com/repos/patrickdappollonio/dux/releases/latest | jq -r .tag_name)
   + [[ -n "$TAG" && "$TAG" != "null" ]] || { warn "could not parse latest dux tag"; exit 1; }
   ```
3. README "Prerequisites": list every required tool with the apt
   one-liner for Debian/Ubuntu:
   ```bash
   sudo apt-get install -y curl jq tar coreutils git rsync
   ```

## Validation
- `bash dux-amq/install.sh` on a VM without `jq` exits 1 with a clear
  message ("missing required tool: jq").
- After installing `jq`, the script proceeds.
- `grep -nR 'grep -oP' dux-amq/` empty.
- `shellcheck dux-amq/install.sh` clean.

## Acceptance criteria
- [ ] No `grep -oP` invocations anywhere in the overlay.
- [ ] Preflight loop checks `curl jq sha256sum tar install git rsync`.
- [ ] README Prerequisites lists all required tools with apt one-liner.
- [ ] `shellcheck dux-amq/install.sh` clean.

## References
- Audit P1-6.
- `jq`: https://jqlang.github.io/jq/
- GNU grep `-P` portability: https://www.gnu.org/software/grep/manual/grep.html#Other-Options

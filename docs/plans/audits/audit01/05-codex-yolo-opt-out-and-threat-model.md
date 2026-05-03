# Phase 05: Codex opt-out + threat-model documentation

> Maps to audit findings: P0-1 (revised scope per user mandate)

## Goal
The Claude `--dangerously-skip-permissions` default is **intentional** and
stays on (user mandate). Remaining work: (1) symmetric `CODEX_AMQ_SAFE=1`
opt-out for `codex-amq:27` (today unconditional `--dangerously-bypass-…`);
(2) explicit threat-model + deployment-guidance section in `dux-amq/README.md`;
(3) recommend ephemeral VM and encrypted persistent disk.

## Pre-conditions
- Phase 00 scaffolding.

## Files to touch
- `dux-amq/wrappers/codex-amq` — add opt-out.
- `dux-amq/config/bashrc-additions.sh` — mention both opt-out vars.
- `dux-amq/README.md` — add "Security model" section.
- `dux-amq/tests/codex_safe.bats` — verify opt-out.

## Steps
1. Add the Codex opt-out:
   ```diff
   - exec amq coop exec ... codex -- --dangerously-bypass-approvals-and-sandbox "$@"
   + CODEX_EXTRA=()
   + [[ "${CODEX_AMQ_SAFE:-}" != "1" ]] && CODEX_EXTRA+=(--dangerously-bypass-approvals-and-sandbox)
   + exec amq coop exec ... codex -- "${CODEX_EXTRA[@]}" "$@"
   ```
   Mention `CLAUDE_AMQ_SAFE=1` and `CODEX_AMQ_SAFE=1` side-by-side as
   commented examples in `bashrc-additions.sh`.
2. README "Security model" must cover, in plain language:
   - **Defaults**: Claude and Codex panes run with full filesystem + shell
     capability and no per-tool prompt. Intentional for interactive
     multi-agent dev work; also the largest concentrated risk on the VM.
     Per-pane opt-out: `CLAUDE_AMQ_SAFE=1`, `CODEX_AMQ_SAFE=1`.
   - **Threat model**: any prompt-injection (malicious README, poisoned
     `gh issue` body, MCP-fetched doc, WebFetch'd page) reaching either
     pane can run arbitrary commands as the VM user — wipe `$HOME`,
     exfil `/data/state/amq` (every pane's transcripts), pivot to other
     panes' worktrees.
   - **Deployment**: ephemeral VM (preemptible spot OK; long-lived
     persistent worker not OK). Encrypt `/data` with LUKS:
     ```bash
     cryptsetup luksFormat /dev/disk/by-id/<dev>
     cryptsetup luksOpen   /dev/disk/by-id/<dev> data
     mkfs.ext4 /dev/mapper/data && mount /dev/mapper/data /data
     ```
   - **Future work**: Claude Code `auto mode` (Anthropic, March 2026)
     replaces `--dangerously-skip-permissions` with classifier-gated
     approval. Re-point the wrapper when the integration is settled.
   - **Revocation**: `rm -rf ~/.local/bin/{amq,dux,*-amq} /data/state/{amq,agents,claude,codex,gemini}` plus disk-encryption rotation.
3. bats test: source `codex-amq` (use the same `(return 0 …) && return`
   guard pattern from Phase 02) and inspect the would-be argv array. Two
   cases: env unset → contains `--dangerously-bypass-approvals-and-sandbox`;
   env=1 → does not.

## Validation
- `bats dux-amq/tests/codex_safe.bats` green.
- Manual: `CODEX_AMQ_SAFE=1 codex-amq` attaches without the bypass flag;
  Codex prompts for tool approval normally.
- README renders; LUKS recipe is copy-pasteable.

## Acceptance criteria
- [ ] `CODEX_AMQ_SAFE=1` reliably suppresses the bypass flag.
- [ ] `bashrc-additions.sh` mentions both opt-outs.
- [ ] README "Security model" covers defaults, threat model, deployment, future work, revocation.
- [ ] No code change flips Claude's default permission behavior.
- [ ] LUKS one-liner present and copy-pasteable.

## References
- Audit P0-1 — original "flip default" recommendation **dropped** per user mandate; symmetric Codex opt-out + docs retained.
- Anthropic auto mode: https://www.anthropic.com/engineering/claude-code-auto-mode
- LUKS: https://gitlab.com/cryptsetup/cryptsetup/-/wikis/home

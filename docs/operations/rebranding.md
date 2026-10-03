# Yaran rebrand and upgrade compatibility

Yaran is the product name. `yaran` is the executable, and `yaran-core`, `yaran-tui`, and `yaran-web` are the workspace crates. The optional shell overlay lives in `yaran-amq/`. Provider wrapper commands such as `claude-amq`, `codex-amq`, `gemini-amq`, and `jcode-amq` keep their names.

The project builds on Patrick D'Appollonio's dux. Its MIT license, copyright, upstream attribution, and captured historical evidence remain intact. Yaran does not claim ownership of upstream's domain, newsletter, npm packages, or Homebrew tap.

## Existing configuration and sessions

No automatic directory move or data deletion is part of this rebrand.

- `YARAN_HOME` is the primary configuration override. `DUX_HOME` remains a fallback only when `YARAN_HOME` is absent. A present but empty or relative new override is an error, not permission to fall back to a different workspace.
- Fresh defaults are `~/.yaran` on macOS and `$XDG_CONFIG_HOME/yaran` on Linux, or `~/.config/yaran` when the XDG override is absent or relative.
- Without a home override, an initialized Yaran home wins. Otherwise an existing dux home is reused in place. A configuration file, session database, store identity, or populated worktree directory identifies initialized state. Empty new directories do not hide legacy state.
- If both homes have state, select the intended one explicitly. Discovery does not merge databases or worktrees.
- The configuration filename, SQLite database, session IDs, provider histories, and stored paths retain their existing formats. No provider's history directory is renamed.
- The internal lock remains `dux.lock`. Both executables must lock the same file when pointed at one workspace, including during a mixed-version rollout. This compatibility filename is not a second product name.

Other `YARAN_*` runtime inputs follow the same new-first, legacy-fallback policy. Child agents receive both new and legacy identity/settings aliases so previously installed wrappers still work. A new-name user setting wins if both names are configured.

## Durable artifacts and browser preferences

Some internal names deliberately remain stable rather than being rewritten:

- Existing AMQ inboxes use the established `dux-amq` data namespace. Resolved queue paths are exported as `YARAN_AMQ_QUEUE_DIR` and `DUX_AMQ_QUEUE_DIR`, so Rust and shell helpers share one queue.
- AMQ ownership markers retain `.dux-amq-source`. Wrappers also recognize the newer spelling without moving or unlinking an existing marker.
- The managed project worktree link and git exclude entries retain their established names. Renaming a live symlink would invalidate saved paths and upstream tooling.
- Existing `.dux/uploads` or `.dux-uploads` directories are reused when the new default would otherwise create a separate upload directory. Explicitly configured paths are unchanged.
- Existing `dux.log` is reused when there is no `yaran.log`. New homes use the new log name.
- Built-in `dux_dark` and `dux-dark` theme aliases still resolve. A user-owned theme file wins before a built-in alias, and loading a legacy config does not rewrite its theme name.

Browser preference reads consult the new Yaran key first, then its legacy dux key. Promotion copies values without deleting the legacy preference. An explicit reset removes both names so old values cannot return after reset. This applies on the same browser origin. Moving to another hostname or port does not transfer local storage automatically.

## Build flags and releases

`YARAN_DISABLE_UI_BUILD` is the current frontend escape hatch. Legacy `DUX_DISABLE_UI_BUILD` is consulted only when it is absent. Setting the new flag to an empty value enables the normal frontend build even if the old skip flag is set. Both old and new placeholder-page markers are recognized, and CI guards reject either brand's externally supplied skip/state markers.

New release tags are **`yaran-vX.Y.Z`**. New archives are `yaran-<os>-<arch>.tar.gz`, containing the `yaran` executable. The display version remains `vX.Y.Z`; current release-note lookup maps it to the Yaran tag. Explicit historical tag lookups remain exact.

Published `dux-amq-v*` tags, original archive/member names, and checksums are immutable provenance. Pinning one of those tags must still request its original archive bytes, not fabricate a Yaran archive at the old URL. The overlay's pinned legacy release is an explicit compatibility bootstrap, not a newly published Yaran release.

## External steps not performed by this change

1. Rename the hosted GitHub repository only after choosing its final address. It currently remains `SiavZ/dux-amq-setup`, and source, issue, and release URLs use that real address.
2. Publish and verify a first `yaran-vX.Y.Z` release containing the new executable and real web UI before advertising a new-brand binary install. Until then, build with `cargo install --path crates/yaran --locked`.
3. Choose and deploy a website origin. Set `YARAN_SITE_URL` for that deployment. The repository's website sources do not establish ownership of a public domain.
4. Create a Yaran npm package or Homebrew formula only if those distribution channels are wanted. The local npm wrapper is private, and upstream installers still install upstream dux.
5. Rename this checkout directory only after all running agents and shells stop using its current absolute path. This rebrand leaves the live working directory untouched.

Captured audits, port-validation reports, measurement logs, original release-note fixtures, and historical upstream blog content retain their original terminology and source URLs. They describe what was actually measured or published, not current Yaran branding.

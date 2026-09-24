# Installation, purge, and operations

This subsystem covers the root release installer, the Dux+AMQ overlay installer/migration scripts, doctor, and the hard-purge cascade. Baseline is `3d52074`.

## P0-01 — Installer reruns overwrite the user's Dux configuration

**Impact.** Rerunning the installer—advertised as safe and idempotent—replaces all user edits in `config.toml` with freshly generated defaults before applying a fixed `sed` patch.

**Baseline evidence.** `3d52074 — dux-amq/install.sh:1-4 — installer contract` says “Idempotent: re-run at will.” `3d52074 — dux-amq/install.sh:407-419 — Dux config step` unconditionally executes `DUX_HOME=... dux config regenerate --yes`. `3d52074 — src/cli.rs:531-553 — run_regenerate` renders defaults and directly overwrites `paths.config_path` when `--yes` is present. The following `sed` changes only known provider keys; it cannot restore projects, macros, keybindings, watch rules, AMQ/storage settings, comments, or unknown extensions.

**Adversarial verification.** The normal in-app `save_config` path preserves comments/unknown keys, but the installer does not call it. There is no first-install guard or backup around regenerate. Overlay idempotency tests assert repeat completion and managed stanza counts, not preservation of a seeded arbitrary Dux config. Audit01 P2-8 identified the overwrite; the fresh end-to-end baseline trace meets audit03's explicit data-loss threshold and is P0.

**Required proof direction.** On an existing config, patch only owned typed settings through a preservation-aware command or surgical TOML edit. Seed a config containing comments, unknown keys, macros, projects, and custom provider args; byte-/semantic-proof that rerun preserves all unowned content.

## P0-03 — Purge deletes the only session identity after an earlier cleanup failure

**Impact.** If any worktree/provider/AMQ/log removal fails, purge still deletes the SQLite session row. The promised rerun recovery is then impossible and residual personal data becomes unaddressable by session ID/branch.

**Baseline evidence.** `3d52074 — src/purge.rs:33-40 — documented ordering` says SQLite is last specifically so a mid-purge failure leaves a recoverable record. `3d52074 — src/purge.rs:248-301 — plan_for_session` places `SqliteRow` last. But `3d52074 — src/purge.rs:326-380 — execute` records an `Error` and continues iterating; the final row deletion runs regardless of prior outcomes. The CLI can return failure afterward, but the identity has already been destroyed.

**Adversarial verification.** “SQLite last” protects process crash before the last step, but not a handled `remove_dir_all` or log-redaction error—the much more likely partial failure. `execute_item` has no dependency gate and the report is built only after all entries. Happy-path, dry-run, skip, and ordering tests do not inject a preceding error and assert row retention. Direct recoverability/data-loss failure remains P0.

**Required proof direction.** Delete the row only if every required preceding item is Done/acceptable Skip; retain enough durable plan identity for retry and test a forced error in each category.

## P0-04 — Purge does not enforce its promised deletion containment

**Impact.** A malformed/corrupt session row can direct the explicitly destructive purge command at an arbitrary user-writable directory, including outside the configured worktree/AMQ roots.

**Baseline evidence.** `3d52074 — src/purge.rs:16-18 — module contract` promises refusal outside the configured worktree root. `3d52074 — src/purge.rs:248-282 — plan_for_session` turns `session.worktree_path` directly into `PurgeItem::Worktree` and joins raw `branch_name` to the AMQ root. An absolute branch path replaces the join prefix. `paths` is explicitly unused at line 296. `3d52074 — src/purge.rs:383-409 — execute_item / execute_remove_dir` sends all directory items directly to `fs::remove_dir_all` with no canonical containment check. By contrast, `src/cli.rs:714-731 — remove_session_worktree` already uses `git::is_under` for the older reset path.

**Adversarial verification.** Purge requires `--hard` plus confirmation, so this is not an unprompted-delete claim. The confirmation describes a session, not an arbitrary expanded path, and the code/docs claim defense against malformed database rows. Provider paths are rooted after a generated encoding, but worktree and AMQ branch are not. Same-UID tampering is not needed: corruption, old imported data, or an absolute malformed value suffices. Because the outcome is arbitrary directory deletion inside the user's authority, P0.

**Required proof direction.** Canonicalize and validate every deletion target against its category root, reject absolute/traversal branch components, refuse root itself, and display validated paths before confirmation. Tests must include `/`, parent traversal, symlinks, absolute branches, missing paths, and valid descendants.

## P1-01 — Custom `STATE_ROOT` installs are split across custom and hard-coded roots

**Impact.** Installation can succeed under a custom persistent root while new shells, migration, peer lookup, and purge continue reading/writing `/data/state`, producing missing state or incomplete cleanup.

**Baseline evidence.** `3d52074 — dux-amq/install.sh:23,147-183 — STATE_ROOT preflight` explicitly supports non-`/data` values and creates the custom tree. Yet:

- `dux-amq/config/bashrc-additions.sh:13-22` hard-codes DUX, AMQ, binary, and record defaults under `/data/state`; installer substitution changes only the overlay version;
- `dux-amq/scripts/finalize-claude-migration.sh:133-147` migrates and links only `/data/state`;
- `src/purge.rs:90-103 — PurgeConfig::default_layout` hard-codes provider and AMQ roots;
- `src/peer.rs:625-639 — optional_amq_root` retains a hard-coded fallback after environment/sibling resolution.

**Adversarial verification.** An operator can manually export every variable, but the installer advertises one `STATE_ROOT` control and writes a shell stanza expected to persist it. No template substitution carries that value. The code paths are active and disagree immediately after a supported custom install. P1.

## P1-05 — Doctor's hash, JSON, anonymization, and read-only contracts are broken

**Impact.** The primary support tool can falsely report binary-integrity mismatch, emit invalid multi-document JSON, leak paths/identities in anonymized output, and mutate the database while advertised as read-only.

**Baseline evidence.** Four independently reproducible contract breaks cluster in the same command:

- `3d52074 — dux-amq/scripts/dux-amq-doctor:72-121,350-368 — timeout/to + integrity`: with GNU `timeout` installed (the production Linux path), `to sha256_file file` asks an external process to execute a shell function, which it cannot resolve; `actual` becomes `(error)` and a valid binary reports mismatch.
- `3d52074 — src/cli.rs:905-1024 — run_doctor`: Bash JSON is forwarded, then Rust prints a second top-level JSON object. The comment requires consumers to use `jq -s`, contradicting `README.md:296-309`, which promises machine-parseable `dux doctor --json` and a single attachable dump.
- The same Rust JSON/text at `src/cli.rs:963-1024` prints database paths and orphaned session IDs/provider/branch/worktree values without receiving or applying the `anonymize` flag. Bash anonymization at `dux-amq/scripts/dux-amq-doctor:749-769` rewrites only message fields, not arbitrary structured fields.
- `3d52074 — src/cli.rs:1053-1067 — collect_sessions_snapshot` calls `SessionStore::open`; `src/storage.rs:141-180` runs migrations on open. That violates `README.md:309`'s read-only guarantee.

**Adversarial verification.** On macOS without GNU timeout, the function call works directly, so the hash bug is platform-conditional—but Linux is the overlay's primary VM target and preflight expects GNU tools. Two newline-delimited JSON objects are individually valid but not one JSON value and fail a direct `jq .` consumer contract. Integrity PRAGMAs may be read-only, but migrations are not. These are concrete operator-tool failures and a missing diagnostic rail, P1.

## P1-07 — The fork's release installer downloads a different upstream binary

**Impact.** A release created from this repository uploads a local installer asset that fetches `patrickdappollonio/dux`, so users following the fork's release instructions install an upstream binary that may lack the fork's audited features and schema.

**Baseline evidence.** Baseline origin is `SiavZ/dux-amq-setup`. `3d52074 — .github/workflows/release.yml:15-160 — build job` builds the current checkout and uploads its archives. `3d52074 — .github/workflows/release.yml:162-173 — upload-install-script` uploads this repository's root `install.sh`. `3d52074 — install.sh:4,58-117 — installer` hard-codes `REPO="patrickdappollonio/dux"`, resolves that repository's latest/tag, and downloads its archive. The overlay installer also fetches upstream at `dux-amq/install.sh:218-227`.

**Adversarial verification.** Keeping an upstream remote is normal and does not itself imply a fork release. The decisive mismatch is that the workflow publishes locally built artifacts and a co-located installer whose repository constant points elsewhere. A caller can override only version/install directory, not repository. P1.

## P1-25 — Purge derives AMQ and log targets differently from the runtime

**Impact.** A successful purge can leave the session's actual AMQ state and log records behind while reporting that those categories were handled.

**Baseline evidence.** `3d52074 — src/purge.rs:277-282 — AMQ plan` uses the raw `session.branch_name`. Runtime delivery/identity resolution at `src/app/inject_runtime.rs:156-199 — match_receiver` prioritizes the sanitized worktree basename, then sanitized branch, matching wrapper behavior. Thus names that differ by case, slash, characters, or worktree basename point at different directories. For logs, `src/logger.rs:150-164 — resolve_log_path` supports an absolute configured path, while `src/purge.rs:426-500 — execute_redact_logs/collect_log_files` scans only flat `dux.log*` files under `paths.root` and never receives `LoggingConfig`.

**Adversarial verification.** Default generated paths often align, which explains happy-path coverage. Custom logging path and worktree/branch-derived handle priority are explicitly supported features, not corrupt inputs. The purge report treats a missing wrong target as Skip and can still delete the identity row, so later discovery is harder. P1.

## Operational notes not promoted to findings

- Hard purge's user confirmation and dry-run controls are real and should remain; they do not compensate for target validation or partial-failure ordering.
- The migration script's `/data/state` assumption is safe for its original fixed-layout use, but becomes a product inconsistency once the installer explicitly supports custom roots.
- Root installation via `sudo` is expected when `/usr/local/bin` is selected and is not, by itself, an elevation finding.

## Subsystem conclusion

These failures do not require a new installer framework. The smallest faithful repairs are preservation-aware config patching, one resolved state-root value propagated into generated files, validated purge plans with dependency-aware row deletion, and a doctor that produces one deliberately read-only/redacted data model.

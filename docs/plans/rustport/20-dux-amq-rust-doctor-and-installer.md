# Phase 20 — Port the doctor, the migration tool, and the installer

**Track:** E (serialized) · **Runs alone**
**Depends on:** 19, and 03 (a real release artifact must exist to fetch) · **Blocks:** 23

## Goal

Port the remaining applets — `dux-amq-doctor` (850 lines),
`finalize-claude-migration.sh` (149), and the testable body of `dux-amq/install.sh`
(565) — then switch the installer to deploy one binary with symlinks and retire the
bash.

## Module tree

```text
dux-amq-rust/src/
  doctor/
    mod.rs            ~140  section orchestration; ALWAYS exit 0
    text.rs           ~190  kv/color rendering, exact %-30s / %-5s widths
    json.rs           ~150  dotted-path setter (the j_set equivalent)
    anonymize.rs      ~200  $HOME -> /HOME; worktree -> /WT/branch-N; branch/agent caches
    sec_versions.rs   ~140  incl. the awk-equivalent DUX_AMQ_VERSION scrape
    sec_binary.rs     ~130
    sec_disk.rs       ~150
    sec_amq.rs        ~240
    sec_symlinks.rs   ~110
    sec_kernel.rs      ~90
    sec_sessions_db.rs ~160 rusqlite instead of the sqlite3 CLI
    sec_runtime.rs    ~120  sysinfo instead of ps
    sec_errors.rs     ~170
  migrate/
    finalize.rs       ~270  flock -n, 4x ensure_no_claude, rsync, backup, atomic swap
  install/
    mod.rs            ~150  step orchestration
    blocks.rs         ~220  strip_block sh/md + byte-identical marker emission
    tiocsti.rs         ~90  the tri-state truth table + sentinel
    config_patch.rs   ~200  the 12 scoped substitutions (through symlinks)
    vscode.rs         ~140  JSONC strip + concat/sort/dedup merge
    pins.rs           ~120  the pin table + sha256 verification
```

## Two improvements the port delivers for free

- **`rusqlite` removes the `sqlite3-cli-missing` degraded state.** The doctor's
  sessions-DB section currently shells out; `doctor.bats:184` skips entirely when
  `sqlite3` is absent. In Rust the section always runs.
- **`sysinfo` removes an un-timeout-guarded `ps` fork** (`dux-amq-doctor:700-705`).

Both must preserve the **read-only** guarantee: `doctor.bats:184` asserts the DB's
sha256, `.schema`, and `pragma user_version` are byte-identical before and after.
`rusqlite` must open in true read-only mode.

## Work items

1. **Port `doctor/` with all 9 sections** — `Versions`, `Binary integrity`,
   `Persistent disk`, `AMQ`, `Symlinks`, `Kernel`, `Sessions DB`, `Runtime`,
   `Recent errors` — rendered even on a partial install.
   - **Always exit 0**, including when AMQ is uninitialized; JSON must then report
     `.amq.initialized == false`.
   - Binary integrity must invoke the equivalent of `timeout 5 sha256sum <pinned-binary>`
     and report `"sha256 matches"`.
   - `--json` emits one valid object with all 8 named top-level keys, each validly typed.
   - `--anonymize` redacts `$HOME` paths and replaces agent handles with `agent-N`
     **everywhere, including inside scraped log lines**, in both text and JSON modes.
   - Preserve the exact `%-30s` / `%-5s` column widths — `doctor.bats` pins the rendering.
2. **Write tests for the four sections the bash never covered.** `Persistent disk`,
   `Symlinks`, `Kernel`, and `Runtime` currently have **no assertion beyond their header
   string appearing**. The port is the moment to fix that.
3. **Port `migrate/finalize.rs`.** Must refuse while any `claude`/`claude-amq` process is
   alive and **leave the source untouched**; be a **true no-op** on an already-symlinked
   tree (no rsync call, message `"already a symlink"`); **never pass `--delete`** by
   default; add it only under `FINALIZE_FORCE_DELETE=1`; and serialize via a
   **non-blocking** flock so a second invocation fails fast with `"another instance"`
   rather than hanging.
   - The bash's real-rsync-failure path is untested (the fake always exits 0) — add
     coverage.
4. **Port `install/blocks.rs`** — the `strip_block` logic, including the historical
   audit02 **P0-G** bug: removing a legacy unversioned AMQ block must **not** delete
   everything after it to EOF. Versioned marker blocks are fully removed; an explicit
   end-sentinel resets the "still stripping" state. Marker emission must be
   **byte-identical** to the bash so existing installs strip cleanly.
5. **Port `install/tiocsti.rs`** — the tri-state truth table: file absent → 2, `1` → 0,
   `0` → 1, garbage → 2, empty → 2. Preserve both the sentinel write and clear sites.
6. **Port `install/config_patch.rs` and `vscode.rs`** — the 12 scoped substitutions
   (which must operate **through symlinks**) and the JSONC strip + concat/sort/dedup merge.
   `configure_vscode_remote` (`install.sh:488`) has **zero** test coverage today; write it.
7. **Port `install/pins.rs`** — the pin table and sha256 verification. Consumes Phase 03's
   refreshed pins; do not re-derive them here.
8. **Reduce `dux-amq/install.sh` to a thin bootstrap**: (a) preflight, (b) fetch or build
   `dux-amq`, (c) delegate everything else to
   `dux-amq install --state-root … [--skip-peers] [--dry-run]`.
   - Preflight must still **aggregate all missing tools** into one message
     (`"missing required tools:"`), naming `curl`, `jq`, `openssl`, `realpath` — never
     bail on the first.
   - The tool list shrinks: `jq` and `sqlite3` are no longer needed at runtime.
9. **Switch installation to one binary plus symlinks.** Replace nine
   `install -m 0755 <script> <dest>` calls with one binary install plus
   `ln -sfn dux-amq $LOCAL_BIN/<applet>` for each of the 10 names.
   **`finalize-claude-migration.sh` keeps its `$STATE_ROOT/scripts/` destination**, as a
   symlink. **The two `.sh` suffixes stay** — symlink names must remain byte-identical.
10. **Repoint `dux`'s doctor integration.** `resolve_doctor_script` (`cli.rs:879`,
    relocated to `cli/doctor.rs` in Phase 14) hardcodes the binary name and a relative
    fallback (`cli.rs:892`). Point it at the new applet and use the config key Phase 14
    introduced.
11. **Preserve idempotency.** A rerun must not invoke `dux config` when a config exists
    (the fake `dux` in `install-idempotency.bats` exits 97 if it is), must keep
    `config.toml` byte-identical, must not run `amq init --force`, and must not leak
    `/data/state` into `.bashrc`.
12. **Preserve the `bashrc-additions.sh` guards**, which fail **closed**: refuse when
    `binary.sha256` is missing, and refuse when the binary's mtime is newer than the
    recorded hash (`"newer than recorded hash"`). The happy-path `eval` is untested today
    — add that test.
13. **Delete the bash scripts** in a single commit with the installer switch. Retain
    `install-gocryptfs.sh` (opt-in, no coverage, out of scope) and the root `install.sh`
    (bootstrap), and say so in `dux-amq/README.md`.
14. **Re-anchor the four `supply-chain-rails.bats` tests.** They are pure `grep`
    assertions against literal bash and YAML source text that will no longer exist. Three
    must be re-anchored to the new build/release tooling; **the real double-tar `cmp`
    reproducibility check is worth keeping and porting.** Same for
    `tiocsti-detect.bats:89` (a `grep -c` against installer text) and
    `encoder-fixtures.bats:148` (byte-identical-across-three-wrappers), which Phase 18
    already deletes.

## Acceptance criteria

- [ ] All 7 `doctor.bats`, 5 `finalize-migration.bats`, 3 `strip-block.bats`, 6
      `tiocsti-detect.bats`, and 3 `install-idempotency.bats` assertions pass against the
      Rust binary.
- [ ] Doctor renders all 9 sections, exits 0 when AMQ is uninitialized, and its JSON
      carries all 8 keys.
- [ ] Doctor opens the sessions DB **read-only**: sha256, `.schema`, and `user_version`
      byte-identical before and after — and the section runs **without** `sqlite3` installed.
- [ ] `Persistent disk`, `Symlinks`, `Kernel`, `Runtime` have real content assertions for
      the first time.
- [ ] `--anonymize` redacts in both text and JSON, including inside scraped log lines.
- [ ] P0-G preserved: legacy block removal does not truncate to EOF.
- [ ] Installer deploys one binary + 10 symlinks; both `.sh` suffixes preserved;
      `finalize-claude-migration.sh` still lands in `$STATE_ROOT/scripts/`.
- [ ] Preflight aggregates all missing tools; `jq`/`sqlite3` dropped from the runtime list.
- [ ] Idempotent rerun preserves `config.toml` byte-for-byte and never calls `dux config`.
- [ ] `bashrc-additions.sh` guards still fail closed; the happy-path `eval` is now tested.
- [ ] `dux doctor` works end-to-end via the Rust applet.
- [ ] Bash scripts removed except `install-gocryptfs.sh` and the root `install.sh`, with
      the exclusion documented.
- [ ] `supply-chain-rails.bats` re-anchored; the reproducibility `cmp` preserved.
- [ ] No file exceeds 500 lines; headers present; Phase 02 gates pass.

## Validation

```bash
cargo test --workspace --all-features
export PATH="$PWD/target/debug/applets:$PATH"
bats dux-amq/tests

# doctor parity: compare bash vs Rust output on the same seeded state
dux-amq/scripts/dux-amq-doctor --json > /tmp/doctor-bash.json    # before removal
cargo run -p dux-amq -- dux-amq-doctor --json > /tmp/doctor-rust.json
jq -S . /tmp/doctor-bash.json > /tmp/a.json
jq -S . /tmp/doctor-rust.json > /tmp/b.json
diff /tmp/a.json /tmp/b.json

# full install on a clean container, twice, asserting idempotency
docker run --rm -v "$PWD:/src" ubuntu:24.04 bash -c \
  '/src/dux-amq/install.sh && /src/dux-amq/install.sh'
```

## Risks

| Risk | Mitigation |
|---|---|
| Doctor text output drifts (column widths, section order) | `doctor.bats` pins it; the bash-vs-Rust JSON diff above catches structural drift, and snapshot the text mode too |
| Marker emission differs, so existing installs fail to strip cleanly | Byte-identical marker output is an acceptance criterion; test against a fixture written by the bash version |
| Symlink switch breaks a `$PATH` assumption | `resolve_doctor_script` relies on `argv[0]` basename; Phase 18 verified this holds. Test with the real `which`-resolved path |
| `install-idempotency.bats` tests **silently skip** without `/data` + `dux` + `amq` | They report as skipped, not failed, on a dev laptop — run them in the gated overlay-CI job and assert they actually ran |
| Deleting the bash before parity is complete | Single commit, after the full suite passes against the Rust binary |

## References

- `artifacts/research-bash-port.md` §2 (doctor), §6 (finalize), §8 (installer), §10, §Migration/compat
- `artifacts/bats-test-parity-matrix.md` §doctor, §finalize-migration, §strip-block,
  §tiocsti-detect, §install-idempotency, §4 (hazards 6, 9, 10)

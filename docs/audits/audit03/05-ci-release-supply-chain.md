# CI, release, and supply chain

Query date for every live source in this file: **2026-07-11**. Baseline code evidence is `3d52074`. This section inventories the exact Cargo lock, GitHub Actions, downloaded/released artifacts, npm/git components, provider CLIs, and release-only tools. It also retains compact raw query results sufficient to repeat and compare the audit.

## Completeness status

**COMPLETE.** All mandatory feeds were reachable: OSV/RustSec exact-version queries, GitHub Global Security Advisory queries for Actions and npm packages, GitHub commit/tag/repository endpoints, npm registry metadata, NVD agent-CLI records, and live release artifacts. There is no date cutoff. The crates.io convenience endpoint for current release-tool versions returned an intermittent 503 during a supplemental check; it is not a mandatory advisory feed and the finding depends only on the baseline's absence of version constraints, so it does not make this section incomplete.

## P1-06 — CI shell lint omits the highest-risk extensionless overlay scripts

**Impact.** Authentication, verifier, bridge, and encoder shell changes can merge without ShellCheck even though CI says it lints the overlay scripts.

**Baseline evidence.** `3d52074 — .github/workflows/overlay-ci.yml:32-41 — shellcheck overlay scripts` expands only `dux-amq/scripts/*.sh`, so it omits extensionless `amq-receive-verify`, `amq-send-signed`, `dux-amq-doctor`, `dux-amq-inject-bridge`, and `encode-claude-project-dir`. `3d52074 — Makefile:22-32 — overlay-shellcheck` adds only doctor, still omitting the other four. These files contain HMAC parsing, replay state, queue routing, and path encoding.

**Adversarial verification.** The Bats suite exercises portions of these scripts but is not static lint, and not every branch has a test. Wrappers are included through `wrappers/*`; there is no equivalent `scripts/*`. This is a concrete safety-rail coverage gap, P1.

## P1-08 — The root installer ignores the release checksum and attestation

**Impact.** The documented easiest installation path executes an archive after transport-only download, leaving published integrity/provenance evidence unused.

**Baseline evidence.** `3d52074 — install.sh:99-125 — main` downloads the selected tarball and immediately extracts/installs it. `3d52074 — .github/workflows/release.yml:142-160,175-211 — attest/SHA256SUMS` publishes provenance attestations and a sorted checksum asset for those same archives. No installer path fetches either, and no embedded digest exists.

**Adversarial verification.** HTTPS/GitHub release hosting narrows transport risk but is not equivalent to checking the release's intentionally produced artifact digest or attestation. The overlay installer does verify its separately pinned Dux/AMQ artifacts; that positive control demonstrates the omission is local to the root installer. P1.

## P1-09 — Release packaging feeds an ISO timestamp to GNU tar's epoch syntax

**Impact.** Every release build reaches a packaging command that should reject or misparse its timestamp before archive upload, breaking the release pipeline.

**Baseline evidence.** `3d52074 — .github/workflows/release.yml:130-140 — Package (reproducible)` uses `--mtime='@${{ github.event.release.created_at }}'`. GitHub's [release REST schema](https://docs.github.com/en/rest/releases/releases?apiVersion=2022-11-28) represents `created_at` as an ISO-8601 string such as `2013-02-27T19:35:32Z`. GNU tar's [date input manual](https://www.gnu.org/software/tar/manual/html_chapter/Date-input-formats.html) reserves `@number` for seconds since the Unix epoch. Substitution therefore produces `@2013-02-27T...`, not an epoch number.

**Adversarial verification.** YAML/shell quoting preserves the substituted characters; it does not convert them. The command deliberately selects GNU tar (`gtar` on macOS), so BSD parsing is irrelevant. The input types are incompatible on all matrix jobs. P1.

## P1-10 — Release and security-gate tools are selected from a moving latest version

**Impact.** Identical source/tag reruns can install different build/SBOM/security tools, defeating reproducibility and allowing an upstream release to break or change the gate without a repository diff.

**Baseline evidence.** `3d52074 — .github/workflows/release.yml:90-104 — tool installation` runs unversioned `cargo install ... --locked` for `cargo-edit`, `cargo-auditable`, and `cargo-cyclonedx`. `3d52074 — .github/workflows/test.yml:62-70` and `.github/workflows/pr.yml:116-128` do the same for `cargo-audit` and `cargo-deny`. `--locked` selects the chosen crate release's own lockfile; it does not pin which crate release Cargo resolves.

**Adversarial verification.** Rust itself is pinned to 1.88.0 and Actions to commits, so the remaining moving inputs are not an intentional all-latest policy. Cache hits may hide drift but cannot define it, especially on a cold/new runner. Exact versions are absent from every command. P1.

## P1-11 — Claude Peers is installed and updated from an unpinned default branch

**Impact.** Any installer rerun changes executable MCP server code and its dependencies to whatever the repository default branch contains at that moment.

**Baseline evidence.** `3d52074 — dux-amq/install.sh:320-359 — Claude Peers MCP`: existing clones receive `git pull --ff-only`; new installs clone without a ref; `bun install` then runs and Claude registers `server.ts`. No commit/tag/hash is recorded. Live HEAD on the query date was `640183fa7048443bf0a6592de45579e813df4587` (verified GitHub commit, 2026-04-26T06:47:35Z).

**Adversarial verification.** Fast-forward-only protects local history from rewrite but intentionally advances it; it is not a version pin. Installation failure is soft, yet successful installation adds an executable MCP trust boundary. Repository security-advisory query returned zero, which does not make future unreviewed drift reproducible. P1.

## P1-12 — Provider wrappers enforce no minimum safe CLI version

**Impact.** Dux can launch known-vulnerable Claude, Codex, or Gemini CLI versions from `PATH` with no warning or fail-closed version floor, including malicious-repository command-execution classes covered by T1.

**Baseline evidence.** `3d52074 — dux-amq/wrappers/claude-amq:386 — final amq coop exec`, `3d52074 — dux-amq/wrappers/codex-amq:200 — final amq coop exec`, and `3d52074 — dux-amq/wrappers/gemini-amq:190 — final amq coop exec` pass the bare provider command; no wrapper parses or constrains `--version`. `3d52074 — SECURITY.md:86-89 — accepted risks` nevertheless says “We pin their CLIs.” Neither root nor overlay installer installs/pins those CLIs.

**Current advisory evidence.** GitHub Advisory Database package queries returned 27 Claude advisories, two Codex advisories, and one Gemini advisory. The highest current Claude patched floor among package-mapped advisories is 2.1.163; Codex has a patched 0.39.0 floor for GHSA-w5fx-fh39-j5rw and an older `<=0.23.0` advisory lacking patched metadata; Gemini's critical pre-sandbox `.gemini/.env` command injection is fixed in 0.39.1. NVD also records Codex fixes at 0.9.0 ([CVE-2025-54558](https://nvd.nist.gov/vuln/detail/CVE-2025-54558)) and 0.12.0 ([CVE-2025-55345](https://nvd.nist.gov/vuln/detail/CVE-2025-55345)), and the Gemini 0.39.1 floor ([CVE-2026-12537](https://nvd.nist.gov/vuln/detail/CVE-2026-12537)). Current npm latest versions were Claude 2.1.207, Codex 0.144.1, and Gemini 0.50.0 and fall outside all published ranges found.

**Adversarial verification.** Upstream CLI internals are out of scope and current latest packages are not alleged vulnerable. The local finding is the false pinning claim and lack of a rail: an old binary already on `PATH` is accepted. Rejected CVE-2026-35020/35021/35022 records were excluded; for example [CVE-2026-35021](https://nvd.nist.gov/vuln/detail/CVE-2026-35021) is explicitly rejected because the path is not normally triggerable. P1.

## P1-13 — Two newly open RustSec advisories make the exact-lock security gate fail

**Impact.** The baseline's mandatory `cargo audit --deny warnings` / `cargo deny` security job has two unignored exact-lock matches. This is a present CI blocker and dependency-maintenance signal.

**Baseline evidence.** `3d52074 — Cargo.lock:61-64,2080-2086 — anyhow 1.0.102 / scc 2.4.0`; `scc` is reached through dev dependency `serial_test` at `Cargo.lock:2179-2191` and `Cargo.toml:42`, while `anyhow` is direct at `Cargo.toml:7`. `3d52074 — deny.toml:15-36`, `.github/workflows/test.yml:62-73`, and `.github/workflows/pr.yml:116-128` ignore only RUSTSEC-2025-0141 and RUSTSEC-2024-0384. Exact-version OSV queries additionally match:

- [RUSTSEC-2026-0190](https://api.osv.dev/v1/vulns/RUSTSEC-2026-0190): `anyhow::Error::downcast_mut` unsoundness, fixed in 1.0.103;
- [RUSTSEC-2026-0205](https://api.osv.dev/v1/vulns/RUSTSEC-2026-0205): `scc::Array::insert` exception-safety/double-free issue, fixed in 3.8.4.

**Adversarial verification.** Full-tree search found no direct `downcast_mut` use, and `scc` is only a test-support transitive; no baseline code calls its `Array::insert`. Therefore this is not reported as a runtime exploit/P0. The gate uses warning-deny semantics and lacks ignores for both IDs, so the CI failure inference does not depend on exploitability. P1.

## Reproducibility evidence — exact Cargo lock

Source: baseline `Cargo.lock`, parsed into 375 registry/git package-version queries; batch endpoint [`POST https://api.osv.dev/v1/querybatch`](https://google.github.io/osv.dev/post-v1-querybatch/); details from `GET https://api.osv.dev/v1/vulns/<ID>`. Query date: 2026-07-11. No cutoff and no pagination were applied.

| Component + exact version | Advisory | Status/fix | Baseline treatment |
|---|---|---|---|
| `anyhow 1.0.102` | RUSTSEC-2026-0190 | affected; fixed 1.0.103 | unignored; new gate failure |
| `bincode 1.3.3` | RUSTSEC-2025-0141 | unmaintained; no patched version | explicitly ignored with review date |
| `instant 0.1.13` | RUSTSEC-2024-0384 | unmaintained; no patched version | explicitly ignored with review date |
| `scc 2.4.0` | RUSTSEC-2026-0205 | affected; fixed 3.8.4 | unignored; new gate failure, dev-transitive |

Retained raw output (compact projection of the batch and detail responses):

```text
query_count=375 result_slots=375 nonempty_slots=4
slot=5   package=anyhow  version=1.0.102 id=RUSTSEC-2026-0190 fixed=1.0.103 summary="Unsoundness in Error::downcast_mut()"
slot=12  package=bincode version=1.3.3   id=RUSTSEC-2025-0141 fixed=<none>  summary="Bincode is unmaintained"
slot=111 package=instant version=0.1.13  id=RUSTSEC-2024-0384 fixed=<none>  summary="instant is unmaintained"
slot=219 package=scc     version=2.4.0   id=RUSTSEC-2026-0205 fixed=3.8.4   summary="Array::insert ... compare function panics ... potential Double-Free"
```

## Reproducibility evidence — GitHub Actions

Source query per row: `GET https://api.github.com/advisories?ecosystem=actions&affects=<owner/repo@version>&per_page=100`, plus `GET https://api.github.com/repos/<owner/repo>/commits/<sha>`. Query date: 2026-07-11. All four advisory result arrays were empty; every pinned commit was reported verified. GitHub's [secure-use guidance](https://docs.github.com/en/actions/reference/security/secure-use) remains the policy source for full-length commit pinning.

| Component + baseline version | Pinned commit | Advisory source | Retained raw output |
|---|---|---|---|
| `actions/checkout 4.2.2` | `11bd71901bbe5b1630ceea73d27597364c9af683` | [query](https://api.github.com/advisories?ecosystem=actions&affects=actions%2Fcheckout%404.2.2&per_page=100) | `{"count":0,"ids":[]}; verified=true; 2024-10-23T14:24:28Z` |
| `dtolnay/rust-toolchain 1` | `e97e2d8cc328f1b50210efc529dca0028893a2d9` | [query](https://api.github.com/advisories?ecosystem=actions&affects=dtolnay%2Frust-toolchain%401&per_page=100) | `{"count":0,"ids":[]}; verified=true; 2025-08-23T01:20:49Z` |
| `Swatinem/rust-cache 2.7.5` | `82a92a6e8fbeee089604da2575dc567ae9ddeaab` | [query](https://api.github.com/advisories?ecosystem=actions&affects=Swatinem%2Frust-cache%402.7.5&per_page=100) | `{"count":0,"ids":[]}; verified=true; 2024-10-12T10:15:11Z` |
| `actions/attest-build-provenance 2.4.0` | `e8998f949152b193b063cb0ec769d69d929409be` | [query](https://api.github.com/advisories?ecosystem=actions&affects=actions%2Fattest-build-provenance%402.4.0&per_page=100) | `{"count":0,"ids":[]}; verified=true; 2025-06-11T17:32:50Z` |

Expected server-side settings (not audited or changed here): require pull-request review and the named PR/overlay/security checks, restrict force pushes/deletion, and protect release environments/tags. `.github/BRANCH_PROTECTION.md` describes the expected configuration; server-side GitHub configuration is out of scope.

## Reproducibility evidence — shipped/downloaded non-Cargo components

| Component + selected version | Baseline source | Live source URL | Query date | Retained raw output / result |
|---|---|---|---|---|
| Dux archive `v0.4.0`, Linux amd64 | `dux-amq/install.sh:34-35,218-227` | [release archive](https://github.com/patrickdappollonio/dux/releases/download/v0.4.0/dux-linux-amd64.tar.gz) | 2026-07-11 | `sha256=a1c449989e9c4dd53b260d75d29d0d5d6832b3852cf5327f3725b5e7bb881102` = pin; tag object commit `8411a995164ae3947ce7452be3c3d1ff75620193` |
| AMQ archive `v0.34.0`, Linux amd64 | `dux-amq/install.sh:36-46,231-305` | [release archive](https://github.com/avivsinai/agent-message-queue/releases/download/v0.34.0/amq_0.34.0_linux_amd64.tar.gz) | 2026-07-11 | archive `sha256=cba940987d00a3d072f395c7ec7a648e47d652f1ff503abf46da538595510d7a` = pin; extracted binary `eb78901f3dd13534884923e02ad9c6852be1b0a4c7f452fe52b8bcd795e3556b` = pin; tag commit `6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54` |
| `skills 1.5.3` npm CLI | `dux-amq/install.sh:39,307-318` | [registry metadata](https://registry.npmjs.org/skills/1.5.3) | 2026-07-11 | `integrity=sha512-lgZa7NvOGJA1WcwyqsOysO3ojMabq4R/NdQGNbry05y5ccDb/8CFed0HoSU9fvKfjuGbXSCVoV++4gcLMKAykQ==`; exact GitHub advisory query count 0; postinstall disabled |
| AMQ skills source | `dux-amq/install.sh:40,314-315` | [commit](https://github.com/avivsinai/agent-message-queue/commit/6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54) | 2026-07-11 | exact `SKILLS_REV=6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54`, same as v0.34.0 tag |
| Claude Peers MCP | `dux-amq/install.sh:320-359` | [HEAD commit API](https://api.github.com/repos/louislva/claude-peers-mcp/commits/HEAD) | 2026-07-11 | baseline version unconstrained; current `640183fa7048443bf0a6592de45579e813df4587`, `verified=true`, repository advisory count 0 |
| Root release Dux binary | `install.sh:4,58-117` | [upstream releases API](https://api.github.com/repos/patrickdappollonio/dux/releases/latest) | 2026-07-11 | version unconstrained unless `DUX_VERSION`; no digest/attestation check (P1-07/P1-08) |
| `cargo-edit`, `cargo-auditable`, `cargo-cyclonedx`, `cargo-audit`, `cargo-deny` | workflows cited in P1-10 | [crates.io registry](https://crates.io/) | 2026-07-11 | version selector is unconstrained latest at job execution; `--locked` present; supplemental version endpoint 503, finding unaffected |

Repository advisory endpoints for `patrickdappollonio/dux`, `avivsinai/agent-message-queue`, and `louislva/claude-peers-mcp` returned zero published repository advisories on the query date. That is recorded as evidence, not proof of absence.

## Reproducibility evidence — provider/terminal-agent CLIs

The wrappers select the installed `PATH` version; “current latest” is evidence for floor calculation, not the version Dux guarantees.

| Component | Baseline-selected version | Current registry version/integrity | Advisory query and retained raw result |
|---|---|---|---|
| `@anthropic-ai/claude-code` | unconstrained | `2.1.207`; `sha512-30D3lGMO0iPmWnz6rCSHKd6d6gWtQ8CV2zKHHspEHZFyjDJsNOr2Xg6Kb/XEPsaFFu4BGIJz/7L6FQkcFq5cNA==` ([metadata](https://registry.npmjs.org/%40anthropic-ai%2Fclaude-code/latest)) | [GitHub advisory query](https://api.github.com/advisories?ecosystem=npm&affects=%40anthropic-ai%2Fclaude-code&per_page=100): `count=27`, no withdrawn records; highest patched floor `2.1.163`; exact latest is outside all returned ranges |
| `@openai/codex` | unconstrained | `0.144.1`; `sha512-Xir1zqPfpenhdoAoshN53uonzbBXj18COyzRkFlVZpSNyEl5XtkuYu9oddELePFN7K/0sXUcSO34Ad5IeCXPbw==` ([metadata](https://registry.npmjs.org/%40openai%2Fcodex/latest)) | [query](https://api.github.com/advisories?ecosystem=npm&affects=%40openai%2Fcodex&per_page=100): `count=2`: GHSA-xrxf-jgv3-qmrm `<=0.23.0`, no patch metadata; GHSA-w5fx-fh39-j5rw `0.2.0..=0.38.0`, fixed `0.39.0`; exact latest outside ranges |
| `@google/gemini-cli` | unconstrained | `0.50.0`; `sha512-Z2HSR+UWgOP11WYomOymglYk2AcwEdFfQKytzVla9tkqMIYGVgIvXobmdxI4cgfPQ54XKE9V9uQBP2PrDhvljQ==` ([metadata](https://registry.npmjs.org/%40google%2Fgemini-cli/latest)) | [query](https://api.github.com/advisories?ecosystem=npm&affects=%40google%2Fgemini-cli&per_page=100): `count=1`, GHSA-wpqr-6v78-jr5g `<0.39.1` plus one preview range, fixed `0.39.1`; exact latest outside ranges |

Retained raw Claude advisory ID/range projection (27 records, `withdrawn_at=null` for all):

```text
GHSA-4vp2-6q8c-pvq2 >=2.1.59,<2.1.128 -> 2.1.128
GHSA-fg94-h982-f3mm >=0.2.54,<2.1.163 -> 2.1.163
GHSA-q5hj-mxqh-vv77 >=2.1.63,<2.1.84 -> 2.1.84
GHSA-vp62-r36r-9xqp <2.1.64 -> 2.1.64
GHSA-5cwg-9f6j-9jvx <2.1.75 -> 2.1.75
GHSA-mmgp-wc2j-qcv7 <2.1.53 -> 2.1.53
GHSA-ff64-7w26-62rf <2.1.2 -> 2.1.2
GHSA-4q92-rfm6-2cqx <2.1.7 -> 2.1.7
GHSA-mhg7-666j-cqg4 <2.0.55 -> 2.0.55
GHSA-66q4-vfjg-2qhh <2.0.57 -> 2.0.57
GHSA-qgqw-h4xq-7w8w <2.0.72 -> 2.0.72
GHSA-q728-gf8j-w49r <2.0.74 -> 2.0.74
GHSA-vhw5-3g5m-8ggf <1.0.111 -> 1.0.111
GHSA-jh7p-qr78-84p7 <2.0.65 -> 2.0.65
GHSA-xq4m-mc3c-vvg3 <1.0.93 -> 1.0.93
GHSA-7mv8-j34q-vp7q <2.0.31 -> 2.0.31
GHSA-5hhx-v7f6-x7gv <1.0.39 -> 1.0.39
GHSA-66m2-gx93-v996 <1.0.120 -> 1.0.120
GHSA-4fgq-fpq9-mr3g <1.0.111 -> 1.0.111
GHSA-2jjv-qf24-vfm4 <1.0.39 -> 1.0.39
GHSA-j4h9-wv2m-wrf7 <1.0.105 -> 1.0.105
GHSA-qxfv-fcpc-w36x <1.0.105 -> 1.0.105
GHSA-ph6w-f82w-28w6 <1.0.87 -> 1.0.87
GHSA-x5gv-jw7f-j6xj <1.0.4 -> 1.0.4
GHSA-x56v-x2h6-7j34 <1.0.20 -> 1.0.20
GHSA-pmw4-pwvc-3hx2 <0.2.111 -> 0.2.111
GHSA-9f65-56v6-gxw7 >=0.2.116,<1.0.24 -> 1.0.24
```

## Supply-chain conclusion

The baseline has several strong controls: exact Cargo checksums, a pinned Rust toolchain, full-SHA Actions, verified Dux/AMQ overlay artifacts, disabled npm install scripts for `skills`, SBOM generation, and provenance attestation. The confirmed gaps are at handoff points: lint globs, installer consumption of release evidence, release timestamp typing, and unconstrained executable/tool versions. Repairs should extend the existing pin/check pattern, not introduce a new supply-chain platform.

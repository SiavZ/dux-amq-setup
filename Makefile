.PHONY: run fmt fmt-check lint lint-fix profiling overlay-shellcheck overlay-bats overlay-test

run:
	cargo run

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

lint:
	cargo clippy --all-targets --all-features

lint-fix:
	cargo clippy --all-targets --all-features --fix --allow-dirty --allow-staged

profiling:
	@command -v flamegraph >/dev/null 2>&1 || { echo "Error: 'flamegraph' not found. Install it with: cargo install flamegraph"; exit 1; }
	cargo flamegraph --profile profiling --bin dux -o flamegraph.svg

# Local mirror of .github/workflows/overlay-ci.yml (audit01 Phase 00, audit03
# P1-06). Run before pushing changes that touch dux-amq/ or an installer.
# ShellCheck enumerates every executable script, extensionless ones included,
# plus the release helper scripts this fork added and the sourced bashrc snippet.
overlay-shellcheck:
	@command -v shellcheck >/dev/null 2>&1 || { echo "Error: 'shellcheck' not found (apt-get install shellcheck)"; exit 1; }
	find install.sh dux-amq -type f -perm -111 -print0 | xargs -0 shellcheck
	shellcheck dux-amq/config/bashrc-additions.sh .github/scripts/verify_fork_lineage.sh .github/scripts/set_workspace_version.sh

overlay-bats:
	@command -v bats >/dev/null 2>&1 || { echo "Error: 'bats' not found (apt-get install bats)"; exit 1; }
	bats dux-amq/tests

overlay-test: overlay-shellcheck overlay-bats

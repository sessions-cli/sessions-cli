.PHONY: install deploy build reload setup uninstall release release-public \
	check ci test check-private fmt clippy

# First-time or upgrade install with dependency checks.
setup:
	@./install.sh

uninstall:
	@./uninstall.sh

# Default dev workflow: build + deploy + macOS codesign.
install deploy:
	@./bin/dev-install.sh

# Build, deploy, and restart daemon + sidebar (keeps tmux workspaces).
reload:
	@./bin/reload.sh

build:
	@cargo build --release --locked

# Single-commit snapshot to sessions-cli/sessions-cli (see release.sh).
# Preflight is the full local CI suite (same as GitHub Actions).
release release-public:
	@./release.sh

# ---------------------------------------------------------------------------
# Local CI — same gates as .github/workflows/ci.yml
# Always run `make check` (or `make test`) before commit/push/release.
# GitHub Actions is a remote mirror, not the primary gate.
# ---------------------------------------------------------------------------
check ci test:
	@./scripts/ci-local.sh all

fmt:
	@./scripts/ci-local.sh fmt

clippy:
	@./scripts/ci-local.sh clippy

check-private:
	@./scripts/ci-local.sh guards

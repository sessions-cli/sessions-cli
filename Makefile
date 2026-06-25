.PHONY: install deploy build reload setup uninstall release release-public

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
	@cargo build --release

# Single-commit snapshot to sessions-cli/sessions-cli (see release.sh).
release release-public:
	@./release.sh

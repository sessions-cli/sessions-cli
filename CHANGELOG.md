# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.0] - 2026-06-14

Initial public release.

### Added

- Ratatui sidebar with live agent state: working, approval, error, and turn-complete (done)
- tmux workspace layout with a dedicated `sessions-ui` sidebar pane
- New session launcher UI for starting agent sessions from the bar
- Top-level CLI commands for common agents and session management
- `sessions doctor` install and hook health checks
- `sessions hooks setup` / `sessions hooks status` for Grok, Codex, Claude, and OpenCode
- One-line install and uninstall scripts (`install.sh`, `bin/uninstall.sh`)
- Portable path helpers (`SESSIONS_*` overrides) and daemon lifecycle scripts
- macOS codesign-at-install deploy path and Linux support

### Agent integrations

- Grok: lifecycle hooks plus `turn_complete` notification in `~/.grok/config.toml`
- Codex: hook install via `~/.codex/hooks/`
- Claude: hooks merged into `~/.claude/settings.json`
- OpenCode: plugin package embedded in the binary and installed by hooks setup

### Deferred

- Shell completions and manpage generation (tracked for a future release)
# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [0.1.1] - 2026-07-11

Public snapshot refresh from the development tree (post-0.1.0).

### Added

- Scheduled automations panel (CLI-agnostic runs, path pickers, schedules)
- Auto-collapse sidebar rail for narrow clients, with peek expand and rail status
- Notepad markdown task lists (checkbox edit, completion markers)
- Configurable PWD-group quick-launch badges (up to 4 agents)
- Privacy release guard in maintainer CI (blocks personal paths and private project markers from public snapshots)

### Fixed

- Sidebar focus, hover, and workspace click reliability under tmux
- Session grouping for home/`~` launches and tombstoned closes
- OpenCode / Grok title and hook routing edge cases
- Settings panel panic when content exceeds pane height
- Cold-start loading feedback for `sessions up` and sidebar
- CI: rustfmt, clippy `-D warnings`, telemetry and environment-dependent tests

### Changed

- Test fixtures scrubbed to synthetic paths/projects (no personal home paths)
- Grok model picker defaults updated (Grok 4.5 / Composer 2.5)
- Optional Ghostty OLED theme under `integrations/ghostty/`

### Security / privacy

- Release and CI refuse personal `/Users/<name>` paths, private project markers, and tracked secrets in the would-ship tree

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
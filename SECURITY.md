# Security

## Reporting a vulnerability

If you discover a security issue, please report it privately via [GitHub Security Advisories](https://github.com/sessions-cli/sessions-cli/security/advisories/new) rather than opening a public issue.

Include enough detail to reproduce the problem and, if possible, a suggested fix.

## Scope

sessions-cli runs locally on your machine. It manages tmux sessions, reads agent hook notifications, and writes state under `~/.local/share/sessions`. It does not transmit session content to a remote service in this release.
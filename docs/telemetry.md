# Telemetry and updates

sessions-cli telemetry is **opt-in**. Default level is `off` — no network requests.

## Levels

| Level | Sends | Receives |
|---|---|---|
| `off` | Nothing | Nothing |
| `updates_only` | install_id, version, os, arch, channel | Update availability |
| `full` | Above + aggregated usage rollups | Updates + stored analytics |

## Controls

```bash
sessions telemetry status
sessions telemetry enable updates_only
sessions telemetry enable full
sessions telemetry disable
sessions telemetry log      # preview payload, no send
sessions telemetry export
```

Environment:

- `DO_NOT_TRACK=1` — forces off
- `SESSIONS_TELEMETRY=off|updates_only|full|log`
- `SESSIONS_HEARTBEAT_URL` — staging endpoint override

## Never collected

Usernames, hostnames, emails, paths, repo names, prompts, session titles, agent session IDs, MCP config, or per-hook event streams.

## Updates

When opted in, the daemon checks for updates periodically. Cached results appear in:

- Sidebar banner (recommended or critical)
- `sessions status`
- `sessions doctor` (critical below `min_supported` fails)

Upgrade:

```bash
sessions upgrade
sessions upgrade --check
```

## Local audit log

With `full` telemetry, events append to `~/.local/share/sessions/telemetry/events.jsonl` (5MB rotation). Export with `sessions telemetry export`.

Schema: `telemetry.schema.json` in the repo root.
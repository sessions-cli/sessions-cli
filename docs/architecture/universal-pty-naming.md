# Universal PTY Naming and State Detection

How sessions-cli names every tmux pane and derives activity state without requiring per-agent configuration.

## Goal

Every sidebar row gets an automatic name and a reasonable activity state (working spinner, done highlight, error) for:

- AI agents (Grok, Codex, Claude, OpenCode, and unknown future binaries)
- Dev tools (`npm run dev`, `cargo watch`, `python manage.py runserver`)
- Plain shells at a prompt

Hooks still provide the richest experience when an agent integrates them. PTY classification provides a universal baseline when hooks are absent.

## Module layout

| Module | Role |
|---|---|
| `src/pty/classify.rs` | Classify foreground process as shell vs tool; extract prompt-like args |
| `src/pty/naming.rs` | Resolve display titles from hooks, summaries, classification, and workspace config |
| `src/pty/lifecycle.rs` | Infer `AgentState` from process liveness and exit status |
| `src/daemon/tmux.rs` | Poll tmux for pane command, cwd, `pane_dead`, and merge lifecycle state into sessions |

`src/pty/mod.rs` re-exports the public surface used by the daemon and agents.

## Naming: classify, don't allowlist

For each pane, `classify_pane(binary, full_command, cwd)` returns:

- **Shell** — binary is `zsh`, `bash`, `sh`, `fish`, or `nu`. Thread label comes from cwd leaf or `"console"`.
- **Tool** — any other foreground binary. App is the binary name (or registered profile display name). Thread is the first natural-language CLI arg, else a shortened command string.

```rust
// src/pty/classify.rs
pub enum PaneKind {
    Shell { name: String },
    Tool { app: String, thread: String, command: String, cwd: String },
}
```

Prompt extraction (`extract_natural_language_arg`) skips flags and picks the first positional arg that looks like natural language (spaces, vowels, not a path or URL). This works for `claude "refactor auth"` and filters `npm run dev --port 3000`.

Registered **app profiles** (`register_app_profile`) let known agents override prompt extraction or display formatting without maintaining a global agent allowlist for naming.

### Example titles

| Foreground process | Sidebar title |
|---|---|
| `zsh` in `~/projects/sessions-cli` | `console` or `sessions-cli` |
| `grok "fix sidebar"` | `grok · fix sidebar` |
| `claude "refactor auth"` | `claude · refactor auth` |
| `npm run dev` | `npm run dev` or `sessions-cli · npm run dev` |
| `cargo watch -x test` | `cargo watch` |

Launcher binaries (`node`, `python`, `deno`) keep the launcher name as app and the full command as thread. That is intentional — the user sees what is actually running.

### Layer cake (naming)

Sources are tried in order; later layers only fill gaps:

1. **Hook prompt** — `sessions notify --event prompt` payload
2. **Agent summary** — on-disk `summary.json` / adapter title (Grok and others)
3. **Process args** — `classify_pane` prompt extraction
4. **Command descriptor** — shortened full command for tools without a prompt arg
5. **Workspace config** — `workspaces.toml` project/thread overrides
6. **cwd leaf** — directory name
7. **`console`** — shell with no better label

`src/pty/naming.rs` implements confidence checks (`is_confident_thread_title`, `is_machine_derived_thread`) so probe pings and bootstrap commands do not replace real task names.

## State: process lifecycle baseline

`infer_pane_state(binary, pane_dead, exit_status)` in `src/pty/lifecycle.rs`:

| Condition | State |
|---|---|
| Shell binary, process alive | `Idle` |
| Non-shell, process alive | `Working` |
| Non-shell, exited 0 | `Done` |
| Non-shell, exited non-zero | `Error` |

Tmux supplies `pane_dead` and `pane_dead_status` in `list_windows` (`src/daemon/tmux.rs`). No extra subprocesses are required for lifecycle detection.

`merge_lifecycle_state(stored, polled)` prevents lifecycle from regressing richer hook-driven state (e.g. keep `Working` when hooks say working but tmux briefly reports done during a turn).

### Layer cake (state)

1. **Hook events** — `prompt`, `pre_tool`, `post_tool`, `turn_complete`, etc.
2. **Pane state files** — `pane-{id}.state` on disk; any agent can write
3. **Process lifecycle** — alive → Working; exit 0 → Done; exit ≠ 0 → Error
4. **`Idle`** — shell or unknown

Hooks win for approval (`pre_tool`) and per-turn boundaries. Lifecycle covers agents and dev servers that never call `sessions notify`.

### Lifecycle limitations (by design)

- **Approval** — lifecycle cannot distinguish "waiting for y/n" from "streaming tokens"; both look like Working.
- **Multi-turn REPLs** — one long-running process is one Working → Done cycle unless the agent fires `turn_complete`.
- **AI-generated titles** — only agents with summary files get post-hoc AI titles; others use extracted prompt or command.

## Daemon integration

On each tmux poll (`poll_tmux` in `src/daemon/tmux.rs`):

1. List windows with `pane_current_command`, cwd, `pane_dead`, `pane_dead_status`
2. `classify_pane` → app/thread for the row
3. `infer_pane_state` → lifecycle state
4. Merge with stored hook state and pane state files in `DaemonState`

When lifecycle marks Done and the session was in progress, the daemon sets `completed_thread` from the current description so the green highlight has text.

## Agent integration (optional, richer layers)

The hook protocol is agent-agnostic:

```bash
sessions notify --event prompt --stdin
sessions notify --event pre_tool
sessions notify --event post_tool
sessions notify --event turn_complete
sessions notify --event session_start
```

Install per-agent hooks via `bin/setup-agent-hooks.sh`. For agents without native hooks, `bin/sessionize` wraps a command and fires start/complete notifications.

See `AGENTS.md` for Grok vs Claude hook contracts (`turn_complete` vs `Stop`).

## Design principles

1. **No agent allowlist for naming** — any non-shell binary is a valid app.
2. **Generic prompt heuristic** — natural language has a detectable shape; no per-agent flag tables.
3. **Universal lifecycle** — every process starts, runs, and exits; tmux exposes death status for free.
4. **Layered sources** — hooks and summaries when available; PTY inference as baseline.
5. **Convention over configuration** — `sessions notify` is the integration surface; new agents do not require Rust changes for basic visibility.

## Related docs

- [`AGENTS.md`](../../AGENTS.md) — hook contracts and reload workflow
- [`docs/audits/`](../audits/) — performance and scale audits (managed-session identity is the next architectural step)
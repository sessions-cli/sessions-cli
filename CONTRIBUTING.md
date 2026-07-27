# Contributing

Thanks for helping improve Sessions. This guide covers how to set up, develop, and submit changes.

## Get started

```bash
git clone https://github.com/sessions-cli/sessions-cli.git
cd sessions-cli
./install.sh
sessions up
```

Fresh install is the adoption test: if your change is not active after that flow, it is incomplete.

## Development workflow

After Rust or UI changes:

```bash
make reload
sessions status
```

`make reload` builds, deploys, and restarts the daemon and sidebar. Run `cargo test` before opening a pull request.

## Portable changes

Every change must work on any machine through the install path — not only on the box where you ran `make reload`.

| You add… | Also update… |
|---|---|
| System dependency (rust, tmux, …) | `bin/install.sh` dependency checks |
| Deployed helper script | `bin/dev-install.sh` |
| New agent or hook contract | `src/hooks/` + `sessions hooks setup` |
| New config or state directory | `bin/paths.sh`; document `SESSIONS_*` overrides if needed |
| Runtime health / missing-setup signal | `sessions doctor` |
| Daemon or sidebar startup behavior | `bin/start-sessionsd.sh`, `bin/reload.sh` |

Do not rely on manual edits under `~/.grok/`, `~/.claude/`, or other agent config dirs unless the same result is wired into `sessions hooks setup` (called by `./install.sh`).

## Pull requests

1. Keep changes focused — one logical improvement per PR.
2. Match existing code style and naming in the files you touch.
3. Confirm `./install.sh` and `sessions up` still work for your change.
4. Open a PR against `main` with a short description of what changed and why.

Questions and early ideas are welcome in [GitHub issues](https://github.com/sessions-cli/sessions-cli/issues).

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Please be respectful in issues and pull requests.

## More docs

- [`docs/`](docs/README.md) — architecture notes, audits, and plans
- [`Agents.md`](Agents.md) — agent and hook contracts used during development
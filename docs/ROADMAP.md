# Roadmap

sessions-cli is focused on making multi-agent terminal workflows feel coherent without replacing the native tools that make each agent useful.

## Near term

- Harden hook setup across Grok, Codex, Claude, and OpenCode.
- Improve session state accuracy for working, approval, error, and complete states.
- Continue centralizing shared workflow pieces such as MCPs, skills, automations, notifications, and session metadata.
- Skills: skillshare control plane + `sessions skills` / `sessions skill` inventory, drift, and sync (see `docs/plans/skills-skillshare-integration-plan.md`).
- Make adoption smoother for users who want to test new models without changing their established terminal workflow.

## session-cloud

session-cloud is planned as the hosted backbone for sessions across different models. The intent is to make the project self-sustaining while centralizing management for the areas that model providers do not share, such as workflow state, cross-agent coordination, shared configuration, and team-level visibility.

The CLI and local TUI remain the core developer experience. The cloud layer should support that workflow rather than replace it.

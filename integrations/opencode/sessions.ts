import type { Plugin } from "@opencode-ai/plugin";

function tmuxEnv(): Record<string, string> {
  const env: Record<string, string> = {};
  if (process.env.TMUX_PANE) env.tmux_pane_id = process.env.TMUX_PANE;
  if (process.env.TMUX_SESSION) env.tmux_session = process.env.TMUX_SESSION;
  if (process.env.PWD) env.cwd = process.env.PWD;
  return env;
}

async function notify(
  $: (...args: TemplateStringsArray) => Promise<{ exitCode: number }>,
  event: string,
  sessionID: string,
  extra: Record<string, unknown> = {},
): Promise<void> {
  const payload = JSON.stringify({
    sessionId: sessionID,
    agent: "opencode",
    ...tmuxEnv(),
    ...extra,
  });
  await $`sessions notify --event ${event} --payload ${payload}`;
}

const SessionsPlugin: Plugin = async ({ $ }) => {
  const sessionStartNotified = new Set<string>();

  const notifySessionStartOnce = async (sessionID: string | undefined) => {
    if (!sessionID || sessionStartNotified.has(sessionID)) return;
    sessionStartNotified.add(sessionID);
    await notify($, "session_start", sessionID);
  };

  return {
    "chat.message": async (input, output) => {
      await notifySessionStartOnce(input.sessionID);
      const prompt = output.parts
        .filter((part) => part.type === "text")
        .map((part) => ("text" in part ? part.text : ""))
        .join("\n")
        .trim();
      await notify($, "prompt", input.sessionID, { prompt });
    },
    "permission.ask": async (input) => {
      await notify($, "pre_tool", input.sessionID);
    },
    "tool.execute.before": async (input) => {
      await notify($, "pre_tool", input.sessionID);
    },
    "tool.execute.after": async (input) => {
      await notify($, "post_tool", input.sessionID);
    },
    event: async ({ event }) => {
      if (
        event.type === "message.updated"
        && (event.properties as Record<string, unknown>)?.info
        && typeof (event.properties as Record<string, unknown>).info === "object"
      ) {
        const msg = (event.properties as Record<string, unknown>).info as Record<string, unknown>;
        if (
          msg.role === "assistant"
          && (msg.finish === "stop" || msg.finish === "end_turn")
          && !msg.error
        ) {
          await notify($, "turn_complete", msg.sessionID as string);
        }
      }
      if (event.type === "tui.session.select") {
        await notifySessionStartOnce(
          (event.properties as Record<string, unknown>).sessionID as string,
        );
      }
    },
    "shell.env": async (input, output) => {
      if (input.sessionID) {
        output.env.OPENCODE_SESSION_ID = input.sessionID;
        await notifySessionStartOnce(input.sessionID);
      }
      if (process.env.SESSIONS_SESSION_ID) {
        output.env.SESSIONS_SESSION_ID = process.env.SESSIONS_SESSION_ID;
      }
    },
  };
};

export default {
  id: "sessions",
  server: SessionsPlugin,
};

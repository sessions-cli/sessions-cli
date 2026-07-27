import type { Plugin } from "@opencode-ai/plugin";
import { readFileSync, existsSync, writeFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";

type QuestionOption = {
  label: string;
  description?: string;
};

type QuestionInfo = {
  question: string;
  header: string;
  options: QuestionOption[];
  multiple?: boolean;
  custom?: boolean;
};

type QuestionRequest = {
  id: string;
  sessionID: string;
  questions: QuestionInfo[];
};

function tmuxEnv(): Record<string, string> {
  const env: Record<string, string> = {};
  if (process.env.TMUX_PANE) env.tmux_pane_id = process.env.TMUX_PANE;
  if (process.env.TMUX_SESSION) env.tmux_session = process.env.TMUX_SESSION;
  if (process.env.PWD) env.cwd = process.env.PWD;
  return env;
}

function sessionsBinary(): string {
  try {
    const here = dirname(fileURLToPath(import.meta.url));
    const pinned = join(here, "sessions-bin");
    if (existsSync(pinned)) {
      const path = readFileSync(pinned, "utf8").trim();
      if (path) return path;
    }
  } catch {
    // fall through
  }
  return "sessions";
}

/**
 * OpenCode's native question Review UI can swallow Enter (opentui textarea bug).
 * Default "popup": collect answers via sessions tmux popup (reliable Enter).
 * Set SESSIONS_OPENCODE_QUESTION_MODE=native to use OpenCode's built-in UI.
 */
function questionMode(): "popup" | "native" {
  const raw = (process.env.SESSIONS_OPENCODE_QUESTION_MODE || "popup").toLowerCase();
  return raw === "native" ? "native" : "popup";
}

// Bun shell from the plugin context (tagged-template executor).
// eslint-disable-next-line @typescript-eslint/no-explicit-any
type Shell = (strings: TemplateStringsArray, ...values: any[]) => Promise<unknown>;

async function notify(
  $: Shell,
  event: string,
  sessionID: string,
  extra: Record<string, unknown> = {},
): Promise<void> {
  const bin = sessionsBinary();
  const payload = JSON.stringify({
    sessionId: sessionID,
    agent: "opencode",
    ...tmuxEnv(),
    ...(process.env.SESSIONS_SESSION_ID
      ? { sessions_session_id: process.env.SESSIONS_SESSION_ID }
      : {}),
    ...extra,
  });
  try {
    await $`${bin} notify --event ${event} --payload ${payload}`;
  } catch {
    // never block the agent on notify failure
  }
}

// Guard against double-load (plugins/ file + config "sessions" package entry).
const g = globalThis as typeof globalThis & { __sessionsOpencodePlugin?: boolean };

const SessionsPlugin: Plugin = async ({ $, client }) => {
  if (g.__sessionsOpencodePlugin) {
    return {};
  }
  g.__sessionsOpencodePlugin = true;

  const sessionStartNotified = new Set<string>();
  const questionsHandled = new Set<string>();

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
      if (event.type === "question.asked" && questionMode() === "popup") {
        const props = event.properties as QuestionRequest;
        if (!props?.id || !props?.questions?.length) return;
        if (questionsHandled.has(props.id)) return;
        questionsHandled.add(props.id);

        await notify($, "approval_required", props.sessionID, {
          kind: "opencode_question",
          request_id: props.id,
        });

        try {
          const bin = sessionsBinary();
          const reqPath = join(tmpdir(), `sessions-oc-q-${props.id}.json`);
          const outPath = join(tmpdir(), `sessions-oc-a-${props.id}.json`);
          writeFileSync(reqPath, JSON.stringify(props));

          const inTmux = Boolean(process.env.TMUX || process.env.TMUX_PANE);
          try {
            if (inTmux) {
              await $`tmux display-popup -w 90% -h 90% -T ${"OpenCode questions"} -E ${bin} opencode-question --request ${reqPath} --output ${outPath}`;
            } else {
              await $`${bin} opencode-question --request ${reqPath} --output ${outPath}`;
            }
          } catch {
            questionsHandled.delete(props.id);
            return;
          }

          if (!existsSync(outPath)) {
            questionsHandled.delete(props.id);
            return;
          }

          const answers = JSON.parse(readFileSync(outPath, "utf8")) as string[][];
          if (!Array.isArray(answers) || answers.length !== props.questions.length) {
            questionsHandled.delete(props.id);
            return;
          }

          // Submit via OpenCode SDK — dismisses native (broken) question UI and unblocks agent.
          await client.question.reply({
            path: { requestID: props.id },
            body: { answers },
          });
        } catch {
          questionsHandled.delete(props.id);
        }
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
      if (process.env.TMUX_PANE) {
        output.env.TMUX_PANE = process.env.TMUX_PANE;
      }
      if (process.env.TMUX_SESSION) {
        output.env.TMUX_SESSION = process.env.TMUX_SESSION;
      }
      if (process.env.PWD) {
        output.env.PWD = process.env.PWD;
      }
      if (process.env.SESSIONS_OPENCODE_QUESTION_MODE) {
        output.env.SESSIONS_OPENCODE_QUESTION_MODE =
          process.env.SESSIONS_OPENCODE_QUESTION_MODE;
      }
    },
  };
};

// OpenCode loaders require a plugin *function* export.
// Object `{ id, server }` shapes fail with: "Plugin export is not a function".
export default SessionsPlugin;
export const Sessions = SessionsPlugin;

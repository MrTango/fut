import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";

type Report = "idle" | "working" | "blocked" | "completed" | "exited";

const OSC_REPORTS: Record<Report, string> = {
  idle: "state=idle:app=pi",
  working: "state=working:app=pi",
  blocked: "state=blocked:app=pi",
  completed: "state=done:app=pi",
  exited: "state=clear",
};

export default function fut(pi: ExtensionAPI) {
  // Pi's interactive TUI writes to stdout, so an OSC 7501 report there stays
  // ordered with its rendering and reaches any terminal implementing the
  // protocol, including over SSH. Without a terminal, fall back to the CLI.
  const terminal = process.stdout.isTTY === true;
  const insideFut = Boolean(process.env.FUT_SOCKET && process.env.FUT_TERMINAL_ID);
  if (!terminal && !insideFut) return;

  let pending = Promise.resolve();
  const report = (state: Report, ctx: ExtensionContext) => {
    if (terminal) {
      try {
        process.stdout.write(`\x1b]7501;${OSC_REPORTS[state]}\x1b\\`);
      } catch {}
      return pending;
    }

    const agentSessionId = ctx.sessionManager.getSessionId();
    const args = [
      "agent",
      "report",
      state,
      "--source",
      "pi",
      "--agent-session-id",
      agentSessionId,
    ];

    pending = pending.then(async () => {
      await pi.exec("fut", args, { timeout: 2000 });
    }).catch(() => {});
    return pending;
  };

  pi.on("session_start", (_event, ctx) => report("idle", ctx));
  pi.on("agent_start", (_event, ctx) => report("working", ctx));
  pi.on("agent_settled", (_event, ctx) => report("completed", ctx));
  pi.on("tool_execution_start", (event, ctx) => {
    if (event.toolName === "ask_user") return report("blocked", ctx);
  });
  pi.on("tool_execution_end", (event, ctx) => {
    if (event.toolName === "ask_user" && !ctx.isIdle()) return report("working", ctx);
  });
  pi.on("session_shutdown", (_event, ctx) => report("exited", ctx));
}

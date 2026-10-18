import assert from "node:assert/strict";
import { afterEach, test } from "node:test";

import fut from "../fut.ts";

type Handler = (event: any, context: any) => unknown;

const originalSocket = process.env.FUT_SOCKET;
const originalTerminalId = process.env.FUT_TERMINAL_ID;
const originalIsTTY = Object.getOwnPropertyDescriptor(process.stdout, "isTTY");
const originalWrite = process.stdout.write;

afterEach(() => {
  restoreEnv("FUT_SOCKET", originalSocket);
  restoreEnv("FUT_TERMINAL_ID", originalTerminalId);
  if (originalIsTTY) Object.defineProperty(process.stdout, "isTTY", originalIsTTY);
  else delete (process.stdout as { isTTY?: boolean }).isTTY;
  process.stdout.write = originalWrite;
});

// Pi's stdout is its terminal. Each test chooses whether it has one.
function setTerminal(terminal: boolean): string[] {
  Object.defineProperty(process.stdout, "isTTY", { value: terminal, configurable: true });
  const written: string[] = [];
  // The test runner reports over stdout too; capture only status reports.
  process.stdout.write = ((chunk: unknown, ...rest: unknown[]) => {
    if (typeof chunk === "string" && chunk.startsWith("\x1b]7501;")) {
      written.push(chunk);
      return true;
    }
    return (originalWrite as (...args: unknown[]) => boolean).call(process.stdout, chunk, ...rest);
  }) as typeof process.stdout.write;
  return written;
}

function restoreEnv(name: string, value: string | undefined) {
  if (value === undefined) delete process.env[name];
  else process.env[name] = value;
}

function harness(options: { failFirst?: boolean; blockFirst?: boolean } = {}) {
  const handlers = new Map<string, Handler>();
  const calls: Array<{ command: string; args: string[]; options: unknown }> = [];
  let shouldFail = options.failFirst ?? false;
  let unblockFirst: (() => void) | undefined;
  const firstBlocked = new Promise<void>((resolve) => {
    unblockFirst = resolve;
  });

  const pi = {
    on(event: string, handler: Handler) {
      handlers.set(event, handler);
    },
    async exec(command: string, args: string[], execOptions: unknown) {
      calls.push({ command, args, options: execOptions });
      if (options.blockFirst && calls.length === 1) await firstBlocked;
      if (shouldFail) {
        shouldFail = false;
        throw new Error("fut unavailable");
      }
      return { stdout: "", stderr: "", code: 0, killed: false };
    },
  };

  const context = {
    sessionManager: { getSessionId: () => "pi-session-123" },
    isIdle: () => false,
  };

  return {
    calls,
    releaseFirst() {
      unblockFirst?.();
    },
    install() {
      fut(pi as never);
    },
    async emit(event: string, data: Record<string, unknown> = {}, ctx = context) {
      const handler = handlers.get(event);
      assert.ok(handler, `missing ${event} handler`);
      await handler({ type: event, ...data }, ctx);
    },
    handlers,
  };
}

test("reports the complete lifecycle in native event order", async () => {
  setTerminal(false);
  process.env.FUT_SOCKET = "/tmp/fut.sock";
  process.env.FUT_TERMINAL_ID = "terminal-42";
  const testHarness = harness();
  testHarness.install();

  await testHarness.emit("session_start", { reason: "startup" });
  await testHarness.emit("agent_start");
  await testHarness.emit("tool_execution_start", { toolName: "ask_user" });
  await testHarness.emit("tool_execution_end", { toolName: "ask_user" });
  await testHarness.emit("agent_settled");
  await testHarness.emit("session_shutdown", { reason: "quit" });

  assert.deepEqual(
    testHarness.calls.map(({ args }) => args[2]),
    ["idle", "working", "blocked", "working", "completed", "exited"],
  );
  for (const call of testHarness.calls) {
    assert.equal(call.command, "fut");
    assert.deepEqual(call.args.slice(0, 2), ["agent", "report"]);
    assert.deepEqual(call.args.slice(3), [
      "--source",
      "pi",
      "--agent-session-id",
      "pi-session-123",
    ]);
    assert.equal(call.args.includes("--turn-id"), false);
    assert.deepEqual(call.options, { timeout: 2000 });
  }
});

test("does not infer lifecycle from unrelated or already-idle tool events", async () => {
  setTerminal(false);
  process.env.FUT_SOCKET = "/tmp/fut.sock";
  process.env.FUT_TERMINAL_ID = "terminal-42";
  const testHarness = harness();
  testHarness.install();

  await testHarness.emit("tool_execution_start", { toolName: "bash" });
  await testHarness.emit("tool_execution_end", { toolName: "ask_user" }, {
    sessionManager: { getSessionId: () => "pi-session-123" },
    isIdle: () => true,
  });

  assert.deepEqual(testHarness.calls, []);
});

test("contains command failures and continues the serialized report queue", async () => {
  setTerminal(false);
  process.env.FUT_SOCKET = "/tmp/fut.sock";
  process.env.FUT_TERMINAL_ID = "terminal-42";
  const testHarness = harness({ failFirst: true });
  testHarness.install();

  await assert.doesNotReject(testHarness.emit("session_start"));
  await assert.doesNotReject(testHarness.emit("agent_start"));

  assert.deepEqual(
    testHarness.calls.map(({ args }) => args[2]),
    ["idle", "working"],
  );
});

test("serializes concurrent reports without reordering them", async () => {
  setTerminal(false);
  process.env.FUT_SOCKET = "/tmp/fut.sock";
  process.env.FUT_TERMINAL_ID = "terminal-42";
  const testHarness = harness({ blockFirst: true });
  testHarness.install();

  const idle = testHarness.emit("session_start");
  const working = testHarness.emit("agent_start");
  await new Promise((resolve) => setImmediate(resolve));

  assert.deepEqual(testHarness.calls.map(({ args }) => args[2]), ["idle"]);
  testHarness.releaseFirst();
  await Promise.all([idle, working]);
  assert.deepEqual(testHarness.calls.map(({ args }) => args[2]), ["idle", "working"]);
});

test("reports over OSC 7501 on its terminal instead of the CLI", async () => {
  for (const insideFut of [true, false]) {
    if (insideFut) {
      process.env.FUT_SOCKET = "/tmp/fut.sock";
      process.env.FUT_TERMINAL_ID = "terminal-42";
    } else {
      delete process.env.FUT_SOCKET;
      delete process.env.FUT_TERMINAL_ID;
    }
    const written = setTerminal(true);
    const testHarness = harness();
    testHarness.install();

    await testHarness.emit("session_start", { reason: "startup" });
    await testHarness.emit("agent_start");
    await testHarness.emit("tool_execution_start", { toolName: "bash" });
    await testHarness.emit("tool_execution_start", { toolName: "ask_user" });
    await testHarness.emit("tool_execution_end", { toolName: "ask_user" });
    await testHarness.emit("agent_settled");
    await testHarness.emit("session_shutdown", { reason: "quit" });

    assert.deepEqual(written, [
      "\x1b]7501;state=idle:app=pi\x1b\\",
      "\x1b]7501;state=working:app=pi\x1b\\",
      "\x1b]7501;state=blocked:app=pi\x1b\\",
      "\x1b]7501;state=working:app=pi\x1b\\",
      "\x1b]7501;state=done:app=pi\x1b\\",
      "\x1b]7501;state=clear\x1b\\",
    ]);
    assert.deepEqual(testHarness.calls, []);
  }
});

test("is inert without a terminal outside Fut", () => {
  setTerminal(false);
  delete process.env.FUT_SOCKET;
  delete process.env.FUT_TERMINAL_ID;
  const testHarness = harness();
  testHarness.install();

  assert.equal(testHarness.handlers.size, 0);
  assert.deepEqual(testHarness.calls, []);
});

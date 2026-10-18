# Fut lifecycle adapter for Pi

**Legacy fallback for status.** Pi 1.1.0 and later reports status to Fut
natively over OSC 7501 and needs no lifecycle extension. The extension remains
for older Pi versions. The package also bundles the Fut skill, which is still
worth installing.

This first-party Pi extension reports lifecycle state for the Pi session running
in a Fut terminal. It is a one-way adapter: it does not create or change
layouts, submit prompts, read terminal output, or coordinate agents.

## Install

Install the package from GitHub, then restart Pi:

```sh
pi install git:github.com/mikker/fut
```

For development, add a local checkout instead:

```sh
pi install /absolute/path/to/fut
```

Local paths are loaded in place. To register the package only for the current
project instead of in your user settings, add `-l`:

```sh
pi install -l /absolute/path/to/fut
```

The root `package.json` registers `integrations/pi/fut.ts` as the package's Pi
extension and the bundled Fut skill. Restart Pi after installing it. The skill
teaches Pi to inspect and control Fut resources without changing visual focus,
including reading another agent pane's lifecycle state and terminal output.

When Pi's stdout is a terminal, the adapter writes OSC 7501 reports
(`app=pi`; completed is `done`, exited is `clear`) to it, in order with Pi's
own output. This works in any terminal that implements the protocol, including
over SSH, and needs no `fut` binary.

Without a terminal, such as in print or RPC mode with redirected output, the
adapter falls back to the Fut CLI. That fallback is inert unless both
`FUT_SOCKET` and `FUT_TERMINAL_ID` are present, as they are for Pi started
inside Fut. The `fut` binary must be on `PATH` and provide:

```sh
fut agent report STATE --source SOURCE \
  [--agent-session-id ID] [--turn-id ID]
```

The daemon validates the inherited terminal ID against the reporting process
tree. Integrations omit `--terminal-id`; that option is reserved for external
controllers intentionally reporting for another terminal.

CLI reports are serialized in event order. Each CLI call has a two-second timeout,
and command failures are swallowed so lifecycle reporting cannot fail a Pi
event handler.

## Event mapping

| Fut state | Authoritative Pi evidence |
| --- | --- |
| `idle` | `session_start` |
| `working` | `agent_start`; end of an `ask_user` tool while Pi is still active |
| `blocked` | start of an `ask_user` tool execution |
| `completed` | `agent_settled`, after retries, compaction retries, and queued continuations are exhausted |
| `exited` | `session_shutdown` |

Every CLI report includes `source=pi` and Pi's
`SessionManager.getSessionId()` value as the agent session ID. Pi exposes a
per-run numeric turn index only on turn start/end events, not a stable turn ID
on lifecycle and tool events, so the adapter does not invent or forward one.

## Limitations

- Blocked/resumed reporting recognizes the native `ask_user` tool name. Other
  tools that implement their own user interaction do not provide authoritative
  ask-user lifecycle evidence and are not inferred as blocked.
- If Pi is terminated without delivering `session_shutdown`, Fut retains the
  last reported state until terminal exit handling clears it.
- A missing, incompatible, or unresponsive Fut CLI can delay an individual
  report by at most two seconds; the failure is ignored and later reports still
  run in order.

## Test

The tests use Node's built-in TypeScript stripping and test runner:

```sh
node --test integrations/pi/tests/fut.test.ts
```

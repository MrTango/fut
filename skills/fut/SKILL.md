---
name: fut
description: Control Fut, a project-oriented terminal multiplexer. Use when the user explicitly mentions Fut or asks to inspect or manage Fut resources, create background terminals, send or collect terminal I/O, or coordinate reported coding-agent activity.
---

# Fut

Use Fut's noun-first CLI to inspect and control sessions, workspaces, tabs,
panes, terminals, and integrated coding agents without changing visual focus.

## Discover the installed CLI

Treat the installed binary as authoritative. Inspect the relevant command before
using it:

```sh
fut --help
fut pane --help
fut pane split --help
fut terminal --help
fut agent --help
fut agent prompt --help
fut agent skill
```

Do not run bare `fut` for discovery; it opens the current directory and attaches
interactively. Do not assume commands from tmux, Herdr, or another Fut release
exist.

## Resolve explicit targets

Processes inside Fut inherit a complete resource chain:

```sh
printf '%s\n' \
  "$FUT_SESSION_ID" \
  "$FUT_WORKSPACE_ID" \
  "$FUT_TAB_ID" \
  "$FUT_PANE_ID" \
  "$FUT_TERMINAL_ID"
fut --json context
```

`context` treats `FUT_TERMINAL_ID` as the stable identity and resolves its
current live ancestry from the daemon. Ancestor variables describe the spawn
location and may be stale after a pane move. Treat `missing_context`,
`invalid_context`, `stale_context`, and `closing_context` as typed failures;
do not fill gaps from another client's focus.

Look up any known resource UUID explicitly, including from outside Fut:

```sh
fut --json get "$resource_id"
fut --json list
fut --json agent list
```

If a client reports that its protocol does not match the running daemon, do not
give up on read-only inspection. Find the daemon process which owns
`FUT_SOCKET`, resolve that process's executable, and use that exact binary with
`--socket "$FUT_SOCKET"` for `get`, `list`, `agent get`, `agent read`, and
`terminal read`. On macOS, `lsof -p DAEMON_PID` shows the executable as its
`txt` entry. Never use this fallback for mutations or lifecycle reports;
restart Fut so the normal `fut` on `PATH` matches the daemon first.

Read IDs from JSON responses. Creation commands put the new resource IDs under
`.result.selected`; agent commands use `.result.agent` or `.result.agents`.
Pass those IDs explicitly to every later command. Check the exit status, parse
successful JSON from stdout, and parse a structured JSON error from stderr on
failure.

For direct human use inside Fut, layout commands may omit the applicable
current ID (`tab new`, `tab rename NAME`, `pane split right`, `pane close`, and
similar forms). Automation should retain returned explicit IDs whenever it
owns a particular resource; omitted-ID mutations are guarded against ancestry
changes by the daemon.

## Create a background terminal

Split beside an existing pane to stay in the same tab. Omit `--cwd` to inherit
the anchor pane's current directory. The command creates the pane in the
background and never selects it in a client:

```sh
fut --json pane split "$FUT_PANE_ID" right -- codex
fut --json pane split "$FUT_PANE_ID" down --cwd /path/to/project -- zsh
```

Record `.result.selected.pane_id` and `.result.selected.terminal_id` immediately.
Use `pane new` when a tab ID, rather than an anchor pane, is the intended scope:

```sh
fut --json pane new "$FUT_TAB_ID" --cwd "$PWD" -- codex
```

Arguments after `--` are a child argument vector. Wait for a launched agent's
status report before expecting it in `agent list`; inspect its explicit
terminal ID with `agent get`.

## Control an ordinary terminal

Submit one literal shell command plus Enter atomically:

```sh
fut --json terminal run "$terminal_id" 'mise run test'
```

Use lower-level input only when atomic command submission is not appropriate:

```sh
fut --json terminal send-text "$terminal_id" 'draft text'
fut --json terminal send-keys "$terminal_id" ctrl+c
```

`send-text` does not press Enter. `send-keys` accepts named keys, characters,
and chords exposed by installed help.

Collect bounded output without attaching:

```sh
fut --json terminal read "$terminal_id"
fut --json terminal read "$terminal_id" --source recent-unwrapped --lines 200
fut --json terminal wait-output "$terminal_id" --literal 'READY' --timeout 30s
fut --json terminal wait-output "$terminal_id" --regex 'done [0-9]+' --timeout 2m
```

Prefer one daemon-side `wait-output` deadline to polling. Treat `output_timeout`,
`terminal_exited`, invalid regexes, and output limits as typed outcomes.

## Retire a disposable workspace

After an external tool has permanently removed the current workspace's backing
directory, request asynchronous retirement instead of leaving the terminal in a
deleted working directory:

```sh
fut --json workspace retire
fut --json workspace retire "$workspace_id"
```

Retirement is acknowledged before Fut terminates the workspace's terminals.
The inferred form validates the caller's live terminal context. Use ordinary
`workspace close` when the caller must wait for confirmed process termination;
use retirement when the caller is itself inside the target being closed.

## Inspect program status and coordinate agents

Fut learns program status primarily from OSC 7501, without a plugin. The
historical `agent` commands include any explicitly reporting program, not just
coding agents. CLI adapters are fallbacks that feed the same record model;
screen-detected terminals do not qualify for agent control. Discover and inspect
reporters without focus changes:

```sh
fut --json agent list
fut --json agent get "$terminal_id"
fut --json agent read "$terminal_id" --source recent-unwrapped --lines 200
```

Submit a prompt atomically. Add `--wait` and `--timeout` when the result is needed:

```sh
fut --json agent prompt "$terminal_id" 'Review the failing test.'
fut --json agent prompt "$terminal_id" 'Review it, fix it, and report tests.' --wait --timeout 2m
fut --json agent prompt "$terminal_id" --stdin < detailed-prompt.md
```

Prefer `--stdin` for multiline or generated prompts so their contents never
need to be interpolated into a shell argument.

Status JSON lives under `activity.records`: each record has an `id` (empty for
root), stored `state` (`idle`, `working`, `done`, `blocked`, `error`), and optional
`app`, `title`, `message`, `kind`, and `progress`. `activity.source` gives
provenance. Record summary priority is blocked > error > working > done > idle.
Do not expect the old `activity.state`, integration, detection, or event fields.

`prompt --wait` is the fresh-prompt barrier: it requires a new `working`
transition after submission before accepting settled `done`, `blocked`,
`error`, or `idle`. Inspect `.result.activity.records`; blocked and error are
successful structured outcomes, so surface the blocker or error and its output
instead of treating exit status zero as completion or blindly retrying.

Wait for work already in progress separately:

```sh
fut --json agent wait "$terminal_id" --timeout 2m
```

Standalone `agent wait` may return an already settled state immediately. It does
not establish a fresh prompt barrier. Treat `not_an_agent`, `agent_busy`,
`agent_timeout`, `agent_events_lagged`, `agent_exited`, `terminal_exited`, and
`input_backlog_full` (the program has stopped reading its input) as typed
failures. A retained older result must not settle fresh work on another
record; reporting-process exit while its terminal stays alive is `agent_exited`.

## Report lifecycle from an integration

Prefer native OSC 7501 when adding status to a program. For example, from its
own terminal:

```sh
printf '\e]7501;state=working:app=demo:progress=40\e\\'
printf '\e]7501;state=done:app=demo:msg=UmVhZHk=\e\\'
```

OSC `title` and `msg` values are base64 UTF-8 without control characters. An
optional `id=build/test` addresses a child record. Every report replaces its
record completely; repeat fields that should remain. `state=clear` clears all
records, or a subtree when an id is supplied. A support query
`OSC 7501 ; ? ST` receives `?:states=idle,working,done,blocked,error:kinds=permission,question,auth`
with the same terminator and no record contents. Queries are optional.

Only an integration for the process in that terminal should use the CLI fallback:

```sh
fut --json agent report idle --terminal-id "$FUT_TERMINAL_ID" --source codex
fut --json agent report working --terminal-id "$FUT_TERMINAL_ID" --source codex --agent-session-id "$agent_session_id" --progress 40 --app codex
fut --json agent report blocked --terminal-id "$FUT_TERMINAL_ID" --kind permission --message 'Apply these changes?'
fut --json agent report completed --terminal-id "$FUT_TERMINAL_ID" --message 'Ready for review'
```

Inside Fut, `--terminal-id` may be omitted because `FUT_TERMINAL_ID` is used.
Do not manufacture reports on behalf of another agent or infer semantic state
from its screen.

CLI `completed` stores done; `exited` clears the addressed subtree or all
records. Optional `--id`, `--app`, `--title`, `--message`, `--kind`, and
`--progress` express full records. CLI text is decoded plain text, not base64.
Older `--turn-id` is accepted but ignored. Prompt start or automatic process-exit
cleanup drops transient records, preserving done/error. Reading attention
quiets notifications daemon-wide without deleting the stored result.

## Handle alternate-screen programs

Recent history is unavailable while a full-screen program uses the alternate
screen. Fall back to its visible viewport:

```sh
fut --json agent read "$terminal_id" --source visible
fut --json terminal read "$terminal_id" --source visible
```

Prefer `agent read` for integrated agents because it includes lifecycle and
availability. Use raw `terminal read`, `send-text`, and `send-keys` for an
unintegrated interactive program. Never screen-scrape a completion state when
lifecycle reports are available.

## Clean up only owned resources

Keep the pane and terminal IDs returned by creation. Collect final output and
state before closing the pane:

```sh
fut --json agent get "$created_terminal_id"
fut --json agent read "$created_terminal_id" --source recent-unwrapped --lines 200
fut --json pane close "$created_pane_id"
```

If the process is not an integrated agent, use `terminal read` before closing.
Close only resources created for the current task unless the user explicitly
requests broader cleanup. Never select, attach to, or visually focus a resource
merely to inspect, control, wait for, or close it.

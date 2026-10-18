---
layout: default
title: Claude Code integration
description: Report Claude Code lifecycle activity to Fut.
permalink: /agents/claude-code/
---

# Claude Code integration

> **Legacy fallback.** Claude Code 2.1.295 and later emits OSC 7501 natively
> and shows status in Fut with nothing installed. Install this plugin only for
> older Claude Code versions or when terminal title reporting is disabled with
> `CLAUDE_CODE_DISABLE_TERMINAL_TITLE`. It does not help inside tmux: tmux
> drops OSC 7501, and Fut rejects CLI reports from outside the Fut terminal's
> process tree.

Install Fut first, then add Fut's marketplace and lifecycle plugin:

```sh
claude plugin marketplace add mikker/fut
claude plugin install fut@fut-integrations
```

Restart Claude Code and launch it inside Fut. Run `/hooks` in Claude Code to
confirm that the Fut handlers are loaded with source `Plugin`.

The adapter reports idle, working, blocked, completed, and exited activity. It
does not control prompts, permissions, tools, terminal layout, or worktrees,
and it remains inactive when Claude Code is running outside Fut.

The adapter uses Fut's CLI fallback: completed stores an OSC-model done record,
and exited clears its records. It feeds the same records as native OSC 7501, so
running both is harmless. Transient records are also cleaned up automatically
when the reporting process exits; done and error results survive automatic exit
cleanup.

The `fut` binary must be on `PATH`. Managed Claude Code policy can disable
plugin hooks; use `/hooks` or the plugin manager's Errors view if activity does
not appear.

See [Agent activity](../agents.md) for the sidebar, notifications, and agent
automation commands.

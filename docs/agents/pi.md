---
layout: default
title: Pi integration
description: Report Pi lifecycle activity to Fut.
permalink: /agents/pi/
---

# Pi integration

> **Legacy fallback for status; still useful for the skill.** Pi 1.1.0 and
> later emits OSC 7501 natively and shows status in Fut with nothing installed.
> The lifecycle extension matters only for older Pi versions or when Pi runs
> inside a multiplexer such as tmux that does not forward Fut's OSC 7501 support
> reply. The package also bundles the Fut skill, which is a reason to install
> it regardless.

Install Fut's Pi extension from GitHub:

```sh
pi install git:github.com/mikker/fut
```

Restart Pi after installation, then launch it inside Fut. The extension reports
Pi's idle, working, blocked, completed, and exited activity. Exiting Pi removes
that terminal from the Agents sidebar. The package also gives Pi a Fut skill so
it can inspect and control sessions, panes, terminals, and agents without
changing visual focus.

The extension uses `fut agent report` as a fallback producer of the same
status records as OSC 7501, so running it alongside Pi's native reporting is
harmless. Completed stores done; an explicit exited report clears its records.
Automatic process-exit cleanup instead preserves done and error results for
inspection.

The integration is a one-way lifecycle adapter. It does not create or change
layouts, submit prompts, read terminal output, or coordinate agents, and it
remains inactive when Pi is running outside Fut. The `fut` binary must be on
`PATH`.

See [Agent activity](../agents.md) for the sidebar, notifications, and agent
automation commands.

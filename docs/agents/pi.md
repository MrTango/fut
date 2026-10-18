---
layout: default
title: Pi integration
description: Report Pi lifecycle activity to Fut.
permalink: /agents/pi/
---

# Pi integration

> **Legacy fallback for status; still useful for the skill.** Pi 1.1.0 and
> later emits OSC 7501 natively and shows status in Fut with nothing installed.
> The lifecycle extension matters only for older Pi versions. The package also
> bundles the Fut skill, which is a reason to install it regardless.

Install Fut's Pi extension from GitHub:

```sh
pi install git:github.com/mikker/fut
```

Restart Pi after installation. The extension reports
Pi's idle, working, blocked, completed, and exited activity. Exiting Pi removes
that terminal from the Agents sidebar. The package also gives Pi a Fut skill so
it can inspect and control sessions, panes, terminals, and agents without
changing visual focus.

When Pi's output is a terminal, the extension writes OSC 7501 reports there, so
it works in any terminal that implements the protocol, including over SSH.
Without a terminal, such as in print or RPC mode with redirected output, it
falls back to `fut agent report` inside Fut. Both paths feed the same records
as Pi's native reporting, so running both is harmless. Completed stores done;
an explicit exited report clears its records.
Automatic process-exit cleanup instead preserves done and error results for
inspection.

The integration is a one-way lifecycle adapter. It does not create or change
layouts, submit prompts, read terminal output, or coordinate agents. The CLI
fallback needs the `fut` binary on `PATH`.

See [Agent activity](../agents.md) for the sidebar, notifications, and agent
automation commands.

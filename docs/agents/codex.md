---
layout: default
title: Codex integration
description: Report Codex lifecycle activity to Fut.
permalink: /agents/codex/
---

# Codex integration

> **Legacy fallback, except for completion.** Recent Codex releases emit OSC
> 7501 idle, working, and blocked status natively and need no plugin for those.
> Codex does not report turn completion over OSC 7501, so the `notify` setting
> below is still required for completion attention. Install the plugin only for
> older Codex versions or when Codex runs inside a multiplexer such as tmux that
> does not forward Fut's OSC 7501 support reply.

The Codex plugin requires `python3`. Install Fut first, then add Fut's
marketplace and plugin:

```sh
codex plugin marketplace add mikker/fut
codex plugin add fut@fut-integrations
```

Start Codex, run `/hooks`, and trust the Fut hooks. Codex skips new or changed
non-managed plugin hooks until they are reviewed.

Codex publishes final turn completion through its machine-local `notify`
command. Add this user-level setting to `~/.codex/config.toml`:

```toml
notify = [
  "fut",
  "agent",
  "notify",
  "codex",
]
```

If `notify` is already in use, configure the existing notification program to
dispatch the same JSON argument to `fut agent notify codex`; do not add a
second `notify` key. Restart Codex after changing plugin or notification
configuration.

The integration reports idle, working, blocked, and completed activity. It does
not control prompts, permissions, tools, terminal layout, or worktrees, and it
remains inactive when Codex is running outside Fut.

These CLI fallback reports feed the same records as native OSC 7501, so running
the plugin alongside a native reporter is harmless; completed stores done,
rather than deriving completion from an idle state. Turn ids from older
adapters are ignored. Screen detection remains a lower-priority fallback.

See [Agent activity](../agents.md) for the sidebar, notifications, screen-based
fallback detection, and agent automation commands.

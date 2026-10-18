---
id: lsc-33ad
status: closed
deps: []
links: []
created: 2026-10-08T02:41:43Z
type: task
priority: 3
---
# Clamp requested size for remote daemons without large-screens.v1

A client attached through `fut --machine` to a daemon that did not select `large-screens.v1` still requests its full host size, which that daemon rejects above 50,000 cells (`invalid_size` / invalid handshake).

## Design

When the negotiated capabilities lack large-screens.v1, clamp the hello and resize sizes to LEGACY_MAX_VISIBLE_CELLS (preferring to shrink rows and columns proportionally) and show the constrained-size gutter as for shared sizes.

## Acceptance Criteria

Attaching from a large host terminal to an older remote daemon succeeds with a smaller grid instead of failing.

## Notes

**2026-10-18T19:35:57Z**

Closing as moot: since 0.35 the remote protocol is generation 2 (src/protocol/remote.rs GENERATION) and rejects any other generation at the handshake, and every generation-2 daemon offers large-screens.v1 unconditionally in interactive mode. A current client can never reach a daemon that lacks the capability; such a daemon fails earlier with IncompatibleGeneration. Reopen only if a daemon deliberately stops offering large-screens.v1.

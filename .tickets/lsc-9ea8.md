---
id: lsc-9ea8
status: open
deps: []
links: [lsc-78c8]
created: 2026-10-08T03:38:28Z
type: feature
priority: 2
---
# Send scrolling as a row shift instead of a full screen

When output scrolls, every row of the grid changes, so the row diff exceeds DELTA_ROW_FALLBACK_THRESHOLD and the daemon sends a full screen. The measured plain 508x160 screen is 163 KB and takes 1.9 ms to decode (PERF.md Round 7). Whole-system flood throughput and CPU have not been measured here.

## Design

Detect when the new grid equals the previous one shifted up by N rows within a region (whole screen or a scroll region), and send a delta that shifts retained rows by N and carries only the newly exposed rows. Prefer comparing rows by hash against the previous screen over trusting VT scroll events, so the delta stays correct for any output. Keep the existing row diff as the fallback, and gate any new delta field behind the local protocol (and a remote capability, since generation-1 delta payloads are frozen).

## Acceptance Criteria

perf:e2e flood at 508x160 shows wire bytes and client decode time scaling with new rows rather than grid size; deltas apply identically to full screens in tests; older remote peers keep the current behavior.

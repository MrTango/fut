---
id: lsc-9ea8
status: closed
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

## Notes

**2026-10-18T19:48:02Z**

Baseline (main e835589, release, host load avg 12-30 from parallel agents). Saturating flood (seq 1 100000) sends ~4,500 new lines per 8 ms publication, more than the grid has rows, so every frame is genuinely new content: 21/22 frames full at both sizes (200x50: 20,103 B, 200 us decode; 508x160: 162,668 B, 1.63 ms decode, 3.3 MiB/run). A shift delta cannot help that case. Added perf scenarios where it can: 'scroll' (paced log output, 4 lines per ~12 ms burst) and 'wheel' (60 scrollback wheel events each way at 20 ms). Baseline scroll 508x160: 359/406 full, 56.2 MiB, 1.56 ms avg full decode, 566-780 ms total decode over 6 s; 200x50: 388/405 full, 7.5 MiB, 80 ms decode. Wheel 508x160: 60/60 full each direction, 9.3 MiB, 1.6-2.6 ms decode each; 200x50: 60/60 full, 1.2 MiB. Latency 200x50: typing p50/p95 0.5/0.6 ms, burst 8.3/8.9 ms. All runs' materialized grids matched a fresh full refresh.

**2026-10-18T22:21:29Z**

Done on branch worktree-agent-ad2664414ad274fc2 (rebased on main 45cc751); PERF.md Round 8. Release, back to back with main, load 12-21, all grids matched a fresh refresh. scroll 508x160: main 53 delta/356 full, 56.2 MiB, 559 ms decode -> branch 406/0, 1.6 MiB, 21 ms (4.2 KB, 53 us per frame). wheel 508x160: 60 full, 9.3 MiB each way -> 60 deltas, 0.18 MiB, 2.7 ms. 200x50 scroll 7.5 -> 0.7 MiB, wheel 1.2 -> 0.08 MiB. Saturating flood unchanged (21-22 full frames at both sizes, 3.3 MiB at 508x160): every 8 ms publication carries more new lines than the grid has rows. Latency unchanged (typing 0.5/0.6 ms, burst ~8.6 ms). Review fixes: candidate shifts now proposed by exact anchor-row comparison before hashing (the hash-first version cost saturating floods 0.85 ms/frame at 508x160; now 1.02 vs 0.96 ms rows-only), single-multiply row hash, Kitty images added to the random VT round trip. Real client in a 200x50 PTY via pyte: 19/19 steps (seq, yes|head, less j/k/space/b/G/g, vim ctrl+e/y/f/b, top) matched the daemon screen; less j/k x40 received 32 KiB vs 774 KiB on main. Remote SSH smoke (FUT_REAL_SSH_TARGET) not run; remote gating covered by unit and e2e tests. Local protocol version follows the package minor and bumps at release.

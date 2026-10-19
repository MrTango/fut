---
id: lsc-78c8
status: closed
deps: []
links: [lsc-9ea8]
created: 2026-10-08T02:41:43Z
type: feature
priority: 3
---
# Chunk full screens by row range so grid size is not bound by one frame

Full screens are a single frame capped at 8 MiB, so the visible cell limit (150,000) is derived from the worst-case screen encoding. Ghostty, WezTerm's mux, and tmux have no total-cell cap tied to a transport frame.

## Design

Send full screens (and large deltas) as row-range chunks tied to one revision, completed by an end marker. The client assembles chunks and presents only complete screens, discarding a partial screen when a newer revision begins. The outbound writer must never coalesce away part of a chunk sequence. Copy-mode screens need the same path. Remote peers need a new optional capability. Replace the cell cap with Ghostty-style per-dimension limits plus a memory bound.

## Acceptance Criteria

Grids beyond 150,000 cells attach and render; no frame exceeds MAX_FRAME_LEN; older remote peers keep their bounds. Coordinate with scroll-aware deltas; compare their end-to-end costs before prioritizing either approach.

## Notes

**2026-10-18T22:35:54Z**

Repro on main d994c4a (debug build, disposable daemon under /private/tmp/claude-501/l78, isolated HOME/XDG): a real client attached in a 508x300 PTY prints 'Error: daemon error (invalid_size): terminal dimensions require 152400 visible cells; maximum is 150000' and never attaches; the e2e test kitty_images_are_delivered_once_and_large_screens_are_accepted already pins Resize 1000x200 -> invalid_size. Current bounds: src/domain.rs MAX_VISIBLE_CELLS = 150,000 (TerminalSize::cell_count/validate, and the hard ceiling of validate_within), LEGACY_MAX_VISIBLE_CELLS = 50,000 (remote peers without large-screens.v1, via Capabilities::max_visible_cells and ClientConnection::max_visible_cells on hello/resize); src/protocol.rs MAX_FRAME_LEN = 8 MiB (8,388,608 B) enforced by the length-delimited codec and encode/decode_payload. Derivation: a screen is one frame; a worst-case cell (32-byte grapheme MAX_CELL_CONTENT_BYTES, truecolor fg/bg, all flags, selected, 3-byte hyperlink index) encodes to ~48-50 B, so 150,000 cells plus a maximal 512 KiB hyperlink table (MAX_SCREEN_HYPERLINK_BYTES, 65,536 entries) and 1,024 Kitty references/placements measure 7.83 of 8.39 MB (Round 7; pinned by protocol test maximum_content_selected_styled_snapshot_and_copy_frames_fit). Client has no cell clamp of its own: it sends its host size in hello/resize and the daemon rejects it; the constrained-size gutter only reflects shared-minimum sizing.

**2026-10-19T00:09:00Z**

Implemented on branch worktree-agent-a4947822a19c479d9 (4c05b90, b16f59b, PERF.md Round 9). Screens/copy-mode screens/deltas with >150,000 cells go out as screen_begin / screen_rows (<=150,000 cells each) / screen_end, split in the writer after coalescing so sequences are atomic; client ScreenAssembler presents only complete screens. Limits: u16 per dimension (65,535) plus MAX_GRID_CELLS = 2^20 (40 MiB per copy). Remote capability chunked-screens.v1 (needs large-screens.v1); older peers keep 150,000 / 50,000 and get a cropped corner of larger shared terminals. Measurements (release, main d994c4a vs branch back to back, load 3-14): flood/scroll/wheel/latency at 200x50 and 508x160 unchanged (e.g. 508x160 flood 1/19 frames, 2.9 MiB, decode 29.6 vs 28.9 ms; scroll 1.6 MiB, 19.2 vs 19.3 ms; no chunks, identical largest frames); bench delta produce/apply within 1%. Above the old cap: 508x320 flood 19 chunked screens, 6.0 MiB, largest frame 164 KB; 1000x300 flood largest 302 KB; scroll at both stays delta-only. Worst-case chunk 7.2 MB, widest worst-case row 3.1 MB. Manual pty+pyte: local 508x320 (pane 508x319) 19/19, local 200x50 19/19, remote new client 508x320 6/6, main client as remote peer rejected at 508x320 (same as main daemon) and 19/19 at 200x50. mise run check green.

---
id: os-ypyk
status: closed
deps: []
links: [os-pcxw, os-wtnt, os-g084]
created: 2026-10-16T14:37:27Z
type: task
priority: 2
assignee: Mikkel Malmberg
parent: os-wtnt
---
# Map OSC 9;4 progress into program status root record


## Notes

**2026-10-16T14:51:38Z**

Optionally map incoming OSC 9;4 progress to the root record. Disable mapping once a real OSC 7501 report has arrived, until RIS, so legacy progress cannot erase a blocked message or kind.

**2026-10-18T19:45:43Z**

Repro (isolated daemon, main e835589 debug build): printf OSC 9;4;1;40 BEL and 9;4;3 ST in a Fut /bin/sh terminal leave activity.records empty (revision 0); agent list empty, agent get -> not_an_agent. A 7501 working report in the same terminal is recorded (source osc). Engine finding: the ticket premise holds partially - Fut drops 9;4 only because nothing subscribes. The pinned Ghostty C API DOES surface it: GHOSTTY_TERMINAL_OPT_PROGRESS_REPORT (GhosttyTerminalProgressReportFn, struct {size, state REMOVE/SET/ERROR/INDETERMINATE/PAUSE, progress i8 -1=omitted}) plus GHOSTTY_TERMINAL_OPT_RESET (called after RIS, after progress/program-status callbacks). The safe wrapper simply lacks handlers for both. No hand-rolled OSC parsing is needed; plan is to add on_progress_report and on_reset handlers to the vendored wrapper.

**2026-10-18T20:13:39Z**

Implemented on branch worktree-agent-ae7a74d186f21c7b3. Wrapper: on_progress_report/on_reset handlers (wrapper test proves parser order incl. BEL/ST, byte split, RIS = progress remove -> program-status clear -> reset). Terminal adapter tags receipts Osc vs Progress; 9;4 maps 1->working+pct (none=indeterminate), 3->working indeterminate, 4->working keeping last pct (NOT blocked: paused is not a request for user input and blocked would raise attention), 2->error, 0->weak clear. Any OSC 7501 callback latches program_status_seen; the reset callback (after RIS's own clear) releases it; prompt start and DECSTR do not. RIS's own progress-remove folds into its full clear. Provenance: StatusSource::Detection{agent=progress, rule=osc_9_4} - reuses the detection shape because a new StatusSource enum case would break the frozen remote generation-2 metadata.v2 contract (docs/remote-protocol.md), and gives exactly the weak precedence wanted: suppressed while an explicit (7501 or CLI) lease exists, replaced by explicit reports, never an agent in agent list/get/projections. Coalescer splits batches at protocol boundaries; process exit always travels as Osc so a weak-batch skip can never swallow lifecycle. Model fixes needed for a second weak source: (1) a weak Clear now only retracts records from that same weak source (9;4;0 must not wipe an explicit done/error); (2) Codex detection's not-running cleanup is scoped to codex-origin records (otherwise it would wipe 9;4 records every observation tick); (3) automatic foreground-owner release also covers weak records, so a 9;4 program that exits without 9;4;0 does not leave working forever in shells without OSC 133. No config option: weak, self-retracting and locked out by 7501, so default-on is safe. Known trade-off per ticket spec: after a 7501 program exits, a later 9;4-only program in the same terminal stays ignored until RIS.

**2026-10-18T20:19:28Z**

Verification: CARGO_INCREMENTAL=0 mise run check exit 0 (fmt, clippy -D warnings, 872 unit + 169 e2e + 7 integration, Pi/Claude/Codex/run/wt adapters, 17/17 run smoke). Vendored wrapper --lib 15/15 and vendored fmt --check pass. Manual isolated daemon (/bin/sh, isolated HOME/XDG, own socket): 9;4;1;40 -> working 40 source detection/progress/osc_9_4, agent get not_an_agent; 9;4;4 keeps 40 (no-op revision); 9;4;3 indeterminate; 9;4;1;75; 9;4;2 -> error with attention; 9;4;0 -> empty; 7501 blocked permission+msg replaces root (source osc); later 9;4;1;90, 9;4;0, and 7501 clear + 9;4;1;50 all leave 9;4 ignored; printf ESC c then 9;4;1;30 -> working 30 again; CLI blocked report then 9;4;1;60 ignored. OSC 133;A clears inferred working, inferred error survives prompt until 9;4;0; a foreground child that emits 9;4;1;33 and exits without removing it is cleaned up ~1s after exit.

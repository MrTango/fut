---
id: os-wtnt
status: closed
deps: []
links: [os-pcxw, os-g084, os-ypyk]
created: 2026-10-16T14:37:24Z
type: feature
priority: 2
assignee: Mikkel Malmberg
---
# Make OSC 7501 the primary program status model

Replace agent activity with OSC 7501 records across VT callbacks, resources, protocol, CLI, client and docs; retain report and detection fallbacks.


## Notes

**2026-10-16T14:51:10Z**

Prerequisite implemented (uncommitted): Ghostty pinned to b3dc3d794513c11633e7c3347935693957744e83, bindings regenerated, safe owned on_program_status and semantic-prompt-kind callbacks added. Zig 0.16.0 still works. Upstream C API now uses signed enum ints, terminal_new(cols, rows) plus SCROLLBACK_MAX_BYTES option, MODE get/set structs, and render-state COLORS get. Wrapper adapted; root full suite and mise run check pass. Separate vendored wrapper --lib suite passes 14 tests, including parsed status, control rejection, exact capability reply with BEL/ST, byte-split input, screen/DECSTR independence, RIS clear. No application wiring or activity-model migration has been retained yet. Do not treat OSC 7501 as supported by Fut until the daemon consumes records. No app changelog entry yet.

**2026-10-16T14:51:10Z**

Migration considerations: preserve state transitions around progress coalescing, clears and prompt boundaries (blindly replacing latest-by-id may lose fresh Working needed by prompt --wait); store attention revision per record or retain addressed attention metadata so progress-only Blocked updates do not re-arm acknowledged attention and clearing one child does not hide another child result. One resource revision per applied batch; derive presentation from records rather than keeping legacy state/event caches. Keep done/error across prompt and exit. Revalidate decoded CLI/protocol text and ids, not just OSC parser input. Follow-ups os-g084, os-pcxw, os-ypyk are linked.

**2026-10-16T14:53:54Z**

Final prerequisite validation: mise run check passed (731 unit, 154 e2e, 7 other Rust integration tests; root fmt/clippy -D warnings, auxiliary integration tests and run smoke all passed). CARGO_TARGET_DIR="$PWD/target" mise exec -- cargo test --manifest-path vendor/libghostty-vt/Cargo.toml --lib passed all 14 wrapper tests. mise exec -- cargo fmt --manifest-path vendor/libghostty-vt/Cargo.toml --check and git diff --check passed. Extra standalone vendored-crate clippy --lib -- -D warnings is not clean: it reports the wrapper crate-wide pedantic lint backlog (123 diagnostics on the first run, including two needless-borrows in the new handlers that were subsequently fixed); root required clippy is clean. No commits created.

**2026-10-16T14:54:37Z**

One further model distinction to resolve during migration: source is provenance, not an ownership lease. Done/Error records survive exit and must remain inspectable via agent get/list, but explicit-source precedence over screen detection must end when the reporting program exits. Avoid using only is_reporting() as the detection ownership guard; retain provenance for persistent records while releasing daemon-side ownership on the lifetime boundary. Resolve app inheritance from the current ancestor records, not by copying app into stored children, so parent updates and whole-record replacement remain correct.

**2026-10-16T14:55:57Z**

Remote-contract investigation: docs/remote-protocol.md Frozen semantics and evolution freezes transitive resource/domain payloads at f77344c, not just envelope field names. Replacing PaneSnapshot.activity cannot be shipped under existing metadata.v1 solely with a local protocol bump or documentation note. Recommended no-shim path is remote generation 2 with a correspondingly versioned metadata contract and updated federation fixtures. Alternative is a separately negotiated capability payload/adapter while preserving generation-1 snapshots, but that retains a compatibility mapping. Neither remote contract nor local protocol version has been changed by the prerequisite patch.

**2026-10-16T14:56:11Z**

Performance commands completed successfully: mise exec -- cargo bench --bench render -- --save-baseline before; mise run perf:bench -- -- --baseline before. Important limitation: baseline before was saved AFTER upgrading the engine, and ran amid concurrent compilation. Comparison reports contradictory load-sensitive changes (plain_200x50 +70.2%, styled_200x50 +35.8%, tui_frame_200x50 -57.6%, dense_anim_200x50 -61.1%). These are not a valid pre-upgrade A/B or evidence of improvement/regression attributable to the patch. Raw logs: /tmp/fut-osc-before.log and /tmp/fut-osc-after.log. A clean pre-upgrade performance comparison and perf:e2e remain outstanding.

**2026-10-16T18:35:19Z**

Implementation phase coordinated across four Sol agents (core/resources/protocol, terminal/daemon, CLI/e2e, client). Parent updated user docs and generation-2 bridge coverage. Shared record migration now compiles: cargo check --locked --all-targets --all-features and cargo build --locked passed. Isolated real-daemon/PTY smoke verified native 40%+message, real client sidebar and keyboard-opened Agents dialog, blocked attention, replacement/subtree clear, done unread, prompt cleanup, DECSTR/alternate-screen independence, and exact BEL/ST query replies. Manual testing found a genuine duplicate-report no-op bug (activity revision 13 -> 14 due to changed timestamp); not considering feature complete until reviewed/fixed and final checks pass. Preserved the pre-record-refactor compiled render benchmark at target/osc7501-render-before (SHA256 6cb3c7897c8d703b03c196916488d369cdd8d9a1c62b90306d881aac753b99be) for valid same-Ghostty before/after measurements; earlier pre-upgrade benchmark remains invalid. No commits.

**2026-10-16T19:01:18Z**

First independent Sol review round completed (core/protocol, UI/CLI/safety, runtime/lifecycle). Verified blockers: stale runtime dedup after CLI clear, lossy overflow of semantic operations/journal, old persistent results masking fresh prompt/wait turns, lost agent_exited error, and within-terminal unread priority; also progress/glyph coupling and human CLI bidi safety. Focused real-daemon reproductions saved by reviewers under /tmp/fut-review-{shadow,wait,agent-exit}-*. Four scoped Sol fix agents now address core no-ops/helpers, lossless bounded terminal transport, daemon lifecycle correlation/takeover, and UI/CLI/E2E regressions. Final suite and fresh reviews remain required; no commits.

**2026-10-16T19:50:27Z**

Final independent reviews caught shutdown ordering, wait correlation, blocked-child progress, and stale federated alert projection issues. Fixes are landing with regressions. Replacing the remaining terminal-wide automatic CLI PID cleanup with authoritative record-scoped lifecycle metadata so an old CLI process exit cannot clear a native replacement or unrelated explicit work. Full check, enhanced real-PTY/client smoke, and same-Ghostty performance comparison remain pending; no completion claim yet.

**2026-10-16T20:27:16Z**

Core migration complete and independently re-reviewed clean: Sol sa-22 core/daemon, sa-24 terminal/runtime, sa-25 UI/CLI/docs. Legacy AgentActivity/AgentState/AgentReportEvent and daemon ownership caches are gone. Authoritative resources own record provenance, explicit leases, receipt LRU and record-scoped automatic PID lifecycle. Native OSC 7501, CLI reports and weak detection share ProgramStatus. Package/local minor 0.32.0; remote generation 2/metadata.v2 explicitly rejects old peers. Ghostty b3dc3d794513c11633e7c3347935693957744e83, Zig 0.16.0. No commits or additional worktrees.

**2026-10-16T20:27:16Z**

Final verification PASS with CARGO_INCREMENTAL=0: mise run check (fmt, root all-target/all-feature clippy -D warnings, 785 unit + 162 e2e + 7 other Rust integration tests; Pi 5, Codex 3, run 7, Claude and wt adapters, all 17 run smoke checks). Fresh debug build PASS. Vendored wrapper 14/14 and vendored formatting PASS. Enhanced isolated real daemon/PTY/attached-client smoke 15/15 PASS; evidence /tmp/fosc-ggps4gp8/evidence.json and /tmp/fut-osc-manual-final.log. Same-write native Working/Done both revision 33 beyond prompt barrier 32 settled before physical PTY exit. Exact BEL/ST capability replies, real tab/workspace progress tokens and Agents dialog, attention, replacement/clear, prompt/DECSTR/alternate-screen/RIS, no-op/redelivery, quiet progress coalescing and exact-PID cleanup all verified. Final diff/fmt checks clean; owned smoke processes stopped.

**2026-10-16T20:27:16Z**

Performance: valid preserved pre-record-refactor vs final binaries at SAME Ghostty pin; feed sum +2.07%, wire -2.42%, total +1.03% (descriptive endpoint range -0.21%..+2.31%). No material >10% regression demonstrated; dense-feed low-single-digit changes remain uncertain under documented host-load drift and A-B-A outliers. This is not a pre-Ghostty-upgrade comparison; older engine-upgrade measurements remain invalid. Full report target/osc7501-performance-final.md. All five fresh-release end-to-end scenarios passed: flood, styled, dense, partial, latency (typing p95 2.1ms, burst p95 9.2ms), /tmp/fut-osc-perf-e2e-final.log. Harness deviation: isolated HOME correctly rejected this repo project recipe, so unchanged scenario logic ran from target/osc7501-perf-e2e-final.sh with runtime outside Git rather than trusting/executing recipes; production runner untouched. Standalone vendored pedantic-clippy backlog remains outside required clean root gate. Deferred os-g084 native emitters, os-pcxw outgoing rollup and os-ypyk OSC 9;4 remain open.

**2026-10-16T21:03:46Z**

Follow-up requested: rebased uncommitted work onto main b4baad1 and integrating main PTY writer/backlog, Kitty images/large screens, shell choice and configurable agent rows. Package/local minor now 0.35.0 because main is 0.34. Refactor removes superseded handrolled OSC parser, duplicate validation and unnecessary payload cloning; runtime owns receipt/PID-aware coalescing. Prior test/perf results apply to pre-rebase snapshot only. New verification and requested Fable/Claude Code review pending; safety stash retained, no feature commits.

**2026-10-16T21:27:39Z**

Rebase/refactor first full gate passed on main b4baad1: 858 unit, 168 e2e, 7 integration plus aux. Requested Fable via Claude Code (sa-32, claude-fable-5-1) found 4 actionable items: unclaimed popup status channel can deadlock; queued native PID lost after reporter exit; external report handlers accept runtime-only events/provenance; old Codex live test schema. Fixed with explicit popup stream opt-out and common close-time discard of unclaimed receivers (active consumers untouched), receipt-owned PID attribution, strict external Report/Clear + Report provenance validation, and 6 ported self-contained live adapter tests. New regressions cover saturated unclaimed close, 40 popup prompt batches with responsive input/resize, receipt metadata, and rejected calls preserving state. Fable follow-up sa-35 and full final gate/build/manual/live reruns pending. Earlier manual replacement fixture used the same native/CLI process and now correctly auto-cleans under main semantics; changed fixture to unrelated background CLI owner plus parent-native replacement.

**2026-10-16T21:48:08Z**

Rebase/refactor COMPLETE on main b4baad1, package/local protocol now 0.35.0. Fable via Claude Code reviewed full tree (sa-32), all four findings fixed; sa-35 follow-up exposed close-before-watcher expect race, fixed with optional RX plus initial/current lifecycle sampling and final ordered drain; sa-36 final static verdict CLEAN. Native foreground PID identity remains deliberately best-effort receipt metadata (informational trade-off accepted, not actual PTY writer identity). Final frozen-tree bt-39 exit0: CARGO_INCREMENTAL=0 mise run check (fmt/clippy, 863 unit +168 e2e +7 integration =1038 Rust tests, Pi5/Codex3/run7/Claude/wt/17 run smoke), fresh cargo build --locked, 16/16 real daemon/PTY/client manual checks, 6/6 self-contained Codex live adapter tests. Logs /tmp/fut-osc-rebase-complete-{check,build,manual,codex-live}.log; manual evidence /tmp/fosc-_6hyv6nx/evidence.json. Earlier final gate had one remote-switch e2e 8s timeout; three focused repeats then final full suite passed WITHOUT source/test changes. Read-only diagnosis sa-37 confirms handoff/query/readiness logic matches main; outer test8s is below production handshake15s/staging10s budget, exact delayed phase unobservable. No scope expansion or retry/sleep workaround. Final git diff --check, no staged/unmerged files, no owned smoke processes. All feature changes remain uncommitted; original rebase safety stash 7509d7a retained.

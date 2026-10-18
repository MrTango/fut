---
id: os-pcxw
status: open
deps: []
links: [os-wtnt, os-g084, os-ypyk]
created: 2026-10-16T14:37:27Z
type: task
priority: 2
assignee: Mikkel Malmberg
parent: os-wtnt
---
# Forward Fut status rollup to host terminal using OSC 7501


## Notes

**2026-10-16T14:51:38Z**

After core support, send a rollup to the host terminal using app=fut and one child record id per Fut terminal. Define clear/detach ownership, query negotiation and notification-rate limits.

**2026-10-18T19:46:31Z**

Design (resolved before coding). Context: client writes to host via stdout between frames (BEL precedent for alerts.signal_outer_terminal); vendored crossterm surfaces every OSC reply as Event::TerminalResponse (HostColors uses it). Spec 0.4 + Ghostty parser: id segments [A-Za-z0-9_.+-]{1,32}, <=128 bytes, depth 8; app <=32; title <=192, msg <=2048 decoded; seq <=4096. Records belong to the terminal, alternate screen/DECSTR do not affect them, RIS clears; process exit / OSC 133 A drop working+blocked only.
1 Negotiation: new client-local [alerts] outer_terminal_status = auto|always|never (default auto). auto sends OSC 7501;? ST once at startup and emits only after a reply starting OSC 7501;? (any time, no timeout needed: silence = never emit, so no DA1 trick). Reason: unknown OSCs are usually ignored, but tmux without passthrough swallows them and some terminals echo/mangle; probing is ~10 bytes. always = for passthrough setups that cannot answer; never = off.
2 Shape: root app=fut, state = highest of children by Fut summary priority (blocked>error>working>done), kind+msg from the winning child (ties: most recently updated), progress only when exactly one child is working/blocked (same rule as tab/workspace rollups). One child per live non-closing terminal whose summary state is not idle, id = compact terminal id (23 chars, f+base64url, valid segment). Child: state/kind/progress from the terminal summary, app from the terminal's program app when it is a valid name (else inherit fut), title = location session/workspace/tab (machine label first when federated), msg = inner title and message joined. Idle omitted. Done forwarded regardless of the inner read state: the inner client acks attention whenever it draws the focused terminal, even if its own host pane is hidden, so the host owns read state for its view. Detection-derived status included (it drives tab indicators too). Capped at 64 children by priority. The terminal hosting the client itself (FUT_TERMINAL_ID) is excluded to avoid self-feedback when FUT_ALLOW_NESTED attaches a client to its own daemon.
3 Ownership: while attached the client owns the host terminal's status (it is the foreground program). First non-empty emission starts with state=clear (also sweeps stale records from a SIGKILLed predecessor); an empty rollup sends state=clear, never done; exit/detach/error return clears via Drop before the guard leaves the alternate screen. Machine handoff and remote reconnects keep the same client process and ownership; records refresh from the next snapshot. SIGKILL cannot clear: host prompt/exit drop working/blocked but done/error remain until the next client or a manual clear. Alternate screen irrelevant per spec.
4 Rate: diff-based (clears for removed ids, reports for changed ones, root last), sent only when the derived set changes; leading edge then at most one batch per 100 ms; progress-only changes at most every 500 ms. Bounded at 65 records <=4 KiB each per batch.
5 Remote: --machine attaches run_loop with remote resources and the local client emits; federated registry endpoints that are current contribute too. No protocol change.

**2026-10-18T20:04:36Z**

Gap reproduced on main e835589 with real isolated daemons: inner client attached inside an outer Fut terminal (FUT_ALLOW_NESTED=1), inner agent report working/native OSC blocked -> outer agent get showed no records, and the client sent no OSC 7501 query. Implemented in src/client/host_status.rs (derive/diff/coalesce, Drop clears) wired into run_loop; reply consumed before HostColors. Docs: docs/agents.md 'Forward status to the outer terminal', docs/configuration.md alerts.outer_terminal_status, docs/decisions/outer-terminal-status.md.

**2026-10-18T20:11:22Z**

Verified: mise run check exit 0 (875 unit incl. 9 host_status, 170 e2e incl. 2 new, 7 upgrade, aux adapters, 17 run smoke). Manual nested check (two isolated daemons, inner client attached in an outer Fut terminal via terminal run, isolated HOME/runtime): 15/15 pass. Outer gets root app=fut + child per inner terminal (progress 40, app demo, title location), native inner OSC blocked -> outer root blocked/permission, 400-update progress flood -> 1-2 outer revisions, Ctrl-b d clears outer, SIGKILL leaves done (documented), next client reclaims, plain PTY host without reply gets only the query. Ticket left open for review/merge.

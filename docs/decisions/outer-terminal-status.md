---
layout: default
title: Decision record — forwarding status to the outer terminal
description: How a Fut client reports its program-status rollup to the terminal it runs in.
permalink: /decisions/outer-terminal-status/
---

# Forwarding status to the outer terminal

- Status: **accepted**
- Date: 2026-10-18
- Scope: ticket `os-pcxw`
- Protocol: Program Status Protocol revision 0.4 (OSC 7501)

## Decision

An attached client reports its rollup to its host terminal as OSC 7501: a root
record with `app=fut` and one child per non-idle Fut terminal. User-facing
behavior is documented in
[Program status and agents](../../agents/#forward-status-to-the-outer-terminal).
The choices and why:

1. **Probe first.** In the default `auto` mode the client sends `OSC 7501 ; ?`
   once and reports nothing until a reply starting with `OSC 7501 ; ?` arrives.
   The specification lets programs report without asking because terminals
   must ignore unknown OSCs, but a long-running client can afford the
   10-byte probe and gains a lot from it: tmux without passthrough, and
   terminals that echo or mangle unknown sequences, never see reports. There is
   no timeout. Silence means never, and a late reply still enables forwarding,
   so the primary-device-attributes trick from the specification is not
   needed. Replies arrive through the same Crossterm `TerminalResponse` path as
   host color replies. `always` exists for setups whose terminal supports the
   protocol but whose reply is lost in between; `never` turns it off.
2. **Shape.** Child ids are compact terminal IDs: 23 characters from the
   protocol's name alphabet, stable for the terminal's life and unique across
   machines. Children follow each terminal's own summary record; the root takes
   the strongest child with the same precedence as Fut's summary (blocked,
   error, working, done), and progress only when exactly one child is active,
   as tab and workspace rollups do. Locations go in `title`, which the
   specification reserves for telling records apart; the program's text goes
   in `msg`. Idle terminals are omitted, but done is forwarded regardless of
   Fut's own read state. A client acknowledges attention whenever it draws the
   focused terminal, even when its own host pane is hidden, so only the host
   knows whether the user has seen the result. Detection-derived status is
   included because it already drives tab indicators. The client's own terminal
   is excluded, which prevents a feedback loop when `FUT_ALLOW_NESTED` attaches
   a client to its own daemon.
3. **Ownership.** The client is the host terminal's foreground program, so it
   owns the root record and, since the protocol cannot clear the root alone,
   the whole record set. Its first report is preceded by `state=clear`, which
   also sweeps records a killed client left behind. An empty rollup clears
   rather than reporting done, so the host does not raise attention for
   "nothing is happening". Exit, detach, and errors clear from the client's
   `Drop`, before the terminal guard leaves the alternate screen. `SIGKILL`
   cannot clear. The specification has the host drop working and blocked
   records when the process exits; done and error persist until the next
   client or a manual clear. Screen switches and soft resets do not touch
   records, and the client never sends a full reset.
4. **Rate.** The client diffs against what it last sent and writes nothing for
   unchanged sets. A change goes out immediately after an idle period, then at
   most every 100 ms; progress-only changes wait 500 ms. With at most 64
   children of at most 4 KiB each, a batch is bounded. The inner daemon already
   coalesces progress floods: 400 native progress updates produced a single
   host batch in manual testing.
5. **Remote.** `fut --machine` runs the same client loop over remote resources
   and federated endpoints contribute their current snapshots, so remote
   terminals are forwarded with no remote-protocol change.

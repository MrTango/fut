---
id: fut-36i4
status: closed
deps: []
links: []
created: 2026-10-20T08:34:09Z
type: feature
priority: 2
assignee: Mikkel Malmberg
---
# Negotiate stable local protocol independently of Fut releases

Introduce a stable local generation and capability handshake, preserving legacy exact-match hello for current old clients and remote permission isolation. Migrate local CLI, UI, federation, autostart, doctor. Freeze local.v1 baseline and test version skew and rejection without daemon replacement.


## Notes

**2026-10-20T08:46:07Z**

Implemented generation-1 local_hello/local_welcome with required local.v1 frozen baseline and exhaustive method allowlists. CLI/UI/federation/autostart/doctor use package-independent negotiation. Legacy hello remains exact-match; no implicit normal downgrade or daemon replacement. Explicit shutdown and unsafe navigator legacy recovery remain. Remote allowlists stay separate. Generic adapters and per-feature local capabilities are future evolution, not implemented in this first step.

**2026-10-20T08:52:22Z**

Validation: 943 library tests, 177 e2e/SSH integration tests and 7 upgrade tests passed. cargo check --all-targets passed. Fixed review findings: graceful legacy shutdown handles its uncorrelated invalid-handshake response, normal requests never downgrade, and daemon-hosted extension catalogs no longer gate UI rendering on client package version while disk package validation remains strict. Final test rerun uses CARGO_INCREMENTAL=0 to avoid a transient incremental linker-cache failure.

**2026-10-20T12:03:22Z**

Manual/review follow-up: compiled real 0.38/0.39 binaries in isolated scratch manifest and verified both directions with real PTY UI attach/input/detach, CLI mutations/automation/doctor, raw malformed negotiation, remote method rejection and graceful shutdown (18 scenario groups). Claude Code Fable review found local extension commands/hooks must still satisfy client package requirements even when catalog rendering is permitted; fixing that plus migrated integration harness, consistent legacy upgrade diagnostics and descriptive invalid_size errors.

**2026-10-20T12:24:36Z**

Claude Code Fable review completed and fixes verified: client-incompatible extension commands are refused at shared execution context while hooks are stripped and presentation/config remains valid; default e2e harness now negotiates with representative local.v1 fixtures; legacy rejection classification/guidance shared across callers while preserving typed errors and safe shutdown; invalid_size remains operational/descriptive; outbound gate violations logged without terminal contents. Manual testing additionally moved background config error causes before long command titles so the incompatibility remains visible in the toast. Final validation: 948 lib + 182 integration + 7 upgrade = 1,137 tests passed; all-target check and formatting/diff checks passed. Manual final tree: 24 protocol/CLI/real-PTY/released-daemon recovery scenario groups plus two real narrow-version extension execution controls passed using isolated sockets; all owned daemons stopped. Reports: /tmp/fut-protocol-manual.nftzQK/.

**2026-10-20T12:46:25Z**

Pre-ship refactor: inline the single-use control hello wrapper and remove its unused server-version return; flatten explicit legacy navigator retries; remove the constant legacy CLI version parameter; centralize readiness failure handling; compute extension availability per extension without an intermediate map. Preserve capability gates and bounded explicit recovery. Ship verification uses the complete mise run check gate.

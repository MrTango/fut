---
id: fut-ntju
status: open
deps: []
links: []
created: 2026-10-18T18:11:30Z
type: bug
priority: 2
assignee: Mikkel Malmberg
tags: [extensions, tests]
---
# Run extension smoke test is flaky under mise run check

extensions/run/test/smoke failed twice in a row during 'mise run release' (2026-10-18, 0.37) at 'restarted runner never became ready' (the 5s wait_for after the second w-ready restart), yet passed three times standalone and once via 'mise run test:run-smoke' alone. One standalone run with per-step timestamps also stalled for ~18 minutes inside 'run w-stale-owner {} stop' before continuing and passing. Isolated runctl restart/stop under both /bin/sh and zsh completes in 1-2s, and zsh -lic startup is ~0.25s under mise, so neither shell startup nor plain termination explains it. With SHELL=/bin/sh the test fails earlier at 'split readiness marker was not matched'. Reproduce via the full 'mise run check' sequence; when it stalls, dump ps/lsof/sample of the runctl processes. 0.37 was tagged by hand after every other check passed; CI ran the smoke test again on the release workflow.


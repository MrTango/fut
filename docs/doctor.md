---
layout: default
title: Diagnostics
description: Read-only checks for Fut's environment and configuration.
permalink: /doctor/
---

# Diagnostics

> **TL;DR:** Run `fut doctor`. It safely checks configuration, terminal
> capabilities, runtime permissions, daemon and saved-machine compatibility,
> and icons without
> starting or changing anything.

`fut doctor` performs a bounded, read-only diagnosis of the client environment:

```sh
fut doctor
fut --json doctor
fut --socket /path/to/fut.sock doctor
```

It does not start or stop a daemon, create or repair directories, remove stale sockets, open a project, spawn a terminal, enter raw mode, or change terminal state.

## Checks

Doctor reports:

- the resolved configuration source, file presence, parsing, and validation;
- `TERM`, color hints, TTY attachment, and the terminal program hint;
- whether the selected locale declares UTF-8;
- runtime-directory ownership and private permissions;
- socket type, ownership, and permissions;
- a bounded Fut protocol handshake when a safe socket exists;
- the active daemon extension generation, fingerprint, package count, manifest
  metadata, canonical roots, and configuration provenance;
- whether the `ssh` executable identifies itself as OpenSSH;
- saved-machine catalog validity and enabled/disabled profile summaries;
- a bounded, non-interactive compatibility handshake to each enabled saved
  machine, with at most four probes in flight;
- the configured icon preset and a visual glyph probe.

Remote probes use `BatchMode=yes` and strict host-key checking. They do not
prompt, accept host keys, edit SSH configuration, write the machine catalog,
start or stop a daemon, or lease a terminal. A profile can therefore report an
authentication, host-key, installation, timeout, or protocol error that must be
repaired with ordinary OpenSSH and Fut commands outside doctor. Every probe
closes and reaps its SSH process before the report completes.

Local clients and daemons negotiate a stable protocol generation and required
capabilities independently of their Fut package versions. Doctor reports the
selected generation, capabilities and package versions when it can safely reach
a daemon. A daemon predating this handshake needs one explicit shutdown and
restart; doctor never performs that upgrade itself. See the
[local protocol contract](local-protocol.md).

Configuration validation and active extension reporting are independent. If a
changed package on disk is incompatible or malformed, the config check explains
why that candidate is rejected while the extensions check still identifies the
daemon's last-known-good active generation. Doctor only reads manifests and the
daemon catalog; it never executes extension hooks or commands.

## Nerd Fonts

Terminal processes have no reliable cross-terminal API for discovering the active font. Doctor therefore reports the active font as unknown and asks for visual verification. It does not infer support from the terminal program or claim that an installed font is selected.

Enabling `preset = "nerd_font"` makes the icon check a warning until visually verified; it does not prevent Fut from starting.

## Exit status

- `0`: the report contains no errors; informational checks and warnings are allowed.
- `1`: one or more checks are errors, such as malformed configuration, `TERM=dumb`, insecure runtime state, or an incompatible endpoint.

Diagnosed errors are part of the complete report rather than a secondary command failure.

## JSON

`fut --json doctor` writes one compact success-envelope-shaped report to stdout, even when the report's status is `error` and the process exits 1:

```json
{"version":1,"command":"doctor","result":{"status":"warning","checks":[{"id":"config","status":"ok","summary":"valid defaults; no configuration file"}]}}
```

Check statuses are `ok`, `info`, `warning`, and `error`. Details include
structured paths, environment hints, protocol versions, active extension
package metadata and provenance, icon glyphs, and the explicit
`active_font: "unknown"` limitation where applicable.

## Related

- [Configuration](configuration.md)
- [Presentation tokens](tokens.md)

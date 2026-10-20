# Project notes

This project uses a CLI ticket system for task management. Run `tk help` when you need to use it.

## Protocol compatibility

- Local and remote negotiated contracts are independent of package versions.
- Read `docs/local-protocol.md` and `docs/remote-protocol.md` before changing wire messages or their transitive domain types. Preserve frozen payloads and semantics; new behavior needs a negotiated capability, a tested adapter, or a new generation.
- Never grant local-only operations through the remote capability allowlist.

## Changelog

- `CHANGELOG.md` covers user-visible changes to the Fut app itself. Omit project website, installer, distribution, CI, release-process, and other repository-maintenance changes unless they alter the installed app's behavior.
- Keep entries brief and update `Unreleased` as user-facing tasks finish.
- Before releasing, review `Unreleased`, prune anything outside that scope, consolidate related entries, move them under the new version, and start a fresh `Unreleased` section.

## Release

- From a clean `main` that is not behind or diverged from `origin/main`, run `mise run release`. The script pushes local commits when needed.
- It tests, advances the `0.x` version, tags and pushes. GitHub builds both macOS binaries, publishes the release, and updates `mikker/homebrew-tap`.

# Releasing Polar Stream

The release workflow builds on native GitHub-hosted x64 and ARM64 runners. It
does not cross-compile GUI packages. liblsl 1.17.7 is downloaded from its
upstream release and accepted only when its pinned SHA-256 checksum matches.

## Publish a version

1. Update the version in the workspace and `apps/polar-stream/tauri.conf.json`.
2. Run `cargo test --workspace` and strict Clippy locally.
3. Push `main`, then create and push a matching version tag such as `v0.1.0`.
4. Watch **Release native packages** in GitHub Actions.

Each matrix job builds its native installer, creates a real LSL outlet using the
bundled runtime, and launches the packaged application binary. The GitHub
release remains a draft unless all nine required installer classes exist. This
prevents `latest` from ever pointing at a partial platform release.

## Signing

The initial private preview packages are unsigned (macOS uses an ad-hoc
signature). Before public distribution, configure an Apple Developer ID with
notarization and a trusted Windows Authenticode certificate. Do not store
certificate material in the repository; use encrypted GitHub Actions secrets.

Linux package signing is optional. Release assets are also covered by GitHub's
authenticated transport and release metadata.

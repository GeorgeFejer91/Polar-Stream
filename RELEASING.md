# Releasing Polar Stream

The release workflow builds on native GitHub-hosted x64 and ARM64 runners. It
does not cross-compile GUI packages. liblsl 1.17.7 is downloaded from its
upstream release and accepted only when its pinned SHA-256 checksum matches.
The official LabRecorder 1.17.0 is pinned to upstream release v1.17.1. Published
x64/universal archives are checksum-verified; native ARM64 jobs build its exact
source and liblsl commits. The staged recorder includes its Qt/liblsl runtime
and the Polar Stream profile that disables remote control. Its reviewed Qt
notice and checksum-pinned LGPL/GPL license texts are required bundle files.

## Publish a version

1. Update the version in the workspace and `apps/polar-stream/tauri.conf.json`.
2. Run `cargo test --workspace` and strict Clippy locally.
3. Push `main`, then create and push a matching version tag such as `v0.1.0`.
4. Watch **Release native packages** in GitHub Actions.

Each read-only matrix job builds its native installers with the exact Tauri CLI
in `package-lock.json`, creates a real LSL outlet using the bundled runtime, and
smoke-tests Polar Stream plus LabRecorder from the staged package itself
(AppImage and DEB on Linux, MSI payload on Windows, DMG on macOS). The macOS
gate also verifies that Polar Stream, the packaged liblsl runtime, LabRecorder,
and LabRecorder's liblsl framework each contain native Apple Silicon and Intel
slices. The mounted DMG is launched on both an Apple Silicon runner and a
separate Intel runner before publication. The matrix then
uploads workflow artifacts without a repository
write token. Only the final publisher job has `contents: write`; it downloads
the complete package set, verifies all nine required installer classes,
generates `SHA256SUMS.txt`, creates or updates a draft release, verifies the
uploaded assets, and then publishes. This prevents `latest` from ever pointing
at a partial platform release.

All reusable GitHub Actions are pinned to full commit SHAs. Review dependency
updates deliberately rather than replacing these pins with floating major tags.

The private repository's latest Release is its authenticated download page.
The branded static page under `download/` is optional because GitHub only
serves Pages from private repositories on plans that include that feature.

## Signing

The initial private preview packages are unsigned (macOS uses an ad-hoc
signature). Before public distribution, configure an Apple Developer ID with
notarization and a trusted Windows Authenticode certificate. Do not store
certificate material in the repository; use encrypted GitHub Actions secrets.

Linux package signing is optional. Release assets are also covered by GitHub's
authenticated transport and release metadata.

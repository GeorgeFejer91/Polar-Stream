# Releasing Polar Stream

The release workflow builds Windows and Linux packages on matching native
GitHub-hosted x64 and ARM64 runners. The universal macOS target is composed on
an Apple Silicon runner and the exact DMG is then launch-tested on both Apple
Silicon and Intel runners. liblsl 1.17.7 is downloaded from its
upstream release and accepted only when its pinned SHA-256 checksum matches.
The official LabRecorder 1.17.0 is pinned to upstream release v1.17.1. Published
Windows x64 and macOS universal archives are checksum-verified; both native
Linux jobs and Windows ARM64 build its exact source and liblsl commits. Building
LabRecorder against the native Ubuntu runner's Qt version prevents linuxdeploy
from coalescing two incompatible Qt ABI versions inside one AppImage. The staged
recorder includes its Qt/liblsl runtime and the Polar Stream profile that
disables remote control. Its reviewed Qt notice and checksum-pinned LGPL/GPL
license texts are required bundle files.

The bundled macOS recorder/runtime requires macOS 14 or later, so the app and
download copy use 14.0 as the minimum supported system.

## Publish a version

1. Set the exact intended tag version in Cargo/Cargo.lock, npm/package-lock, and
   `apps/polar-stream/tauri.conf.json`. For a release candidate, every surface
   must contain the full prerelease version such as `0.6.0-rc.1`; for stable,
   every surface must contain `0.6.0`.
2. Run the full workspace, frontend, package-script, and strict Clippy gates locally.
3. Push the reviewed commit to `main`. The release workflow rejects a tag whose
   commit is not on `origin/main`.
4. In repository settings, keep the `release` environment protected by at
   least one required reviewer. The workflow fails before package builds when
   that protection is absent, and the publisher job enters that environment
   before it receives `contents: write`.
5. Create a fresh tag that exactly matches step 1, such as `v0.6.0-rc.1`; never
   move or reuse a published tag. A later stable `v0.6.0` is a separate reviewed
   version commit and package build, not a relabeling of release-candidate assets.
6. Watch **Release native packages** in GitHub Actions. Tags containing a
   hyphen publish as prereleases and stay out of GitHub's latest-stable route;
   a plain `vX.Y.Z` tag becomes the stable latest release after approval.

Protect `main` with the required CI checks and disallow force-pushes in the
repository ruleset. The workflow's ancestry check rejects an off-main tag, but
it is not a substitute for branch review policy.

Each read-only matrix job builds its native installers with the exact Tauri CLI
in `package-lock.json`, creates a real LSL outlet using the bundled runtime, and
smoke-tests Polar Stream plus LabRecorder from the staged package itself
(AppImage and an installed DEB on Linux, extracted MSI payload plus an
install/uninstall cycle for NSIS on Windows, and mounted DMG on macOS). The macOS
gate also verifies that Polar Stream, the packaged liblsl runtime, LabRecorder,
and LabRecorder's liblsl framework each contain native Apple Silicon and Intel
slices, and that the packaged app declares the documented macOS 14.0 minimum.
The mounted DMG is launched on both an Apple Silicon runner and a separate Intel
runner before publication. The matrix then
uploads workflow artifacts without a repository
write token. Only the final publisher job has `contents: write`; it downloads
the complete package set, verifies all nine required installer classes,
generates `SHA256SUMS.txt`, creates or updates a draft release, verifies the
uploaded assets, and then publishes. This prevents `latest` from ever pointing
at a partial platform release.

The Linux Tauri bundling step exposes the staged LabRecorder `lib` directory
through `LD_LIBRARY_PATH`. `linuxdeploy` inspects every packaged ELF resource,
so it must be able to resolve LabRecorder's pinned liblsl and Qt runtime while
constructing the AppImage even though Polar Stream launches LabRecorder with an
isolated runtime environment later. Keep verbose Tauri logging enabled for this
step so a failed child bundler remains diagnosable from the Actions log.

All reusable GitHub Actions are pinned to full commit SHAs. Review dependency
updates deliberately rather than replacing these pins with floating major tags.

The public repository's **All releases** page is the canonical route for release
candidates and stable packages. GitHub's `releases/latest` and
`releases/latest/download/...` routes intentionally resolve only the current
stable release. GitHub Pages hosts the browser demo and a landing page with both
choices, but never a second copy of installer assets.

## Signing

The current public research-preview packages are unsigned (macOS uses an ad-hoc
signature). Before describing them as trusted production installers, configure
an Apple Developer ID with notarization and a trusted Windows Authenticode
certificate. Do not store
certificate material in the repository; use encrypted GitHub Actions secrets.

Linux package signing is optional. Release assets are also covered by GitHub's
authenticated transport and release metadata.

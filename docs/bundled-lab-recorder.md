# Bundled LabRecorder

Polar Stream native packages include the official LabRecorder as a separate
desktop application. No second download is needed. Polar Stream creates and
keeps the selected native LSL outlets live; **Open Lab Recorder** starts the
bundled recorder, where the user chooses any discoverable Polar Stream or other
LSL streams and records them together in one XDF file.

## Recording workflow

1. Connect the required sensors in Polar Stream. Native raw outputs enable LSL
   automatically; added metrics and formulas create their own named outlets.
2. Select **Open Lab Recorder** under Output transports.
3. In LabRecorder, use **Update** if a newly created stream is not listed yet.
4. Check the streams to include, choose the study root and file name, then
   select **Start**.
5. Select **Stop** before closing LabRecorder. Keep Polar Stream and every
   other source application open until recording has stopped.

The recorder process is independent from the Tauri WebView and does not enter
Polar Stream's acquisition, decoding, publication, or display queues. It reads
the already-published LSL outlets like any other LSL consumer.

GitHub Pages retains the same button for interface parity, but a browser cannot
launch a packaged application or implement native LSL discovery. A browser
attempt fails closed and links to the latest installed-app release.

## Artifact and security contract

- LabRecorder project version `1.17.0` is pinned to upstream release `v1.17.1`
  and commit `8419550553e4336dd46378a9a871b3065a70b895`.
- Published x64 Windows/Linux and universal macOS archives are accepted only
  after exact SHA-256 verification. ARM64 Windows/Linux packages build the same
  immutable source revision with liblsl commit
  `03316f61137485450e7a43aea972c8e55b0c796a`.
- Windows and macOS retain their deployed Qt/liblsl runtimes. Linux packages
  copy the Qt plugins and dependency closure beside LabRecorder, so recording
  does not require the user to install Qt or liblsl separately.
- The native IPC command accepts no executable path, arguments, configuration,
  or shell text. It resolves only the platform-specific bundled resource.
- Polar Stream launches a fixed packaged profile with LabRecorder's
  unauthenticated remote-control socket disabled. A user can still deliberately
  load another LabRecorder configuration from inside the recorder.
- Upstream `LICENSE` and `README.md` files remain in the packaged directory and
  Polar Stream's third-party notices identify the bundled application. The
  dynamically linked Qt runtime includes its notice plus LGPL-3.0 and GPL-3.0
  license texts and remains replaceable with interface-compatible libraries.

`scripts/prepare_lab_recorder.py` owns download/source-build staging and
artifact validation. Release jobs also launch the recorder from the exact MSI,
DMG, AppImage, and DEB payloads before publishing a complete package set.

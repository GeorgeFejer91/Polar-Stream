# Tauri assessment for Polar Stream

## Conclusion

Tauri provides a **material advantage for Polar Stream**, but the advantage is
specific: it gives the app a small, distributable HTML shell around a native
Rust sensor and publishing engine. It does not itself make Bluetooth, metric
calculation, LSL, OSC, or visualization low-latency. Those properties come from
the Rust data path, batching policy, and the deliberate decision that output
publication never waits for the WebView.

For this app, keep Tauri. Replacing it with Electron would add a bundled browser
without improving the native pipeline. Replacing it with Qt/C++ could remove the
WebView but would discard the requested HTML UI and add a second native language
without a meaningful latency gain at 130 Hz ECG and 200 Hz ACC. A Python/PySide
or PyWebView shell like PPS Kit could work at these sensor rates, but its
packaging and interpreter/scientific dependency surface would be larger and it
would be easier to accidentally place processing or publication on the UI
runtime.

## 13 August 2026 Rust/Tauri performance audit

The `tauri-rust-developer` review found that the largest observed runtime cost
was not Bluetooth or Rust: the original UI recursively requested animation
frames even while disconnected. After more than an hour idle, the v0.4.0
AppImage's WebKit content process was still using approximately **73% CPU** on
this Linux machine (an earlier sample was 81%). The revised data-driven renderer
requested no further idle frames; a three-sample `top` check measured **0.5%
WebKit CPU and 0.0% native-app CPU** after startup.

These point-in-time measurements are not a formal benchmark, but the order-of-
magnitude change directly matches the removed perpetual repaint. The revised
process group used about 475 MB RSS (158 MB app, 74 MB network process, 243 MB
content process). An earlier old-build sample used about 572 MB; RSS comparisons
between the AppImage's bundled WebKit and system WebKit remain approximate
because mapped/shared pages and page reclamation differ.

The structural changes behind that result are:

- Derived Rust processors are activated from the selected output set. The
  default raw-only configuration no longer maintains ECG-feature, HRV,
  coherence, breathing, or excitation state.
- LSL receives each **already-arrived Polar notification immediately** through
  one `lsl_push_chunk_ftp(..., pushthrough = 1)` call. This removes repeated FFI
  calls; it does not add a timer, accumulation interval, or application batch.
  OSC likewise emits one immediate UDP packet per notification.
- OSC names and packet storage are reused, LSL conversion storage is reused,
  and selected-output membership is a hash lookup rather than a repeated vector
  scan.
- A small bounded display-only queue isolates Tauri/WebView serialization. If
  the renderer cannot keep up, it may skip visualization frames; native LSL/OSC
  publication is never made to wait and does not drop sensor notifications.
- Canvas rendering is data-triggered and capped at 30 Hz, telemetry DOM updates
  are coalesced to 10 Hz, and ring-buffer drawing no longer allocates a new typed
  array per frame.
- The development-generated preview bundle is loaded only when the metric
  library opens, and all preview SVG nodes are removed when it closes. The SVGs
  are not part of live-signal rendering.
- Release builds use thin LTO, one codegen unit, and stripped symbols. The cold
  optimized build took 19 minutes 49 seconds after clearing 7.6 GiB of prior
  Cargo artifacts, reinforcing that build/cache cost is a genuine downside even
  for a compact Tauri application.

The optimized native executable is 7.2 MB on this machine. The waveform icon is
now generated into native PNG, ICO, ICNS, Windows AppX, Android, and iOS sizes;
the running Linux window exposes it through `_NET_WM_ICON` for taskbar/pin use.

Implementation references:

- [liblsl outlet/chunk API](https://labstreaminglayer.readthedocs.io/projects/liblsl/ref/outlet.html)
- [Cargo release profile controls](https://doc.rust-lang.org/cargo/reference/profiles.html)
- [Tauri icon generation](https://v2.tauri.app/develop/icons/)

## What Tauri contributes—and what it does not

| Property in Polar Stream | Main source of the benefit | Tauri-specific? |
| --- | --- | --- |
| BLE decoding and connection management | Rust crates and `btleplug` | No |
| Metric calculations without a Python runtime | Rust metric engine | No |
| LSL/OSC publishing independent of repainting | Native output router and coordinator | No |
| HTML/CSS interface in a desktop window | System WebView managed by Tauri | Yes |
| Typed Rust-to-JS streaming channel | Tauri IPC channel | Yes |
| OS installers and application metadata | Tauri bundler | Yes |
| CSP, window capabilities, and a five-command invoke surface | Tauri security/configuration plus app design | Partly |
| Shared desktop/mobile shell and Rust library shape | Tauri 2 mobile architecture | Yes, but Android BLE glue remains unfinished |

Tauri's documentation describes its architecture as Rust plus HTML rendered in
the operating system WebView, with message passing between the two. It also
recommends channels for streamed data. Polar Stream uses exactly that pattern,
but sends notification-sized batches to bounded JavaScript buffers rather than
one IPC message per sample:

- [Tauri architecture](https://v2.tauri.app/concept/architecture/)
- [Tauri channels](https://v2.tauri.app/develop/calling-rust/#channels)

The critical performance choice is visible in `apps/polar-stream/src/lib.rs`:
each ECG/ACC notification is published to LSL/OSC and processed for metrics in
Rust before the same batch is sent to the WebView. Therefore a slow, hidden, or
paused chart cannot delay research output. Tauri makes this boundary convenient;
it is the boundary—not the framework name—that protects latency.

## Comparison with the other local apps

This assessment inspected the current local repositories on 13 August 2026:

- **UnderTree (`localtex-studio`)** is a Node 22/Vite application with a local
  server, CodeMirror/Tiptap, PDF.js, and the Codex SDK. Its released Linux
  archive is only 1.22 MB because it is not a self-contained native runtime.
  That is a good fit for a document tool whose principal work is already web and
  process orchestration. Tauri would provide a tidier native shell, but it would
  not make TeX compilation or PDF.js faster and would still require Node or
  equivalent sidecars for current features.
- **PPS Kit** is a Python 3.10–3.13 package with NumPy, SciPy, audio, FastAPI,
  PyWebView/PySide, and PyInstaller variants. Python is an advantage there
  because scientific/audio tooling and experiment generation dominate. Merely
  replacing the window with Tauri would not improve audio timing; that would
  require moving the time-critical audio engine itself to native code.
- **Polar Stream** has a much narrower UI and a continuous hardware-to-network
  path. This is the strongest of the three cases for Rust plus Tauri: the
  WebView is presentation only, while all correctness- and latency-sensitive
  work is in reusable native crates.

## Advantages demonstrated in this repository

### 1. A clean low-latency boundary

Tauri channels let the app keep a single HTML/CSS/JS interface while Rust owns
BLE, metrics, timestamps, LSL, and OSC. The visualizer can run at display rate
and downsample to pixels; the publishers keep the original notification cadence.
An Electron or browser-server implementation can use the same conceptual split,
but Tauri supplies the bridge without shipping Node and Chromium.

### 2. Small native packages where the OS WebView can be reused

Measured v0.3.0 release assets were 7.84 MB for the universal macOS DMG and
4.10–4.15 MB for Linux DEBs. This is a genuine advantage over bundling a browser
or Python scientific stack. The 79.8–81.6 MB Linux AppImages are larger because
they intentionally bundle portability libraries and liblsl.

The Windows result exposes an important exception: this project deliberately
uses Tauri's **offline WebView2 installer**, so v0.3.0 MSI/NSIS packages were
189–215 MB. Windows 10/11 normally already has WebView2, and Tauri documents
that the installer can ensure it is present; choosing the offline installer
trades download size for installation reliability. Thus “Tauri always makes a
small installer” is false for the current Windows policy.

- [Tauri WebView versions and providers](https://v2.tauri.app/reference/webview-versions/)
- [Tauri application-size guidance](https://v2.tauri.app/concept/size/)

Small downloads must not be confused with low memory use. On this Linux machine,
the idle v0.3.0 AppImage process group used approximately **443 MB RSS** in total:
151 MB for `polar-stream`, 61 MB for the WebKit network process, and 232 MB for
the WebKit content process (rounded, one point-in-time measurement). The OS may
share some WebKit pages with other processes, so RSS is not a precise incremental
cost, but the result is enough to reject a claim that Tauri is inherently a
low-memory UI on Linux. Its clear footprint advantage here is distribution size,
not demonstrated resident memory.

### 3. One packaging model for all desktop targets

The release workflow emits DMG, AppImage, DEB, MSI, and NSIS packages from one
application manifest. That is less custom packaging code than PPS Kit's several
PyInstaller entry points/specifications. It does not eliminate platform work:
Bluetooth permissions, signing, WebView behavior, and liblsl still need
platform-specific testing.

### 4. Useful defense in depth for a local hardware app

The frontend has a restrictive CSP, loads only packaged assets, and exposes five
registered Rust commands. Tauri 2 also provides capability/permission scoping.
This is stronger by default than an unconstrained localhost server, although it
does not remove the need to validate command inputs and audit native libraries.

- [Tauri capabilities](https://v2.tauri.app/security/capabilities/)
- [Tauri Content Security Policy](https://v2.tauri.app/security/csp/)

### 5. A credible mobile reuse path, not automatic Android support

Tauri 2 supports Android/iOS shells and native Kotlin/Swift plugins, and this
workspace already produces `staticlib`, `cdylib`, and `rlib` forms. That allows
the metric and output crates plus the HTML UI to be reused. It does **not** mean
the desktop binary already works on Android: Polar BLE permissions/GATT glue,
background behavior, Android liblsl packaging, lifecycle testing, and a signed
APK/AAB pipeline are still required. Tauri's own release notes also caution that
not every plugin is supported on mobile.

- [Tauri 2 mobile architecture and limitations](https://v2.tauri.app/blog/tauri-20/)
- [Tauri mobile prerequisites](https://v2.tauri.app/start/prerequisites/#configure-for-mobile-targets)

## Costs and limitations observed

1. **Build cost is substantial.** After clearing a 7.5 GB rebuildable Cargo
   target cache, a cold `cargo check -p polar-stream --locked` took 4 minutes
   5 seconds on this computer. The checked-in source is small, but native GUI
   and Linux WebKit dependencies create a large development cache.
2. **Linux WebKit is not memory-light in this measurement.** The release process
   group occupied roughly 443 MB RSS while idle, despite the 4.15 MB DEB.
3. **WebViews are not identical.** Windows uses WebView2; macOS uses WKWebView;
   Linux uses WebKitGTK. The background renderer uses Chromium for deterministic
   visual assertions, so CI still needs packaged launch tests on every target to
   catch provider-specific differences.
4. **IPC is not free.** Sending each 200 Hz sample separately would be wasteful.
   Polar Stream avoids that by batching native notifications and by never routing
   LSL/OSC through JavaScript.
5. **Tauri is not a scientific runtime.** NeuroKit is valuable for generating
   explanatory mock traces, but it remains a development-only generator. Real
   production metrics stay in tested Rust modules.
6. **Signing and hardware access remain platform work.** Tauri creates package
   formats; it does not supply Apple/Windows signing identities, validate Polar
   BLE behavior on every adapter, or guarantee Android parity.

## Decision

Tauri is worth retaining for Polar Stream. Its unique contribution is the
combination of an HTML-driven UI, a small system-WebView shell, direct Rust IPC,
security controls, multi-platform bundling, and a route to mobile. The measurable
latency and reliability advantages, however, should be credited primarily to
the modular Rust pipeline and to bypassing the UI for LSL/OSC—not to Tauri's
renderer.

If this project later becomes visualization-heavy enough to require guaranteed
identical GPU behavior across operating systems, revisit a dedicated rendering
engine or bundled Chromium. For the current simple three-panel UI and 130/200 Hz
signals, that cost is not justified.

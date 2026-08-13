# Browser Web Bluetooth to native LSL

Polar Stream's GitHub Pages interface can connect directly to a Polar H10 in a
compatible Chromium browser. A browser tab still cannot create the UDP
multicast discovery and native data sockets required by Lab Streaming Layer.
Polar Stream therefore keeps LSL native and provides an explicit local
companion rather than simulating a browser outlet.

## Operator workflow

1. Install and open the current Polar Stream desktop app on the same computer
   as Chrome or Edge.
2. In Output, press **Open browser with authenticated LSL bridge**.
3. Allow the browser's local-network permission. Keep the desktop app open.
4. In the opened Pages tab, choose **Polar H10 via browser** and approve the
   Web Bluetooth chooser, or choose the visibly synthetic NeuroKit input.
5. Enable LSL and select the output modules. LabRecorder or another LSL resolver
   sees the same canonical stream names used by the desktop path.

A Pages tab opened normally has no bridge secret and keeps LSL disabled. Browser
OSC is not implemented. A smartphone can use the responsive Pages UI and, where
supported, direct Web Bluetooth, but its loopback address is the phone itself;
it cannot use a companion running on a separate desktop computer.

## Data path

```text
H10 Web Bluetooth or labeled NeuroKit mock
  -> shared browser event contract
  -> bounded JavaScript bridge queue
  -> authenticated HTTP on 127.0.0.1:<ephemeral port>
  -> dedicated Rust OutputRouter
  -> packaged liblsl
  -> LSL network
```

The native H10 path remains separate:

```text
H10 -> btleplug -> Rust decoder -> native OutputRouter -> LSL / OSC
```

No browser request, rendering task, or companion queue is placed on that native
sensor path.

## Security boundary

- The server binds IPv4 loopback only and chooses an ephemeral port.
- Every desktop launch replaces any previous pairing and generates a fresh
  random 128-bit token.
- The token is delivered in the URL fragment, which is not sent in the HTTPS
  request to GitHub Pages, and the page removes it immediately after startup.
- The token remains in memory and is never placed in local storage or logs.
- Every non-preflight request requires `Authorization: Bearer <token>` plus a
  random, in-memory browser client identifier. The first authenticated client
  owns the launch; a copied or second tab receives HTTP 409.
- The server validates the exact `Host` and allows only the canonical Pages
  origin plus explicit loopback development origins.
- CORS and Chromium Local Network Access are both required; binding to loopback
  is not treated as authentication.
- Header, body, event, sample, and concurrency limits are enforced before
  deserialization or publication. Incomplete requests lose their slot after a
  five-second read deadline.

Closing the desktop app removes the bridge and its token. Pressing the launch
action again replaces the prior bridge, invalidates its token, and pairs one new
browser client.

## Backpressure and timing

The page queues at most 256 notification/metric events, sends at most 24 events
per request after a 20 ms flush delay, and allows one request in flight. When
the queue fills or the bridge disappears, it drops pending browser events,
disables LSL, and shows the count/error instead of retrying forever or growing
memory. The Rust side accepts at most eight concurrent requests, 64 events per
batch, 1,024 samples per event, 16 KiB of headers, and 256 KiB of body data;
each request must finish arriving within five seconds.

This route adds browser decoding, serialization, scheduling, and loopback HTTP
latency. LSL timestamps are assigned by native liblsl when the bridge receives a
batch; H10 sensor timestamps are preserved in the versioned event contract but
are not silently substituted for the host LSL clock. Direct native acquisition
remains the preferred low-latency research path.

## Validation boundary

Automated tests prove URL-fragment removal, permission-compatible fetch setup,
CORS/authentication behavior, request limits, configuration, ECG/ACC forwarding,
zero-drop nominal mock replay, canonical Pages asset parity, and 390/320 px
layouts. Windows CI proves the companion and WinRT code compile on Windows.

Those checks do not prove physical-H10 browser throughput, Bluetooth reconnect,
Chrome permission behavior on every OS, end-to-end latency, long-run loss, or
LSL interoperability on every lab network. Record those measurements on the
target hardware before using the browser route as experimental evidence.

## Primary references

- [Chrome Local Network Access](https://developer.chrome.com/blog/local-network-access)
- [Web Bluetooth specification](https://webbluetoothcg.github.io/web-bluetooth/)
- [liblsl](https://github.com/sccn/liblsl)
- [MesmerPrism Windows GATT access/retry implementation](https://github.com/MesmerPrism/PolarH10/blob/main/src/PolarH10.Transport.Windows/WindowsGattServiceHandle.cs)
- [WinRT `GattSession.MaxPduSize`](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.genericattributeprofile.gattsession.maxpdusize)

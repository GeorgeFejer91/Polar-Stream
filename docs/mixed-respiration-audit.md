# Mixed Polar/Vernier respiration audit

Status: release-candidate implementation audit for Polar Stream 0.6.0.

## Conservative Polar respiration contract

Polar's official H10 SDK surface provides axis-specific acceleration, not a
respiration measurement. Polar Stream therefore offers only this three-stream
set for new Polar respiration configurations:

| ID | Meaning | Interpretation rule |
| --- | --- | --- |
| `breathing_volume` | Timed-PCA chest-motion waveform, fixed 0–1 | Relative waveform only; not lung volume, airflow, or respiratory rate |
| `breathing_signal_ready` | Calibration/freshness/motion gate, 0 or 1 | Reject or pause interpretation when zero |
| `breathing_signal_confidence` | Range × motion × PCA-dominance quality index, 0–1 | App-specific signal quality, not probability of physiological correctness |

The UI adds and removes these three outputs as a set. `raw_acc` remains an
automatic audit/reprocessing signal and `acc_magnitude` remains a general motion
signal. New configurations use Timed PCA v1, keep the waveform's canonical 0–1
scale, and cannot select the legacy algorithm or a second output
normalization.

Twenty-two older Polar respiratory IDs remain executable so saved studies can
be reopened without breaking stream names. They are hidden from new selection
and visibly labeled **restored compatibility output · unvalidated**. This
includes projection, phase, calibration, rate, confidence derived from cycle
count, and interval/amplitude dynamics. Their IDs and suffixes are covered by a
migration test.

The release set is deliberately conservative, not clinically validated. A
chest-accelerometer validation study found that usable coverage and agreement
depend strongly on the quality threshold, posture, and sensor position. Reviews
of inertial respiration monitoring likewise identify motion artifact as a
major limitation. No reviewed source in this audit validated this specific H10
mounting plus Polar Stream algorithm against airflow or respiratory
inductance plethysmography. See:

- [Polar H10 SDK capabilities](https://github.com/polarofficial/polar-ble-sdk/blob/master/documentation/products/PolarH10.md)
- [Schipper et al. (2021), chest accelerometer versus respiratory inductance plethysmography](https://pubmed.ncbi.nlm.nih.gov/33739305/)
- [Respiratory-rate wearables under motion](https://pubmed.ncbi.nlm.nih.gov/20007035/)

## Simultaneous transport, logging, and visualization

| Surface | Polar derived waveform | Vernier derived waveform | Simultaneous behavior |
| --- | --- | --- | --- |
| Native LSL | `<base>_<source>_breathingVolume` plus readiness/confidence when selected | automatic `<base>_<source>_vernierBreathing`; automatic sparse Double64 `rawVernier` is separate | Independent per-source outlets share liblsl's host clock; source suffixes prevent collision |
| Native OSC | selected Polar scalar paths | automatic `/<base>_<source>_vernierBreathing` | Both nonblocking publishers can remain live; Vernier timestamps are backfilled from its configured sample period |
| Native CSV | automatic raw ECG/ACC and selected derived rows | raw force plus `vernier_breathing` rows | Each source owns a bounded writer and source-scoped file; host and source timestamps permit later alignment |
| Desktop visualization | fixed-range 0–1 waveform | fixed-range 0–1 waveform | One can be selected and the other added as a time-aligned comparison without republishing or merging sources |
| Browser CSV | Polar browser events and selected derived rows | raw force plus browser-derived `vernier_breathing` | One recording session tracks all active sources and continues if only one source disconnects |
| Browser LSL/OSC | unavailable | unavailable | Web Bluetooth stays browser-local; native transports require the installed app |

Raw acquisition is published before display delivery. Each source owns its own
output router and bounded UI channel, so choosing a different chart or a slow
WebView cannot merge sources or block native publication. The Vernier derived
waveform is computed only after its exact raw frame is handed to the native raw
LSL publisher.

## Automated evidence in this release candidate

- Rust metric tests cover Timed-PCA source-time reconstruction, calibration,
  readiness, motion rejection, confidence bounds, and presentation isolation.
- The mixed liblsl producer creates Polar raw ECG, raw ACC, and
  `breathingVolume` alongside Vernier `rawVernier` and `vernierBreathing`.
  The official-pylsl verifier resolves the exact five descriptors and checks
  overlapping timestamps, channel schemas, finite derived values, and 0–1
  bounds.
- A real localhost UDP test decodes Vernier OSC packets and checks canonical
  address, value order, and sample-period timestamp backfill.
- Native CSV tests cover raw and derived rows, including
  `breathing_volume` and `vernier_breathing`. The reference analyzer accepts
  both native schema 2 and current schema 3 recordings. A two-router test writes
  simultaneous Polar and Vernier rows plus source-appropriate provenance into
  distinct files in one directory; a separate allocator test fixes both stream
  name and start millisecond and proves collision-safe, non-overwriting paths.
- Polar respiration LSL descriptors and CSV headers record the application
  version, processor mode, selected axes, and every clamped processing setting.
  CSV starts a new file when that provenance changes, and a respiration LSL
  outlet fails closed if liblsl cannot attach the required processing metadata.
- Vernier breathing LSL descriptors and native CSV metadata record a versioned
  fixed-processor contract: application version, window, robust-bound sample
  threshold and cadence, quantiles, non-finite policy, output range, and inhale
  polarity. Its derived LSL outlet likewise fails closed if that contract cannot
  be attached.
- Browser acceptance tests record Polar and Vernier derived rows in one CSV,
  record a bounded identity manifest for the initial and late-added sources,
  then disconnect one source and prove the surviving source continues.
- Renderer tests cover source-safe output cards and the time-aligned Polar 0–1
  versus Vernier 0–1 comparison view.
- Renderer rejection tests cover add, remove, settings, legacy upgrade, and
  audio-toggle failures. They prove that the last accepted configuration and
  dialogs are restored without disconnecting simultaneous Polar/Vernier
  sources or announcing a false success.

## Remaining physical and release gates

Automated tests establish routing and data contracts, but they do not replace a
simultaneous physical H10 + GDX-RB acceptance run. Before calling the respiratory
result validated, record both devices with LabRecorder and CSV, inspect gaps and
clock alignment, and compare the H10 waveform only where `ready=1` against the
Vernier reference. Repeat across strap positions, posture, paced rates, natural
breathing, and deliberate body motion.

Changing Polar breathing processing settings commits at a notification
boundary shared by the source processor and every output router, so a batch is
handled wholly by either the old or the new provenance. Settings that alter the
Timed-PCA estimator reset its calibration; downstream analysis must wait for
`breathing_signal_ready=1` again. A restored dynamics-only subset cannot retain
per-output upstream breathing settings; it executes against the current default
upstream respiration settings, and its provenance records those effective
defaults. Compatibility dynamics also do not yet version every downstream
cycle/statistics parameter, and optional normalization on legacy outputs is not
encoded in their stream metadata. These limits do not affect the new three-item
release set, whose waveform scale is fixed and whose quality outputs cannot be
normalized, but they prevent treating restored outputs as reproducible new-work
measures.

The native packaging matrix targets Windows x64/ARM64, a universal macOS 14+
DMG, and Ubuntu-22.04-based Linux x64/ARM64 AppImage and DEB packages. CI can
build and launch-test these packages, but Windows Authenticode signing and Apple
Developer ID notarization are not configured. Until those credentials and a
clean-machine physical device run exist, downloads must be labeled unsigned
research previews rather than trusted production installers.

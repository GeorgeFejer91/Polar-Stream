# Polar Stream multi-streamer

Read this file when the user names the main Polar Stream app, controller,
multi-streamer, multi-source workflow, browser demo, or LabRecorder.

- Product: `Polar Stream`
- Source: `apps/polar-stream/`
- Bundle identifier: `dev.georgefejer.polarstream`
- Executable: `polar-stream`
- Release prefix: `Polar-Stream_*`
- Role: multi-source Polar/Vernier acquisition, visualization, LSL/OSC/CSV,
  browser demo, and bundled LabRecorder workflow.

Keep this product independent from both mini apps. It may share domain crates,
but it must not read either mini's preferences, launch a mini as a required
helper, or install into a mini's application directory.

For a completed main-app edit, run the main checks in
`GLOBAL-INSTRUCTIONS.md`, build and install Polar Stream, then stage, publish,
and verify the canonical Pages UI when the accepted change affects that surface.
Report the installed path and the live Pages URL.

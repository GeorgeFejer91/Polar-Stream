# UI design audit

Verified: 2026-09-03

Scope: the shared Polar Stream interface in `apps/polar-stream/ui`, rendered by
both the Tauri desktop application and the static browser demo.

## Method

The interface was reviewed against the Uncodixfy guidance, inspected in source,
and rendered with Playwright in light and dark themes. The automated coverage
includes 1440, 1024, 940, 820, 390, and 320 pixel viewports, the signal-library
dialog, the output-settings dialog, focus behavior, document landmarks, text
contrast, and horizontal overflow.

## Audit findings and disposition

| Severity | Finding | Disposition |
| --- | --- | --- |
| Critical | Literal white and pale component backgrounds inherited dark-theme text colors. The primary action also became white text on a bright green background. | Resolved with semantic surface and foreground tokens plus automated 4.5:1 contrast assertions for representative dialog controls and content. |
| High | Important labels and explanatory copy reached 7–10px. | Resolved for interactive labels and explanations; 9–10px is retained only for compact technical metadata. |
| High | The three-column workspace and two large dialogs had clipping windows at intermediate widths. | Resolved by moving the workspace, metric-library, and formula-layout breakpoints to 960, 1080, and 840px and testing representative boundary widths. |
| High | Several coarse-pointer controls were below a 44px target, and focus styling omitted links and text areas. | Resolved with coarse-pointer target rules and a shared visible focus treatment for buttons, inputs, selects, text areas, and links. |
| Medium | Number tiles, duplicate eyebrow labels, pills, gradients, deep shadows, and repeated tinted cards made the product resemble a generic dashboard rather than a research tool. | Resolved with direct section headings, quiet text status, flat list rows, underline filters, solid surfaces, and restrained depth. |
| Medium | Live regions wrapped interactive lists, Formula Lab introduced a second `main` landmark, and mobile metric selection left focus inside the hidden list. | Resolved by limiting announcements to status text, restoring one main landmark, preventing accidental dialog submission, and moving mobile focus to the selected detail heading. |
| Medium | The sticky mobile status bar could cover the long stacked workspace. | Resolved by returning the status bar to normal document flow at and below 960px. |

## Resulting design

Polar Stream now uses a quiet three-part workbench: Input, Output, and
Visualization. ECG red, ACC blue, Vernier green, source palettes, chart grids,
and scientific evidence remain because they carry domain meaning. Decorative
numbering, ornamental status marks, and redundant containers do not.

The implementation is presentation-only. It does not change BLE acquisition,
the metric catalog, respiration processing, stream payloads, recording schemas,
or LSL/OSC/CSV routing.

## Follow-up opportunities

- The stacked mobile workbench is intentionally complete but remains long. A
  future accessible section switcher could reduce scrolling if simultaneous
  visibility is not required during study operation.
- Desktop panels keep independent scrolling so device, output, and chart context
  can remain visible together. Stronger scroll-position affordances can be
  evaluated with real operators without changing that workflow prematurely.

# Changelog

All notable changes to the Wave Insight DSH plugin are documented here. The
format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Plugin
versions are independent of the Wave Insight application version.

## [0.1.3] - 2026-11-18

### Added

- The sidebar tab now renders the **real Wave Insight viewer** in an iframe
  instead of a hand-written SVG sketch.
- A new `wave-insight-web-backend` crate hosts the built UI and answers the
  front end's waveform queries over one loopback `/invoke` route, backed by the
  native memory-mapped dump registry. The Host starts it automatically and
  publishes its URL through the projection.
- The front end has a `web-remote` feature (mutually exclusive with `tauri`)
  that talks to that backend and accepts one-way view commands from the Host.
- Host view changes are forwarded to the embedded UI: open dump, add/remove
  signals, visible time range and cursors.

### Changed

- The old React/SVG waveform panel was removed from `client.js`.

## [0.1.2] - 2026-11-18

### Added

- `get_changes` now returns bounded `time`/`value` pairs to the model and
  renders them in the tool result. Previously it reported only a change count,
  so an assistant could never read what the waveform actually did.

### Fixed

- Tool failures are real `isError` results with stable codes instead of
  success-looking `Error:` text; session telemetry and the UI now see failures.
- The `waveInsight` projection folds PTC sub-call results
  (`tool/ptc-dispatch`) as well as native `tool/result` events, so waveform
  commands issued from `run_code` update the panel.
- A failed `openTab` is logged and retried; the Session is only marked opened
  after a successful call.
- The helper process now runs asynchronously and honors `exec.signal`, so a
  large dump no longer blocks every other DSH session and cancellation stops
  the helper.

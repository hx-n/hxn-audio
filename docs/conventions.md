# Conventions

- Preserve the allocation/free audit of render. No render-time locks,
  allocations, final resource drops, logging, host calls or thread creation.
  Prepare parameters and source storage on the control side.
- Keep the core independent of runtimes and devices. Runtime selection
  belongs in generated adapters, not Cargo features on the library core.
- Validate whole PCM writes before reserving capacity. Count accepted
  frames, retain the suffix and drain EOF without losing queued frames.
- Bound browser message payloads and ownership tables. Copy guest memory
  before acknowledging a request; never retain a pointer for async use.
- Keep hxn-av's PCM and decoder ownership rules. Do not introduce another
  codec or sample-format implementation in this library.
- Treat suspended/initializing browser output as states, not successful
  audible playback. Exercise trusted activation separately from checks that
  enable browser autoplay. Exercise suspend/resume and route changes on
  mobile; compilation alone is not a device pass.
- Test packaged assets and namespaced commands without Rust/source files.
  Public workflows use hxn CLI tools, never repository shell scripts.
- Keep playback runners, mobile test apps, fixtures and browser drivers under
  `tests/`. Run them against installed packages; exclude them from haxelib
  metadata and public CLI commands. Mobile test hosts are library validation
  fixtures, not general hxn application packaging or deployment tools.
- Runtime and concurrent/browser tests run under timeouts. Repeat them;
  record unavailable targets and measured timing evidence in git-bug.
- Update API docs and skills with workflow changes. Run cargo fmt, clippy,
  Rust tests and Haxe formatter before a commit. Commit/push only when asked.

- The combined Ash/iOS check uses DWARF unwind metadata when isolated Rust
  archives and Objective-C exceed Mach-O compact-unwind personality slots.
  Keep that host-link adjustment in the test harness.
- Confirm actual audibility with the test harness's `--audible` option;
  render/device metrics alone do not establish that a person heard output.

- Physical iOS checks use a dedicated app identifier and an explicitly
  selected development team. Keep signing details and device identifiers
  out of committed templates. Remove the temporary app after a run and
  verify its console completion marker; installation alone is not a pass.

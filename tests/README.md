# Testing hxn-audio

`playback/` contains the playback program, AAC fixture, browser driver and
Android/iOS test hosts. These are contributor tests, excluded from the
haxelib and its CLI. The mobile hosts exercise this library; applications
provide their own startup, deployment and audio-session/focus policy.

Run against a packaged hxn-audio selected in the active haxelib repository.
The package supplies the adapters and assets; the test class path supplies
the harness. From this checkout:

```
haxe -cp tests/playback --run Tool hl --vm ash
haxe -cp tests/playback --run Tool cpp
haxe -cp tests/playback --run Tool cpp --audible
haxe -cp tests/playback --run Tool cpp --av
haxe -cp tests/playback --run Tool hl --wasm --threads --browser --vm ash
haxe -cp tests/playback --run Tool cpp --wasm --browser
haxe -cp tests/playback --run Tool cpp --wasm --threads --browser
haxe -cp tests/playback --run Tool hl --target aarch64-apple-ios-sim --device <simulator-ID> --vm ash
haxe -cp tests/playback --run Tool cpp --target x86_64-apple-ios --device <simulator-ID>
haxe -cp tests/playback --run Tool cpp --av --target aarch64-apple-ios --device <device-ID> --team <team-ID>
haxe -cp tests/playback --run Tool hl --target aarch64-linux-android --device <adb-serial> --vm ash
haxe -cp tests/playback --run Tool cpp --target aarch64-linux-android --device <adb-serial>
```

Use an absolute test class path to run from a separate consumer directory.
Test assets resolve relative to that class path; output goes under the
working directory's `target/hxn-audio/check/`. Run each invocation under an
external timeout and repeat concurrent/browser checks. Add `--vm 'ash --mode
jit'` or `--vm 'ash --mode interp'` for the corresponding native VM modes.
Browser tests need Node 22+, Chrome and matching Ash/Emscripten tools. Ash
uses its own wasm build/serve tooling.

`--av` needs installed hxn-av and its matching platform archives. It decodes
the AAC fixture through hxn-av into a 257-frame ring, forcing partial-write
retries, and checks EOF and exact consumption while other sounds overlap.
`--audible` extends the generated tone to eight seconds at a moderate signal
level and never changes system volume. Device progress and backend latency
estimates do not prove audible output or acoustic loopback timing; obtain
listening confirmation separately.

Mobile tests need platform SDKs, a selected simulator/device and matching
package archives. Android also needs a JDK, SDK build-tools and NDK; its
Activity initializes the native application context. Physical iOS requires
macOS, a paired device in Developer Mode and an Xcode development account.
Pass `--team` or `HXN_APPLE_TEAM`; keep team/device identifiers in local
arguments, never in committed templates. The dedicated UIKit app owns its
playback session and runs the program on a worker. hxcpp links a static
archive; Ash builds a signed embedded AOT framework with its target runtime.
The test uses Xcode development signing, checks the device console completion
marker and uninstalls its temporary app, including after a failed launch.
The combined Ash/iOS host retains DWARF unwind metadata with compact-unwind
encoding disabled when isolated Rust personalities exceed Mach-O's table.

`cargo test --workspace` includes functional, concurrent streaming and render
allocation/free audits. `node tests/shared-dsp.mjs` checks shared DSP ownership
and memory growth; `node --expose-gc tests/mailbox-gc.mjs` checks browser
mailbox lifetime. `hxn check` runs the offline example on the selected
runtimes. Keep Haxe formatter checks covering `sources`, `examples` and
`tests`. Record results, measurements and unavailable platforms in git-bug.

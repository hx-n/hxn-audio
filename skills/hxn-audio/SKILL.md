---
name: hxn-audio
description: Use or change hxn-audio audio output, voices, PCM streams, bus routing and effects in Haxe on native, mobile and browser runtimes, including its installed hxn audio CLI commands.
---

# hxn-audio

Use `-lib hxn-audio`; packaged adapters and the AudioWorklet DSP require no
Rust in an application. `new AudioOutput({})` opens default output;
`AudioOutput.offline({})` provides explicit render-to-Bytes for device-free
tests. Options are records, not constructed classes.

- `new AudioBuffer(bytes, rate, channels)` takes finite little-endian
  interleaved f32 PCM, mono or stereo. `output.play(buffer, options)` returns
  an independently controlled voice. The voice retains closed buffers.
- `output.stream(rate, channels, capacity, options)` takes bounded writes.
  `write` returns accepted frames. Retain/retry the suffix on a later tick;
  `finish` marks EOF only after all input was accepted. Do not busy-wait.
- With hxn-av installed, `Av.copy(data)` uses its existing format/planar
  conversion; `Av.buffer(data)` makes an immutable sound. Decode and copy
  outside rendering, and close media blocks/options. Inspect hxn-av's
  actual platform support before assuming its decoders run on the web.
  `DecoderStream` owns a stream and borrows the decoder/output; call `pump`
  from the event loop to retry partial writes and drain EOF. `--av` on
  `hxn audio check` validates this with the installed hxn-av decoder.
- Bus zero is master; `output.bus(index)` accesses the fixed bank. Routing
  is acyclic and output-local. Voices can move with `setBus(bus)`; buses
  offer gain/pan, low-pass and feedback delay. Close every handle.
- Browser creation returns while initialization is pending. Poll metrics
  for state/error, request playback activation when suspended, and use a
  secure isolated page. The agent resumes requested playback on click/key.
  `pause` prevents that retry until `resume`.
- Mobile applications own audio-session/focus/background policy and call
  pause/resume at lifecycle boundaries. Poll native device events and
  reopen with selectDevice; don't discard the mixer or pending music.
- Latency metrics are backend timestamp estimates, not acoustic loopback.
  A zero latency is unknown, not instantaneous output.

The installed package provides `hxn audio doctor` and `hxn audio check
hl|cpp`. Add `--audible` for an eight-second listening check; render metrics
alone do not prove audible output. Browser checks add `--wasm --browser`, plus `--threads` for Ash or
threaded hxcpp. Use Ash's wasm build/serve tooling for HashLink programs.
Mobile checks add `--target` plus `--device`; `--av` also runs the platform
AAC decoder into the stream. The host owns SDK linking, app startup and
cleanup. Test a physical device's audibility separately from emulator and
simulator progress metrics.

When editing, read docs/design.md and docs/conventions.md. Keep rendering
allocation/free/lock/callback-free; defer retirement to the controller.
Run the Rust tests and allocation audit, hxn check on every native runtime,
actual output checks on mobile and all three browser modes, and installed
package checks. Source hooks build the DSP for wasm/packaging with nightly
Rust and rust-src; haxelib consumers receive the compiled module.

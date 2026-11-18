# hxn-audio

[![Native](https://github.com/hx-n/hxn-audio/actions/workflows/native.yml/badge.svg)](https://github.com/hx-n/hxn-audio/actions/workflows/native.yml)
[![Browser](https://github.com/hx-n/hxn-audio/actions/workflows/browser.yml/badge.svg)](https://github.com/hx-n/hxn-audio/actions/workflows/browser.yml)
[![Generated API](https://github.com/hx-n/hxn-audio/actions/workflows/generated.yml/badge.svg)](https://github.com/hx-n/hxn-audio/actions/workflows/generated.yml)
[![Release](https://github.com/hx-n/hxn-audio/actions/workflows/release.yml/badge.svg)](https://github.com/hx-n/hxn-audio/actions/workflows/release.yml)

Audio output and mixing for Haxe on HashLink, Ash, hxcpp and CPPIA,
with native device output and browser AudioWorklets.

Install with `haxelib install hxn-audio`, then use `-lib hxn-audio`. Its
adapters, Haxe classes, Worklet and DSP module are included. On Linux,
hxcpp applications also need the ALSA development package (`libasound2-dev`
on Debian/Ubuntu) for linking; HashLink/Ash need the ALSA runtime library.

```haxe
import hxn.audio.*;

final output = new AudioOutput({});
final sound = new AudioBuffer(pcm, 48000, 2); // interleaved f32 Bytes
final voice = output.play(sound, {gain: 0.5, pan: -0.2});
sound.close(); // the playing voice retains its samples
// Keep the output and voice until playback has finished.
```

Play immutable PCM buffers or bounded streams, route voices through buses,
and control gain, pan, filtering and delay. Close handles when done. Streams
report accepted frames; retry the unwritten suffix before marking EOF.
With `hxn-av`, `Av` and `DecoderStream` copy and feed decoded audio.

See the [API reference](https://hxn.dev/api/hxn-audio/) for the classes and
[design](docs/design.md) for mixing, ownership and buffering rules.

## Browser and mobile

The browser adapter supports Ash wasm and both Emscripten modes. The
packaged `hxn.audio.Init.register()` macro copies its assets beside the
program, including unthreaded hxcpp output. Serve them over a secure origin
with COOP/COEP isolation; Ash's serving tools provide those headers. The
AudioWorklet owns a standalone DSP module and shared stereo ring.
Haxe needs no worker thread to render audio.

Opening browser output is asynchronous. Check `metrics().state` for
`Initializing`, `Suspended`, `Running` or `Error`, and display
`metrics().error` on failure. Browser output starts suspended. Call `resume()` or activate it with a
trusted click/key; the page agent starts pending playback on those events. `pause()` suppresses
that retry until `resume()`. Device selection requires the browser's
`AudioContext.setSinkId` support and its device permissions.

Native output uses CoreAudio on Apple platforms, WASAPI on Windows, ALSA
on Linux and AAudio on Android (API 26 or newer). iOS uses CPAL's
AVAudioSession interruption and route handling. The application owns its
audio-session category, audio focus and background-audio policy. Call
`pause()` and `resume()` from the application's lifecycle. Poll device
failures and reopen with `selectDevice`; device changes do not reset the
mixer's sample rate or discard queued music. Android app hosts initialize
`ndk-context` with their JavaVM and application Context before audio calls;
the application host supplies this setup before opening output.

`metrics().latencySeconds` is a backend estimate: native callback-to-playback
timestamps plus renderer buffering, or WebAudio base/output latency.
Zero means it is not known yet. It is not an acoustic loopback measurement.
Browser PCM storage is bounded to 64 MiB, stream rings to 32 MiB, and pending
uploads to 32 MiB. Split long sources into stream writes. Each PCM buffer or
upload is limited to 32 MiB.

## CLI and development

`hxn audio doctor` checks the installed AudioWorklet DSP and reports browser
requirements. `hxn build` builds bindings and `hxn package` creates a haxelib.
Source lifecycle hooks build the DSP with nightly Rust and rust-src when
wasm or packaging needs it; `hxn audio dsp` builds it explicitly from source.

From source, `hxn check hl --vm ash` and `hxn check cpp cppia` exercise
mixing offline. Playback tests and their mobile/browser hosts live under
`tests/`; see [testing](tests/README.md) for contributor instructions.

See [design](docs/design.md), [conventions](docs/conventions.md) and the
[skill](skills/hxn-audio/SKILL.md) when changing this library.

## License notices

Redistributed applications must retain the applicable dependency notices in
[third-party/NOTICES.md](third-party/NOTICES.md) and the accompanying license
texts. The package includes the unchanged MPL-2.0 `audio_thread_priority`
source archive for builds that use it; preserve its source availability.

## Roadmap

The following validation and integration work remains open after the first release:

- **Browser coverage:** Firefox and physical Android Chrome/iPhone Safari,
  including AAC streaming through hxn-av.
- **Windows output:** real WASAPI device playback and latency checks, beyond
  CI compilation and offline mixer tests.
- **Device transitions:** headset connect/disconnect, output route changes,
  iOS interruptions, and Android audio-focus loss and recovery.
- **Latency:** acoustic loopback measurements on representative hardware.
  Current latency metrics are backend estimates.
- **hxn-av integration:** publish its browser PCM/AAC updates and validate
  the released packages together.

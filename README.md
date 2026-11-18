# hxn-audio

Audio output and mixing for Haxe on HashLink, Ash, hxcpp and CPPIA,
with native device output and browser AudioWorklets. Haxe supplies buffers and
controls voices; the mixer runs on the audio thread.

Use a packaged haxelib with `-lib hxn-audio`. Its adapters, Haxe classes,
Worklet and DSP module are included.

```haxe
import hxn.audio.*;

final output = new AudioOutput({});
final sound = new AudioBuffer(pcm, 48000, 2); // interleaved f32 Bytes
final voice = output.play(sound, {gain: 0.5, pan: -0.2, bus: 1});
sound.close(); // the playing voice retains its samples

final bus = output.bus(1);
bus.setLowpass(4000);
voice.setGain(0.8);
// Keep the output and voice until playback has finished.
```

`AudioOptions` selects the mixer sample rate, voice/bus capacities, requested
device buffer size and device ID. Empty device IDs choose the default.
`Audio.devices()` enumerates outputs; `selectDevice(id)` reopens native
output while preserving voices and effect state. Device events and browser
state are polled through `metrics()` on the application thread.

PCM is little-endian, finite, interleaved f32, mono or stereo, at
8000–192000 Hz. Buffers are immutable and shared by native voices. Sources
are linearly resampled to the mixer rate; this is suitable for ordinary
playback rather than offline mastering. `pan` uses equal-power panning.
Gain changes and explicit stops ramp over 32 mixer frames. Final output is
clamped after voices and buses are mixed.

For longer audio, `output.stream(rate, channels, capacityFrames, options)`
creates a bounded stream. `write(bytes)` returns accepted **frames**. Keep
and retry the unwritten suffix on a later application tick; do not spin or
block the page. `finish()` marks EOF after the last successful write and
lets the queue drain. Underruns output silence without advancing the source.
On platforms supported by hxn-av, with `-lib hxn-av`, `Av.copy(audioData)` uses hxn-av's existing planar/sample
conversion and `Av.buffer(audioData)` creates a reusable sound. Decoder
polling, copying and writes run on the application thread. Close decoded
blocks, their options and the decoder when done.

Bus zero is the master. The fixed bus bank initially routes each other bus
to zero. `routeTo` rejects cycles and buses from other outputs. Buses support
gain, pan, a low-pass filter and a preallocated feedback delay. `setBus`
moves a live voice to another bus. Close all handles explicitly when done.
Closing an output stops its device; remaining voices then report finished.
`finished()` means the mixer consumed the source; device buffers may still
contain its last frames.

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

With `hxn-av` installed, `hxn.audio.DecoderStream` transfers decoded audio
into a bounded stream. Call `pump()` from the event loop, keep the
decoder/output alive, and close the bridge when done. It retains and retries
partial writes and drains EOF.

See [design](docs/design.md), [conventions](docs/conventions.md) and the
[skill](skills/hxn-audio/SKILL.md) when changing this library.

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

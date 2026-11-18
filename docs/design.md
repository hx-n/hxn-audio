# Design

The Haxe API marks the Rust types in src/lib.rs for hxn generation. Its
session mutex serializes application-side operations only. Voice controls
use atomics. Sources and bus history belong to one Processor in the plain
Rust hxn-audio-core crate, which has no Haxe or device dependency.

The controller sends bounded commands through rtrb. Voices use immutable
PCM or bounded frame rings. The processor mixes in fixed 128-frame chunks,
applies bus routing in topological order, and clamps at the output. It owns
preallocated voice slots, bus scratch storage and epoch-tagged delay rings.
Changing a delay never clears a long buffer in a callback.

Finished sources, their sample references and stream rings pass through a
retirement ring. The controller reclaims them, outside rendering. A full
retirement queue retains the ended voice in its slot. Native-memory weights
stay alive with the samples or writer that owns the allocation.

CPAL owns the native stream. Its callback receives only the renderer and
atomic status. A small output resampler accommodates device rates without
retuning the mixer when devices change. Dropping the callback returns its
renderer through a parking slot outside render calls. Reopening waits for
ownership with a bounded timeout; no rendering lock is involved. Device
callbacks publish event codes and timestamp estimates rather than calling
Haxe.

The browser page agent owns AudioContexts and device selection. It serves
an Ash/Emscripten mailbox; unthreaded Emscripten dispatches directly on the
page. Requests acknowledge synchronously after copying any PCM they use.
Asynchronous WebAudio operations update page-owned state only. They never
write to Rust request/reply memory after acknowledgment. One library-wide
mailbox stays valid for the page's lifetime.

The page and its AudioWorklet instantiate the same standalone DSP module
with one shared memory. The page instance owns control allocation, PCM
uploads, stream writes and resource retirement. The Worklet receives only
an opaque renderer address and a dedicated preallocated stack. Its process()
calls render and copies a preallocated stereo ring into WebAudio channels.
No control-side code runs on the audio thread. The page waits for the
Worklet's startup acknowledgment before accessing shared control state.
Closing waits for AudioContext.close() before reclaiming the renderer.
An already-suspended AudioContext proceeds directly to DSP initialization;
only a running context needs a suspend transition before setup.

The same mixer core serves native, browser and offline output. Uploads,
commands, voices, PCM and stream rings have explicit bounds. Startup writes
reserve capacity; stop requests coalesce while startup is pending. Cached
PCM can serve multiple voices, and budgets count each allocation once,
including sources awaiting retirement.

The haxelib's CLI registration supplies the audio namespace and source
lifecycle hooks. Additional binary assets use cli.files and build-hook
page_files; the package's init macro copies browser assets for direct Haxe
builds too. The Worklet module is a prebuilt asset for haxelib consumers.

The playback harness and all of its hosts, fixtures and browser driver live
under `tests/playback/`. It builds against an installed haxelib so package
coverage does not depend on shipping the harness. See [testing](../tests/README.md).
The published CLI contains the DSP build hooks and `audio doctor`/`audio dsp`.
Application hosts own mobile startup, SDK linking and audio-session policy.

## Output, voices and streaming

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

With `hxn-av` installed, `hxn.audio.DecoderStream` transfers decoded audio
into a bounded stream. Call `pump()` from the event loop, keep the
decoder/output alive, and close the bridge when done. It retains and retries
partial writes and drains EOF.

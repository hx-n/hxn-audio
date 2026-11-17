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

The same mixer core serves native, browser and offline output. Uploads,
commands, voices, PCM and stream rings have explicit bounds. Startup writes
reserve capacity; stop requests coalesce while startup is pending. Cached
PCM can serve multiple voices, and budgets count each allocation once,
including sources awaiting retirement.

The haxelib's CLI registration supplies the audio namespace and source
lifecycle hooks. Additional binary assets use cli.files and build-hook
page_files; the package's init macro copies browser assets for direct Haxe
builds too. The Worklet module is a prebuilt asset for haxelib consumers.

Mobile checks build a small installed host around the packaged libraries.
Android initializes its application context before device access. Apple
hxcpp archives keep system libraries as auto-link references. The combined
Ash/iOS host disables compact-unwind encoding and retains DWARF unwind
metadata when isolated Rust personalities exceed Mach-O's compact table.
The listening option extends the signal; it never changes system volume.

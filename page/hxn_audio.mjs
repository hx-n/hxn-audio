// Page-owned controls acknowledge requests after copying their input. The
// Worklet only renders; no asynchronous operation retains a guest pointer.
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const outputs = new Map();
let nextOutput = 1;
let devices = [{id: '', name: 'Default output', isDefault: true}];
const base = () => new URL('.', location.href);
async function refreshDevices() {
  if (!navigator.mediaDevices?.enumerateDevices) return;
  try {
    const list = await navigator.mediaDevices.enumerateDevices();
    devices = [{id: '', name: 'Default output', isDefault: true}, ...list.filter(d => d.kind === 'audiooutput' && d.deviceId !== 'default').map(d => ({id: d.deviceId, name: d.label || 'Audio output', isDefault: false}))];
  } catch (_) { /* Enumeration may be restricted; the default remains usable. */ }
}
const empty = () => ({renderedFrames: 0, activeVoices: 0, starvedFrames: 0, clippedSamples: 0, peak: 0, voices: []});
function failure(output, error) {
  output.error = String(error); output.state = 'error';
  output.event = error?.name === 'NotFoundError' ? 2 : 3;
}
function text(output, address, length) {
  return decoder.decode(new Uint8Array(output.memory.buffer, address, length).slice());
}
function check(output, ok) {
  if (!ok) throw new Error(text(output, output.dsp.audio_error(), output.dsp.audio_error_len()));
}
function input(output, bytes) {
  const pointer = output.dsp.audio_input(bytes.length);
  if (!pointer && bytes.length) throw new Error('audio upload exceeds 32 MiB');
  new Uint8Array(output.memory.buffer, pointer, bytes.length).set(bytes);
}
function command(output, value) {
  input(output, encoder.encode(JSON.stringify(value)));
  check(output, output.dsp.audio_command());
}
function apply(output, value, pcm) {
  try {
    if (value.op === 'playBuffer') {
      const stored = output.buffers.get(value.buffer);
      const fresh = !stored?.loaded;
      if (fresh) {
        if (!pcm) throw new Error('audio buffer upload is missing');
        input(output, new Uint8Array(pcm));
        check(output, output.dsp.audio_load(value.buffer, value.sampleRate, value.channels));
      }
      try { command(output, {...value, op: 'play'}); }
      catch (error) {
        if (fresh) { command(output, {op: 'unload', buffer: value.buffer}); output.buffers.delete(value.buffer); }
        throw error;
      }
      if (fresh) output.buffers.set(value.buffer, {loaded:true, bytes:pcm.byteLength / (value.channels * 4) * 8});
      return {};
    }
    if (value.op === 'write') {
      input(output, new Uint8Array(pcm));
      const accepted = output.dsp.audio_write(value.id);
      if (accepted < 0) check(output, 0);
      return {accepted};
    }
    command(output, value);
    if (value.op === 'unload') output.buffers.delete(value.buffer);
    return {};
  } catch (error) {
    if (error instanceof WebAssembly.RuntimeError) failure(output, error);
    throw error;
  }
}
function snapshot(output) {
  if (output.ready && !output.error) {
    const address = output.dsp.audio_snapshot();
    output.snapshot = JSON.parse(text(output, address, output.dsp.audio_reply_len()));
  }
  const context = output.context;
  return {...output.snapshot, state: output.node || output.error ? output.state : 'initializing', error: output.error, event: output.event, device: output.device, deviceRate: context?.sampleRate || 0, callbacks: output.snapshot.callbacks || 0, latencySeconds: (context?.baseLatency || 0) + (context?.outputLatency || 0)};
}
async function initialize(output, options) {
  try {
    if (!globalThis.crossOriginIsolated || typeof SharedArrayBuffer === 'undefined') throw new Error('audio requires cross-origin isolation (COOP and COEP)');
    const context = new AudioContext({sampleRate: options.sampleRate, latencyHint: 'interactive'});
    output.context = context;
    if (context.state !== 'suspended') await context.suspend();
    if (output.closed) return;
    context.addEventListener('sinkchange', () => { output.event = 1; });
    context.onstatechange = () => { if (!output.closed && !output.error) output.state = context.state; };
    const module = await WebAssembly.compileStreaming(fetch(new URL('hxn_audio_dsp.wasm', base())));
    if (output.closed) return;
    output.memory = new WebAssembly.Memory({initial: 256, maximum: 8192, shared: true});
    output.dsp = new WebAssembly.Instance(module, {env: {memory: output.memory}}).exports;
    check(output, output.dsp.audio_create(context.sampleRate, options.voices, options.buses, 8192));
    const renderer = output.dsp.audio_renderer();
    const stackTop = output.dsp.audio_stack_top();
    await context.audioWorklet.addModule(new URL('hxn_audio_worklet.mjs', base()));
    if (output.closed) return;
    const node = new AudioWorkletNode(context, 'hxn-audio', {numberOfInputs: 0, numberOfOutputs: 1, outputChannelCount: [2], processorOptions: {module, memory: output.memory, renderer, stackTop}});
    output.node = node;
    node.onprocessorerror = () => failure(output, 'AudioWorklet processor failed');
    node.port.onmessage = ({data}) => {
      if (output.closed || !data.ready) return;
      output.ready = true;
      try {
        for (const {value, pcm} of output.queued) {
          const result = apply(output, value, pcm);
          if ((value.op === 'playBuffer' || value.op === 'stream') && output.startupStops.has(value.id)) command(output, {op:'stop',id:value.id});
          if (value.op === 'write' && result.accepted !== value.frames) throw new Error('reserved stream capacity changed');
        }
        output.queued.length = 0; output.bytes = 0; output.startupStops.clear();
      } catch (error) { failure(output, error); }
    };
    node.connect(context.destination);
    output.state = context.state;
    if (options.device) {
      if (!context.setSinkId) throw new Error('this browser cannot select an output device');
      await context.setSinkId(options.device);
    }
    if (output.wantRunning && output.activationRequested) await context.resume().catch(error => { if (error.name !== 'NotAllowedError') throw error; });
  } catch (error) { if (!output.closed) failure(output, error); }
}
function close(output) {
  output.closed = true;
  output.queued.length = 0; output.bytes = 0;
  output.node?.disconnect();
  // Closing resolves after audio processing stops. Keep the render owner alive
  // until then; disconnecting a node alone does not transfer ownership back.
  const stopped = output.context?.close() ?? Promise.resolve();
  stopped.then(() => {
    try { output.dsp?.audio_destroy(); } catch (_) { /* A trapped instance is reclaimed with its memory. */ }
    output.dsp = null;
    output.node?.port.close();
  }).catch(() => {});
}
function enqueue(output, value, pcm) {
  if (output.queued.length >= 256 || output.bytes + (pcm?.byteLength || 0) > 32 * 1024 * 1024) throw new Error('browser audio startup queue is full');
  output.queued.push({value, pcm}); output.bytes += pcm?.byteLength || 0;
}
export function execute(memory, request) {
  const op = request.op;
  if (op === 'devices') return devices;
  if (op === 'open') {
    const id = nextOutput++;
    const output = {id, device: request.device, state: 'initializing', event: 0, error: '', wantRunning: true, activationRequested: false, closed: false, ready: false, context: null, node: null, dsp: null, memory: null, queued: [], bytes: 0, voiceLimit: request.voices, handles: new Set(), streams: new Map(), buffers: new Map(), startupStops: new Set(), startupStreamFrames: 0, snapshot: empty()};
    outputs.set(id, output); initialize(output, request); return {id};
  }
  const output = outputs.get(request.output);
  if (!output || output.closed) throw new Error('audio output is closed');
  if (op === 'close') { close(output); outputs.delete(request.output); return {}; }
  if (op === 'info') return snapshot(output);
  if (op === 'pause' || op === 'resume') {
    output.wantRunning = op === 'resume'; output.activationRequested = output.wantRunning;
    output.context?.[op === 'resume' ? 'resume' : 'suspend']().catch(error => { if (error.name !== 'NotAllowedError') failure(output, error); });
    return {};
  }
  if (op === 'select') {
    if (!output.context?.setSinkId) throw new Error('this browser cannot select an output device');
    output.context.setSinkId(request.device).then(() => { output.device = request.device; output.event = 0; }).catch(error => failure(output, error));
    return {};
  }
  if (output.error) throw new Error(output.error);
  if (op === 'stop' && !output.ready) { output.startupStops.add(request.id); return {}; }
  if (op === 'available') {
    const stream = output.streams.get(request.id);
    if (!stream) throw new Error('unknown stream');
    if (!output.ready) return {available: stream.available};
    const voice = snapshot(output).voices.find(v => v.id === request.id);
    return {available: voice?.available ?? 0};
  }
  let pcm;
  if (op === 'write') {
    const stream = output.streams.get(request.id);
    if (!stream || stream.finished) throw new Error('unknown or finished stream');
    const frames = Math.min(request.length / (stream.channels * 4), output.ready ? Infinity : stream.available);
    if (!frames) return {accepted: 0};
    pcm = new Uint8Array(memory.buffer, request.address, frames * stream.channels * 4).slice().buffer;
    const value = {...request, frames};
    if (!output.ready && (output.queued.length >= 256 || output.bytes + pcm.byteLength > 32 * 1024 * 1024)) return {accepted:0};
    if (output.ready) return apply(output, value, pcm);
    enqueue(output, value, pcm); stream.available -= frames; return {accepted: frames};
  }
  if (op === 'playBuffer' || op === 'stream') {
    if (output.handles.size >= 512) throw new Error('close finished voice handles before adding more');
    if (!output.ready && output.handles.size >= output.voiceLimit) throw new Error('mixer has no free voice slots');
    if (op === 'playBuffer' && !output.buffers.has(request.buffer)) {
      const size = request.length / (request.channels * 4) * 8;
      const used = [...output.buffers.values()].reduce((sum, buffer) => sum + buffer.bytes, 0);
      if (output.buffers.size >= 512 || used + size > 64 * 1024 * 1024) throw new Error('browser audio PCM cache exceeds its capacity');
      pcm = new Uint8Array(memory.buffer, request.address, request.length).slice().buffer;
    }
    if (op === 'stream' && !output.ready && output.startupStreamFrames + request.capacity > 4194304) throw new Error('browser stream rings exceed 32 MiB');
  }
  if (output.ready) apply(output, request, pcm);
  else {
    enqueue(output, request, pcm);
    if (op === 'playBuffer' && pcm) output.buffers.set(request.buffer, {loaded:false,bytes:pcm.byteLength / (request.channels * 4) * 8});
    if (op === 'stream') output.startupStreamFrames += request.capacity;
  }
  if (op === 'playBuffer' || op === 'stream') output.handles.add(request.id);
  if (op === 'stream') output.streams.set(request.id, {channels: request.channels, available: request.capacity, finished: false});
  if (op === 'finish') { const stream = output.streams.get(request.id); if (stream) stream.finished = true; }
  if (op === 'forget') { output.handles.delete(request.id); output.streams.delete(request.id); }
  return {};
}
export function start({memory, address, serveMailbox = true}) {
  refreshDevices();
  navigator.mediaDevices?.addEventListener('devicechange', () => { refreshDevices(); for (const output of outputs.values()) output.event = 1; });
  const resume = () => { for (const output of outputs.values()) if (output.wantRunning && !output.closed) { output.activationRequested = true; output.context?.resume().catch(() => {}); } };
  document.addEventListener('pointerdown', resume, {capture: true});
  document.addEventListener('keydown', resume, {capture: true});
  const dispatch = () => {
    const words = new Int32Array(memory.buffer, address, 8);
    let response;
    try {
      const request = JSON.parse(decoder.decode(new Uint8Array(memory.buffer, words[2] >>> 0, words[3] >>> 0).slice()));
      response = {ok: execute(memory, request)};
    } catch (error) { response = {error: String(error)}; }
    let bytes = encoder.encode(JSON.stringify(response));
    if (bytes.length > (words[5] >>> 0)) bytes = encoder.encode('{"error":"audio reply exceeds mailbox capacity"}');
    new Uint8Array(memory.buffer, words[4] >>> 0, bytes.length).set(bytes); words[6] = bytes.length;
  };
  if (serveMailbox) (async () => {
    let seen = 0;
    for (;;) {
      const words = new Int32Array(memory.buffer, address, 8);
      if (Atomics.waitAsync) {
        const wait = Atomics.waitAsync(words, 0, seen); if (wait.async) await wait.value;
      } else {
        while (Atomics.load(words, 0) === seen) await new Promise(resolve => setTimeout(resolve, 1));
      }
      const sent = Atomics.load(words, 0); if (sent === seen) continue;
      dispatch(); seen = sent; Atomics.store(words, 1, seen); Atomics.notify(words, 1);
    }
  })();
  return dispatch;
}

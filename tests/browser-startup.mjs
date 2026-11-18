// Already-suspended contexts must initialize before any trusted activation.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import {execute} from '../page/hxn_audio.mjs';

const wasm = readFileSync(new URL('../assets/hxn_audio_dsp.wasm', import.meta.url));
globalThis.crossOriginIsolated = true;
globalThis.location = {href: 'http://localhost/playback/index.html'};
globalThis.fetch = async () => new Response(wasm, {headers: {'Content-Type': 'application/wasm'}});

const contexts = [];
let initialState = 'suspended';
globalThis.AudioContext = class {
  constructor() {
    this.state = initialState;
    this.sampleRate = 48000;
    this.baseLatency = 0.01;
    this.destination = {};
    this.suspends = 0;
    this.audioWorklet = {addModule: async () => {}};
    contexts.push(this);
  }
  addEventListener() {}
  suspend() {
    this.suspends++;
    // Reproduce a suspend request that never settles on an idle context.
    if (this.state === 'suspended') return new Promise(() => {});
    this.state = 'suspended';
    return Promise.resolve();
  }
  resume() {
    this.state = 'running';
    this.onstatechange?.();
    return Promise.resolve();
  }
  close() { this.state = 'closed'; return Promise.resolve(); }
};
globalThis.AudioWorkletNode = class {
  constructor() {
    this.port = {close() {}};
    queueMicrotask(() => this.port.onmessage?.({data: {ready: true}}));
  }
  connect() {}
  disconnect() {}
};

for (initialState of ['suspended', 'running']) {
  const {id} = execute(null, {op: 'open', sampleRate: 48000, voices: 4, buses: 2, device: ''});
  let info;
  for (let tick = 0; tick < 200; tick++) {
    info = execute(null, {op: 'info', output: id});
    if (info.state !== 'initializing') break;
    await new Promise(resolve => setTimeout(resolve, 10));
  }
  assert.equal(info.error, '');
  assert.equal(info.state, 'suspended', `${initialState} context did not finish initialization`);
  assert.equal(contexts.at(-1).suspends, initialState === 'suspended' ? 0 : 1);
  execute(null, {op: 'resume', output: id});
  assert.equal(execute(null, {op: 'info', output: id}).state, 'running');
  execute(null, {op: 'close', output: id});
  await Promise.resolve();
}
console.log('suspended/running context startup and explicit resume: passed');

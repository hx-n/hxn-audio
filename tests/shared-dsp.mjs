// Control allocations may grow memory after the Worklet has captured its view.
import {readFileSync} from 'node:fs';
import assert from 'node:assert/strict';
const module = await WebAssembly.compile(readFileSync(new URL('../assets/hxn_audio_dsp.wasm', import.meta.url)));
const memory = new WebAssembly.Memory({initial:256, maximum:8192, shared:true});
const control = new WebAssembly.Instance(module,{env:{memory}}).exports;
assert.equal(control.audio_create(48000,4,1,128),1);
const renderer=control.audio_renderer();
const render = new WebAssembly.Instance(module,{env:{memory}}).exports;
render.__stack_pointer.value=control.audio_stack_top();
const original=new Float32Array(memory.buffer);
const samples=new Float32Array(8*1024*1024);samples.fill(0.1);
const address=control.audio_input(samples.byteLength);
new Uint8Array(memory.buffer,address,samples.byteLength).set(new Uint8Array(samples.buffer));
assert.equal(control.audio_load(1,48000,1),1);
function command(value){
  const bytes=new TextEncoder().encode(JSON.stringify(value));
  const address=control.audio_input(bytes.length);
  new Uint8Array(memory.buffer,address,bytes.length).set(bytes);
  assert.equal(control.audio_command(),1,JSON.stringify(value));
}
for(const id of [1,2])command({op:'play',id,buffer:1,bus:0,gain:1,pan:0,rate:1,looping:false});
assert.ok(memory.buffer.byteLength>original.byteLength);
const output=render.audio_render(renderer,128)>>>2;
assert.ok(original[output]>0);
for(const id of [1,2])command({op:'stop',id});
for(let n=0;n<4;n++)render.audio_render(renderer,128);
control.audio_snapshot();
control.audio_destroy();
console.log('shared DSP memory growth, PCM reuse and shutdown: passed');

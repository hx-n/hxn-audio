// An idle asynchronous waiter must stay alive across GC and memory growth.
import {Worker} from 'node:worker_threads';
import assert from 'node:assert/strict';
assert.equal(typeof global.gc, 'function', 'run with --expose-gc');
Object.defineProperty(globalThis,'navigator',{value:{},configurable:true});
globalThis.document={addEventListener(){}};
const memory=new WebAssembly.Memory({initial:1,maximum:2,shared:true});
const agent=await import('../page/hxn_audio.mjs');
agent.start({memory,address:0});
const worker=new Worker(`const {parentPort,workerData}=require('node:worker_threads');
parentPort.on('message',()=>{
 const words=new Int32Array(workerData.buffer,0,8);
 const bytes=new TextEncoder().encode('{"op":"devices"}');
 new Uint8Array(workerData.buffer,64,bytes.length).set(bytes);
 words[2]=64;words[3]=bytes.length;words[4]=1024;words[5]=4096;
 const sent=Atomics.add(words,0,1)+1;Atomics.notify(words,0);
 while(Atomics.load(words,1)!==sent){const seen=Atomics.load(words,1);if(seen===sent)break;Atomics.wait(words,1,seen,1000);}
 parentPort.postMessage(new TextDecoder().decode(new Uint8Array(workerData.buffer,1024,words[6])));
});`,{eval:true,workerData:memory});
try {
 for(let n=0;n<12;n++){
  await new Promise(resolve=>setTimeout(resolve,5));
  for(let i=0;i<3;i++)global.gc();
  if(n===6)memory.grow(1);
  const reply=await new Promise((resolve,reject)=>{
   const timer=setTimeout(()=>reject(new Error('idle mailbox waiter was lost')),2000);
   worker.once('message',value=>{clearTimeout(timer);resolve(value);});
   worker.postMessage('request');
  });
  assert.equal(JSON.parse(reply).ok[0].name,'Default output');
 }
 console.log('mailbox GC and memory growth: passed');
} finally {await worker.terminate();}

// Only rendering enters this instance. The page owns control allocation and
// deferred destruction; both instances use the same module and shared memory.
class HxnAudioProcessor extends AudioWorkletProcessor {
  constructor(options) {
    super();
    const {module, memory, renderer, stackTop} = options.processorOptions;
    const instance = new WebAssembly.Instance(module, {env: {memory}});
    instance.exports.__stack_pointer.value = stackTop;
    this.render = instance.exports.audio_render;
    this.renderer = renderer;
    this.view = new Float32Array(memory.buffer);
    // The page starts control calls only after this instance has finished startup.
    this.port.postMessage({ready: true});
  }
  process(_inputs, outputs) {
    const channels = outputs[0];
    if (!channels?.length) return true;
    const frames = channels[0].length;
    const pointer = this.render(this.renderer, frames) >>> 2;
    for (let channel = 0; channel < channels.length; channel++) {
      const out = channels[channel];
      for (let i = 0; i < frames; i++) out[i] = pointer ? this.view[pointer + i * 2 + Math.min(channel, 1)] : 0;
    }
    return true;
  }
}
registerProcessor('hxn-audio', HxnAudioProcessor);

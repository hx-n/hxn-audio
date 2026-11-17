#include <emscripten.h>
EM_JS(int, hxn_audio_start_js, (unsigned address), {
  if (typeof document === 'undefined') return 0;
  /*AGENT*/
  Module.hxnAudioExecute = start({memory: {get buffer() { return HEAPU8.buffer; }}, address, serveMailbox: false});
  return 1;
});
EM_JS(void, hxn_audio_execute_js, (), { Module.hxnAudioExecute(); });
int hxn_audio_agent(unsigned address) { return hxn_audio_start_js(address); }
void hxn_audio_execute() { hxn_audio_execute_js(); }

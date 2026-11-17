//! Shared DSP: controls run on the page, rendering on one Worklet.
use hxn_audio_core::{Config, Controller, Pcm, Processor, StreamWriter, Voice, VoiceOptions};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::SeqCst},
    },
};

struct Engine {
    control: Controller,
    renderer: usize,
    stack_top: usize,
    callbacks: Arc<AtomicU64>,
    buffers: HashMap<u32, Pcm>,
    voices: HashMap<u32, Voice>,
    streams: HashMap<u32, StreamWriter>,
    stream_capacity: HashMap<u32, usize>,
    playing_pcm: HashMap<u32, Pcm>,
    retired_pcm: Vec<(Voice, Pcm)>,
    retired_streams: Vec<(Voice, usize)>,
}
struct Renderer {
    render: Processor,
    head: usize,
    output: Vec<f32>,
    _stack: Vec<u128>,
    callbacks: Arc<AtomicU64>,
}
impl Engine {
    fn collect(&mut self) {
        self.control.collect();
        self.playing_pcm
            .retain(|id, _| self.voices.get(id).is_some_and(|v| !v.finished()));
        self.retired_pcm.retain(|(v, _)| !v.finished());
        self.retired_streams.retain(|(v, _)| !v.finished());
    }
}
#[derive(Default)]
struct State {
    engine: Option<Engine>,
    input: Vec<u8>,
    reply: Vec<u8>,
    error: String,
}
thread_local! { static STATE: RefCell<State> = RefCell::new(State::default()); }

/// Staging memory is owned exclusively by the control instance.
#[unsafe(no_mangle)]
pub extern "C" fn audio_input(size: u32) -> *mut u8 {
    STATE.with_borrow_mut(|state| {
        if size > 32 * 1024 * 1024 {
            return std::ptr::null_mut();
        }
        state.input.resize(size as usize, 0);
        state.input.as_mut_ptr()
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn audio_create(rate: u32, voices: u32, buses: u32, quantum: u32) -> i32 {
    perform(|state| {
        if state.engine.is_some() {
            return Err("audio engine already exists".into());
        }
        if !(1..=8192).contains(&quantum) {
            return Err("unsupported Worklet quantum".into());
        }
        let config = Config {
            sample_rate: rate,
            max_voices: voices as usize,
            buses: buses as usize,
            max_delay_frames: rate as usize,
            ..Config::default()
        };
        let (control, render) = hxn_audio_core::mixer(config)?;
        let stack = vec![0u128; 4096];
        let stack_top = stack.as_ptr() as usize + stack.len() * size_of::<u128>();
        let callbacks = Arc::new(AtomicU64::new(0));
        let renderer = Box::into_raw(Box::new(Renderer {
            render,
            head: 0,
            output: vec![0.0; quantum as usize * 16],
            _stack: stack,
            callbacks: callbacks.clone(),
        })) as usize;
        state.engine = Some(Engine {
            control,
            renderer,
            stack_top,
            callbacks,
            buffers: HashMap::new(),
            voices: HashMap::new(),
            streams: HashMap::new(),
            stream_capacity: HashMap::new(),
            playing_pcm: HashMap::new(),
            retired_pcm: Vec::new(),
            retired_streams: Vec::new(),
        });
        Ok(())
    })
}
fn perform(f: impl FnOnce(&mut State) -> Result<(), String>) -> i32 {
    STATE.with_borrow_mut(|state| match f(state) {
        Ok(()) => {
            state.error.clear();
            1
        }
        Err(error) => {
            state.error = error;
            0
        }
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn audio_load(id: u32, rate: u32, channels: u32) -> i32 {
    perform(|state| {
        let engine = state.engine.as_mut().ok_or("audio engine is absent")?;
        if channels != 1 && channels != 2 {
            return Err("PCM must be mono or stereo".into());
        }
        engine.collect();
        engine
            .playing_pcm
            .retain(|id, _| engine.voices.get(id).is_some_and(|v| !v.finished()));
        let mut allocations = HashSet::new();
        let used: usize = engine
            .buffers
            .values()
            .chain(engine.playing_pcm.values())
            .chain(engine.retired_pcm.iter().map(|(_, pcm)| pcm))
            .filter(|pcm| allocations.insert(pcm.storage_id()))
            .map(Pcm::allocation_bytes)
            .sum();
        if used + state.input.len() / (channels as usize * 4) * 8 > 64 * 1024 * 1024 {
            return Err("Worklet PCM budget exceeds 64 MiB".into());
        }
        let pcm = Pcm::from_le_bytes(&state.input, rate, channels)?;
        if engine.buffers.len() >= 512 && !engine.buffers.contains_key(&id) {
            return Err("Worklet buffer table is full".into());
        }
        engine.buffers.insert(id, pcm);
        Ok(())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn audio_write(id: u32) -> i32 {
    STATE.with_borrow_mut(|state| {
        let Some(writer) = state.engine.as_mut().and_then(|e| e.streams.get_mut(&id)) else {
            state.error = "unknown audio stream".into();
            return -1;
        };
        match writer.write(&state.input) {
            Ok(count) => count as i32,
            Err(error) => {
                state.error = error.into();
                -1
            }
        }
    })
}
fn u(v: &Value, name: &str) -> Result<u32, String> {
    v[name]
        .as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("invalid {name}"))
}
fn f(v: &Value, name: &str) -> Result<f64, String> {
    v[name].as_f64().ok_or_else(|| format!("invalid {name}"))
}
fn options(v: &Value) -> Result<VoiceOptions, String> {
    Ok(VoiceOptions {
        bus: u(v, "bus")? as usize,
        gain: f(v, "gain")?,
        pan: f(v, "pan")?,
        rate: f(v, "rate")?,
        looping: v["looping"].as_bool().unwrap_or(false),
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn audio_command() -> i32 {
    perform(|state| {
        let v: Value = serde_json::from_slice(&state.input).map_err(|e| e.to_string())?;
        let engine = state.engine.as_mut().ok_or("audio engine is absent")?;
        engine.collect();
        let op = v["op"].as_str().ok_or("missing command")?;
        match op {
            "play" | "stream" => {
                let id = u(&v, "id")?;
                if engine.voices.contains_key(&id) || engine.voices.len() >= 512 {
                    return Err("audio voice table is full or duplicate".into());
                }
                let voice = if op == "play" {
                    let pcm = engine
                        .buffers
                        .get(&u(&v, "buffer")?)
                        .ok_or("unknown PCM buffer")?;
                    let voice = engine.control.play(pcm, options(&v)?)?;
                    engine.playing_pcm.insert(id, pcm.clone());
                    voice
                } else {
                    let capacity = u(&v, "capacity")? as usize;
                    if engine.stream_capacity.values().sum::<usize>()
                        + engine
                            .retired_streams
                            .iter()
                            .map(|(_, frames)| frames)
                            .sum::<usize>()
                        + capacity
                        > 4_194_304
                    {
                        return Err("Worklet stream rings exceed 32 MiB".into());
                    }
                    let (voice, writer) = engine.control.stream(
                        u(&v, "capacity")? as usize,
                        u(&v, "sampleRate")?,
                        u(&v, "channels")?,
                        options(&v)?,
                    )?;
                    engine.stream_capacity.insert(id, capacity);
                    engine.streams.insert(id, writer);
                    voice
                };
                engine.voices.insert(id, voice);
            }
            "unload" => {
                engine.buffers.remove(&u(&v, "buffer")?);
            }
            "gain" | "pan" | "rate" | "voiceBus" | "stop" | "forget" | "finish" => {
                let id = u(&v, "id")?;
                let voice = engine.voices.get(&id).ok_or("unknown voice")?;
                match op {
                    "gain" => voice.set_gain(f(&v, "value")?)?,
                    "pan" => voice.set_pan(f(&v, "value")?)?,
                    "rate" => voice.set_rate(f(&v, "value")?)?,
                    "voiceBus" => voice.set_bus(u(&v, "bus")? as usize)?,
                    "stop" => voice.stop(),
                    "finish" => engine.streams.get(&id).ok_or("unknown stream")?.finish(),
                    _ => {
                        voice.stop();
                        if let Some(pcm) = engine.playing_pcm.remove(&id) {
                            engine.retired_pcm.push((voice.clone(), pcm));
                        }
                        if let Some(capacity) = engine.stream_capacity.remove(&id) {
                            engine.retired_streams.push((voice.clone(), capacity));
                        }
                        engine.voices.remove(&id);
                        engine.streams.remove(&id);
                    }
                }
            }
            "busGain" => engine
                .control
                .set_bus_gain(u(&v, "bus")? as usize, f(&v, "value")?)?,
            "busPan" => engine
                .control
                .set_bus_pan(u(&v, "bus")? as usize, f(&v, "value")?)?,
            "lowpass" => engine
                .control
                .set_lowpass(u(&v, "bus")? as usize, f(&v, "value")?)?,
            "delay" => engine.control.set_delay(
                u(&v, "bus")? as usize,
                hxn_audio_core::Delay {
                    frames: u(&v, "frames")? as usize,
                    feedback: f(&v, "feedback")?,
                    wet: f(&v, "wet")?,
                },
            )?,
            "route" => engine
                .control
                .route(u(&v, "bus")? as usize, u(&v, "parent")? as usize)?,
            _ => return Err("unknown audio command".into()),
        }
        Ok(())
    })
}
/// Opaque render ownership is transferred to the Worklet until it stops.
#[unsafe(no_mangle)]
pub extern "C" fn audio_renderer() -> usize {
    STATE.with_borrow(|s| s.engine.as_ref().map_or(0, |e| e.renderer))
}
#[unsafe(no_mangle)]
pub extern "C" fn audio_stack_top() -> usize {
    STATE.with_borrow(|s| s.engine.as_ref().map_or(0, |e| e.stack_top))
}

/// Render touches preallocated data only, without accessing control-side TLS.
///
/// # Safety
/// The address came from audio_renderer in this memory and is used exclusively
/// by one render thread, with its own wasm stack. The owner remains alive until
/// all render calls have stopped; control methods never borrow its pointee.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn audio_render(address: usize, frames: u32) -> *const f32 {
    if address == 0 {
        return std::ptr::null();
    }
    let renderer = unsafe { &mut *(address as *mut Renderer) };
    renderer.callbacks.fetch_add(1, SeqCst);
    let samples = frames as usize * 2;
    if samples > renderer.output.len() / 8 {
        return std::ptr::null();
    }
    if renderer.head + samples > renderer.output.len() {
        renderer.head = 0;
    }
    let start = renderer.head;
    let output = &mut renderer.output[start..start + samples];
    if renderer.render.render(output).is_err() {
        output.fill(0.0);
    }
    renderer.head += samples;
    output.as_ptr()
}

/// # Safety
/// The Worklet has stopped and cannot render again. The page waits for its
/// AudioContext.close() promise before releasing this memory's render owner.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn audio_destroy() {
    STATE.with_borrow_mut(|state| {
        if let Some(engine) = state.engine.take() {
            unsafe {
                drop(Box::from_raw(engine.renderer as *mut Renderer));
            }
            drop(engine);
        }
    });
}
/// Control-side collection and status publication; never called by process().
#[unsafe(no_mangle)]
pub extern "C" fn audio_snapshot() -> *const u8 {
    STATE.with_borrow_mut(|state| {
        let Some(engine) = state.engine.as_mut() else { return std::ptr::null(); };
        engine.collect();
        engine.playing_pcm.retain(|id, _| engine.voices.get(id).is_some_and(|v| !v.finished()));
        let meter = engine.control.meter();
        let voices: Vec<Value> = engine.voices.iter().map(|(id, voice)| json!({ "id": id, "finished": voice.finished(), "position": voice.position_seconds(), "available": engine.streams.get(id).map(StreamWriter::available_frames) })).collect();
        state.reply = serde_json::to_vec(&json!({ "callbacks": engine.callbacks.load(SeqCst), "availableVoices": engine.control.available_voices(), "renderedFrames": meter.rendered_frames, "activeVoices": meter.active_voices, "starvedFrames": meter.starved_frames, "clippedSamples": meter.clipped_samples, "peak": meter.peak, "voices": voices })).unwrap();
        state.reply.as_ptr()
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn audio_reply_len() -> u32 {
    STATE.with_borrow(|s| s.reply.len() as u32)
}
#[unsafe(no_mangle)]
pub extern "C" fn audio_error() -> *const u8 {
    STATE.with_borrow(|s| s.error.as_ptr())
}
#[unsafe(no_mangle)]
pub extern "C" fn audio_error_len() -> u32 {
    STATE.with_borrow(|s| s.error.len() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(bytes: &[u8]) {
        let target = audio_input(bytes.len() as u32);
        assert!(!target.is_null());
        // The staging allocation belongs to this control thread until the next call.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len());
        }
    }
    fn command(value: Value) {
        input(&serde_json::to_vec(&value).unwrap());
        assert_eq!(
            audio_command(),
            1,
            "{}",
            STATE.with_borrow(|s| s.error.clone())
        );
    }
    fn snapshot() -> Value {
        let address = audio_snapshot();
        // No other control call mutates the reply while this slice is borrowed.
        let bytes = unsafe { std::slice::from_raw_parts(address, audio_reply_len() as usize) };
        serde_json::from_slice(bytes).unwrap()
    }
    #[test]
    fn controls_and_rendering_have_separate_ownership_until_shutdown() {
        assert_eq!(audio_create(48_000, 8, 2, 128), 1);
        let renderer = audio_renderer();
        let (send, work) = std::sync::mpsc::channel();
        let (done, rendered) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            while work.recv().is_ok() {
                // This thread alone owns rendering, and the controller is kept alive.
                let output = unsafe { audio_render(renderer, 128) };
                let samples = unsafe { std::slice::from_raw_parts(output, 256) };
                assert!(samples.iter().all(|v| v.is_finite()));
                done.send(samples.iter().any(|v| *v != 0.0)).unwrap();
            }
        });
        let bytes: Vec<u8> = (0..1024).flat_map(|_| 0.1f32.to_le_bytes()).collect();
        input(&bytes);
        assert_eq!(audio_load(1, 48_000, 1), 1);
        for id in 1..=4 {
            command(
                json!({"op":"play","id":id,"buffer":1,"bus":0,"gain":1.0,"pan":0.0,"rate":1.0,"looping":false}),
            );
        }
        command(json!({"op":"unload","buffer":1}));
        let mut audible = false;
        for _ in 0..16 {
            send.send(()).unwrap();
            let _ = snapshot();
            audible |= rendered.recv().unwrap();
        }
        drop(send);
        worker.join().unwrap();
        assert!(audible);
        assert_eq!(snapshot()["activeVoices"], 0);
        assert!(snapshot()["callbacks"].as_u64().unwrap() > 0);
        // Rendering has stopped before ownership returns to the control instance.
        unsafe {
            audio_destroy();
        }
        assert_eq!(audio_renderer(), 0);
    }
}

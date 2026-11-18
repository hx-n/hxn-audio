#![cfg_attr(
    all(target_arch = "wasm32", target_feature = "atomics"),
    feature(stdarch_wasm_atomic_wait)
)]
//! Audio output and real-time mixing. Decoder PCM enters through bounded writes.
use hxn_audio_core::{Config, Controller, Pcm, Processor, StreamWriter, VoiceOptions};
#[cfg(target_family = "wasm")]
mod web;
#[cfg(target_family = "wasm")]
use serde_json::json;
#[cfg(not(target_family = "wasm"))]
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Mutex, Weak};

#[cfg(not(target_family = "wasm"))]
mod native;

hxn::package!("hxn.audio");

#[hxn::data]
#[derive(Clone)]
pub struct AudioOptions {
    #[hxn(default = "AudioOptions::default_rate")]
    pub sample_rate: i32,
    #[hxn(default = "AudioOptions::default_voices")]
    pub voices: i32,
    #[hxn(default = "AudioOptions::default_buses")]
    pub buses: i32,
    #[hxn(default = "AudioOptions::default_buffer")]
    pub buffer_frames: i32,
    #[hxn(default)]
    pub device: String,
}
impl AudioOptions {
    pub fn default_rate() -> i32 {
        48_000
    }
    pub fn default_voices() -> i32 {
        64
    }
    pub fn default_buses() -> i32 {
        8
    }
    pub fn default_buffer() -> i32 {
        256
    }
    fn config(&self) -> Result<Config, String> {
        if !(0..=16_384).contains(&self.buffer_frames) {
            return Err("device buffer must be at most 16384 frames".into());
        }
        Config {
            sample_rate: self.sample_rate as u32,
            max_voices: self.voices as usize,
            buses: self.buses as usize,
            max_delay_frames: self.sample_rate as usize,
            ..Config::default()
        }
        .validate()
        .map_err(String::from)
    }
}

#[hxn::data]
pub struct PlayOptions {
    #[hxn(default)]
    pub bus: i32,
    #[hxn(default = "PlayOptions::unity")]
    pub gain: f64,
    #[hxn(default)]
    pub pan: f64,
    #[hxn(default = "PlayOptions::unity")]
    pub rate: f64,
    #[hxn(default)]
    pub looping: bool,
}
impl PlayOptions {
    pub fn unity() -> f64 {
        1.0
    }
    fn core(&self) -> VoiceOptions {
        VoiceOptions {
            bus: self.bus as usize,
            gain: self.gain,
            pan: self.pan,
            rate: self.rate,
            looping: self.looping,
        }
    }
}

#[hxn::data]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

/// Device events are polled on the application thread. Reopening is explicit.
#[hxn::data]
pub enum AudioDeviceEvent {
    None,
    Changed,
    Unavailable,
    Error,
    PerformanceLimited,
}

#[hxn::data]
pub enum AudioState {
    Offline,
    Initializing,
    Running,
    Suspended,
    Error,
}

#[hxn::data]
pub struct AudioMetrics {
    pub state: AudioState,
    pub error: String,
    pub rendered_frames: u64,
    pub active_voices: u32,
    pub starved_frames: u64,
    pub clipped_samples: u64,
    pub peak: f32,
    pub device: String,
    pub device_rate: u32,
    pub device_channels: u32,
    /// Backend timestamp estimate plus queued renderer frames; zero until known.
    pub latency_seconds: f64,
    pub callbacks: u64,
    pub xruns: u64,
    pub event: AudioDeviceEvent,
}

struct StoredPcm {
    pcm: Pcm,
    _weight: hxn::Weight,
}

#[hxn::interface(shared, weighed)]
pub struct AudioBuffer {
    stored: Arc<StoredPcm>,
}
#[hxn::methods]
impl AudioBuffer {
    /// Little-endian, interleaved finite f32 PCM. Mono and stereo are accepted.
    pub fn new(pcm: &[u8], sample_rate: u32, channels: u32) -> Result<Self, String> {
        let pcm = Pcm::from_le_bytes(pcm, sample_rate, channels).map_err(String::from)?;
        let weight = hxn::Weight::new(pcm.allocation_bytes());
        Ok(Self {
            stored: Arc::new(StoredPcm {
                pcm,
                _weight: weight,
            }),
        })
    }
    pub fn frames(&self) -> u64 {
        self.stored.pcm.frames() as u64
    }
    pub fn sample_rate(&self) -> u32 {
        self.stored.pcm.sample_rate
    }
    pub fn channels(&self) -> u32 {
        self.stored.pcm.channels
    }
}

enum Backend {
    Offline(Box<Processor>),
    #[cfg(not(target_family = "wasm"))]
    Native(native::Output),
    #[cfg(target_family = "wasm")]
    Browser(Arc<web::Context>),
}
struct Retained {
    voice: VoiceControl,
    _pcm: Option<Arc<StoredPcm>>,
    _stream: Option<Arc<hxn::Weight>>,
}
struct Session {
    config: Config,
    parents: Vec<usize>,
    backend: Option<Backend>,
    controller: Controller,
    retained: Vec<Retained>,
    _weight: hxn::Weight,
}
impl Session {
    fn collect(&mut self) -> Result<(), String> {
        #[cfg(target_family = "wasm")]
        if let Some(Backend::Browser(context)) = &self.backend {
            context.poll()?;
        }
        self.controller.collect();
        self.retained.retain(|r| {
            let finished = r.voice.finished();
            #[cfg(target_family = "wasm")]
            if finished && let VoiceControl::Browser(voice) = &r.voice {
                voice.release_if_closed();
            }
            !finished
        });
        #[cfg(target_family = "wasm")]
        if let Some(Backend::Browser(context)) = &self.backend {
            context.collect_buffers()?;
        }
        Ok(())
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.controller.close();
        #[cfg(target_family = "wasm")]
        if let Some(Backend::Browser(context)) = &self.backend {
            context.close();
        }
        drop(self.backend.take());
        self.controller.collect();
        self.retained.clear();
    }
}

#[hxn::interface(shared, weighed)]
pub struct AudioOutput {
    session: Arc<Mutex<Session>>,
}
impl AudioOutput {
    fn with_backend(options: AudioOptions, offline: bool) -> Result<Self, String> {
        let config = options.config()?;
        hxn::log::debug!(target: "hxn.audio", "opening {} output at {} Hz", if offline { "offline" } else { "device" }, config.sample_rate);
        #[cfg(target_family = "wasm")]
        if !offline {
            let context = web::Context::open(&options)?;
            let (controller, _) = hxn_audio_core::mixer(Config {
                max_delay_frames: 1,
                ..config
            })
            .map_err(String::from)?;
            return Ok(Self {
                session: Arc::new(Mutex::new(Session {
                    config,
                    parents: vec![0; config.buses],
                    backend: Some(Backend::Browser(context)),
                    controller,
                    retained: Vec::new(),
                    _weight: hxn::Weight::new(
                        config.buses * (config.max_delay_frames * 20 + 128 * 16)
                            + config.max_voices * 256,
                    ),
                })),
            });
        }
        let (controller, processor) = hxn_audio_core::mixer(config).map_err(String::from)?;
        let backend = if offline {
            Backend::Offline(Box::new(processor))
        } else {
            #[cfg(not(target_family = "wasm"))]
            {
                Backend::Native(native::Output::new(
                    processor,
                    config.sample_rate,
                    &options.device,
                    options.buffer_frames as u32,
                )?)
            }
            #[cfg(target_family = "wasm")]
            {
                let _ = processor;
                return Err("browser output adapter is unavailable".into());
            }
        };
        let weight =
            config.buses * (config.max_delay_frames * 20 + 128 * 16) + config.max_voices * 256;
        Ok(Self {
            session: Arc::new(Mutex::new(Session {
                config,
                parents: vec![0; config.buses],
                backend: Some(backend),
                controller,
                retained: Vec::new(),
                _weight: hxn::Weight::new(weight),
            })),
        })
    }
    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Session>, String> {
        self.session
            .lock()
            .map_err(|_| "audio control lock is poisoned".into())
    }
}
#[hxn::methods]
impl AudioOutput {
    /// Opens the selected device. An empty device identifier selects the default.
    #[hxn(blocking)]
    pub fn new(options: AudioOptions) -> Result<Self, String> {
        Self::with_backend(options, false)
    }
    pub fn offline(options: AudioOptions) -> Result<Self, String> {
        Self::with_backend(options, true)
    }
    pub fn play(&self, buffer: &AudioBuffer, options: PlayOptions) -> Result<AudioVoice, String> {
        let mut session = self.lock()?;
        session.collect()?;
        options
            .core()
            .validate(session.config.buses)
            .map_err(String::from)?;
        #[cfg(target_family = "wasm")]
        let voice = if let Some(Backend::Browser(context)) = &session.backend {
            VoiceControl::Browser(context.play(&buffer.stored, &options)?)
        } else {
            VoiceControl::Local(
                session
                    .controller
                    .play(&buffer.stored.pcm, options.core())
                    .map_err(String::from)?,
            )
        };
        #[cfg(not(target_family = "wasm"))]
        let voice = VoiceControl::Local(
            session
                .controller
                .play(&buffer.stored.pcm, options.core())
                .map_err(String::from)?,
        );
        session.retained.push(Retained {
            voice: voice.clone(),
            _pcm: Some(buffer.stored.clone()),
            _stream: None,
        });
        Ok(AudioVoice {
            voice,
            session: Arc::downgrade(&self.session),
        })
    }
    pub fn stream(
        &self,
        sample_rate: u32,
        channels: u32,
        capacity_frames: u32,
        options: PlayOptions,
    ) -> Result<AudioStream, String> {
        let mut session = self.lock()?;
        session.collect()?;
        options
            .core()
            .validate(session.config.buses)
            .map_err(String::from)?;
        hxn_audio_core::validate_stream(capacity_frames as usize, sample_rate, channels)
            .map_err(String::from)?;
        if options.looping {
            return Err("streams cannot loop; feed another iteration instead".into());
        }
        #[cfg(target_family = "wasm")]
        let (voice, writer) = if let Some(Backend::Browser(context)) = &session.backend {
            (
                VoiceControl::Browser(context.stream(
                    sample_rate,
                    channels,
                    capacity_frames,
                    &options,
                )?),
                None,
            )
        } else {
            let (v, w) = session
                .controller
                .stream(
                    capacity_frames as usize,
                    sample_rate,
                    channels,
                    options.core(),
                )
                .map_err(String::from)?;
            (VoiceControl::Local(v), Some(Mutex::new(w)))
        };
        #[cfg(not(target_family = "wasm"))]
        let (voice, writer) = {
            let (v, w) = session
                .controller
                .stream(
                    capacity_frames as usize,
                    sample_rate,
                    channels,
                    options.core(),
                )
                .map_err(String::from)?;
            (VoiceControl::Local(v), Some(Mutex::new(w)))
        };
        let weight = Arc::new(hxn::Weight::new(capacity_frames as usize * 8));
        session.retained.push(Retained {
            voice: voice.clone(),
            _pcm: None,
            _stream: Some(weight.clone()),
        });
        Ok(AudioStream {
            channels,
            _weight: weight,
            voice: AudioVoice {
                voice,
                session: Arc::downgrade(&self.session),
            },
            writer,
        })
    }
    /// Bus zero is the master; every other bus initially routes to it.
    pub fn bus(&self, index: u32) -> Result<AudioBus, String> {
        if index as usize >= self.lock()?.config.buses {
            return Err("bus is outside this output".into());
        }
        Ok(AudioBus {
            index: index as usize,
            session: Arc::downgrade(&self.session),
        })
    }
    /// Renders an offline output only. Byte length must be a whole stereo frame.
    pub fn render(&self, destination: &mut [u8]) -> Result<(), String> {
        if !destination.len().is_multiple_of(8) {
            return Err("render destination must contain whole stereo f32 frames".into());
        }
        let mut session = self.lock()?;
        let Some(Backend::Offline(processor)) = session.backend.as_mut() else {
            return Err("render is only available on offline outputs".into());
        };
        let mut block = [0.0f32; 256];
        for bytes in destination.chunks_mut(1024) {
            let samples = bytes.len() / 4;
            processor
                .render(&mut block[..samples])
                .map_err(String::from)?;
            for (sample, out) in block[..samples].iter().zip(bytes.as_chunks_mut::<4>().0) {
                *out = sample.to_le_bytes();
            }
        }
        session.collect()?;
        Ok(())
    }
    pub fn metrics(&self) -> Result<AudioMetrics, String> {
        let mut session = self.lock()?;
        session.collect()?;
        let meter = session.controller.meter();
        let mut result = AudioMetrics {
            state: AudioState::Offline,
            error: String::new(),
            rendered_frames: meter.rendered_frames,
            active_voices: meter.active_voices,
            starved_frames: meter.starved_frames,
            clipped_samples: meter.clipped_samples,
            peak: meter.peak,
            device: String::new(),
            device_rate: session.config.sample_rate,
            device_channels: 2,
            latency_seconds: 0.0,
            callbacks: 0,
            xruns: 0,
            event: AudioDeviceEvent::None,
        };
        #[cfg(not(target_family = "wasm"))]
        if let Some(Backend::Native(output)) = &session.backend {
            result.state = if output.paused() {
                AudioState::Suspended
            } else {
                AudioState::Running
            };
            if matches!(output.status.event.load(SeqCst), 2 | 3) {
                result.state = AudioState::Error;
                result.error = "audio device failed; reopen with selectDevice".into();
            }
            result.device = output.device.clone();
            result.device_rate = output.device_rate;
            result.device_channels = output.device_channels;
            result.latency_seconds = output.status.latency_ns.load(SeqCst) as f64 / 1e9;
            result.callbacks = output.status.callbacks.load(SeqCst);
            result.xruns = output.status.xruns.load(SeqCst);
            result.event = match output.status.event.load(SeqCst) {
                0 => AudioDeviceEvent::None,
                1 => AudioDeviceEvent::Changed,
                2 => AudioDeviceEvent::Unavailable,
                4 => AudioDeviceEvent::PerformanceLimited,
                _ => AudioDeviceEvent::Error,
            };
        }
        #[cfg(target_family = "wasm")]
        if let Some(Backend::Browser(context)) = &session.backend {
            let info = context.poll()?;
            result.state = match info["state"].as_str().unwrap_or("") {
                "initializing" => AudioState::Initializing,
                "running" => AudioState::Running,
                "error" => AudioState::Error,
                _ => AudioState::Suspended,
            };
            result.error = info["error"].as_str().unwrap_or("").into();
            result.rendered_frames = info["renderedFrames"].as_u64().unwrap_or(0);
            result.active_voices = info["activeVoices"].as_u64().unwrap_or(0) as u32;
            result.starved_frames = info["starvedFrames"].as_u64().unwrap_or(0);
            result.clipped_samples = info["clippedSamples"].as_u64().unwrap_or(0);
            result.peak = info["peak"].as_f64().unwrap_or(0.0) as f32;
            result.device = info["device"].as_str().unwrap_or("").into();
            result.device_rate = info["deviceRate"].as_u64().unwrap_or(0) as u32;
            result.callbacks = info["callbacks"].as_u64().unwrap_or(0);
            result.event = match info["event"].as_u64().unwrap_or(0) {
                0 => AudioDeviceEvent::None,
                1 => AudioDeviceEvent::Changed,
                2 => AudioDeviceEvent::Unavailable,
                4 => AudioDeviceEvent::PerformanceLimited,
                _ => AudioDeviceEvent::Error,
            };
            result.latency_seconds = info["latencySeconds"].as_f64().unwrap_or(0.0);
        }
        Ok(result)
    }
    pub fn pause(&self) -> Result<(), String> {
        let mut session = self.lock()?;
        match session.backend.as_mut() {
            #[cfg(not(target_family = "wasm"))]
            Some(Backend::Native(output)) => output.pause(),
            #[cfg(target_family = "wasm")]
            Some(Backend::Browser(context)) => context.command(json!({"op":"pause"})).map(|_| ()),
            _ => Err("pause requires a device output".into()),
        }
    }
    pub fn resume(&self) -> Result<(), String> {
        let mut session = self.lock()?;
        match session.backend.as_mut() {
            #[cfg(not(target_family = "wasm"))]
            Some(Backend::Native(output)) => output.resume(),
            #[cfg(target_family = "wasm")]
            Some(Backend::Browser(context)) => context.command(json!({"op":"resume"})).map(|_| ()),
            _ => Err("resume requires a device output".into()),
        }
    }
    /// Reopens on the control thread, retaining the mixer and its queued audio.
    #[hxn(blocking)]
    pub fn select_device(&self, id: &str) -> Result<(), String> {
        let mut session = self.lock()?;
        match session.backend.as_mut() {
            #[cfg(not(target_family = "wasm"))]
            Some(Backend::Native(output)) => output.select(id),
            #[cfg(target_family = "wasm")]
            Some(Backend::Browser(context)) => context
                .command(json!({"op":"select", "device":id}))
                .map(|_| ()),
            _ => {
                let _ = id;
                Err("device selection requires a device output".into())
            }
        }
    }
}

#[derive(Clone)]
enum VoiceControl {
    Local(hxn_audio_core::Voice),
    #[cfg(target_family = "wasm")]
    Browser(web::Voice),
}
impl VoiceControl {
    fn stop(&self) {
        match self {
            Self::Local(v) => v.stop(),
            #[cfg(target_family = "wasm")]
            Self::Browser(v) => {
                let _ = v.command("stop", 0.0);
            }
        }
    }
    fn finished(&self) -> bool {
        match self {
            Self::Local(v) => v.finished(),
            #[cfg(target_family = "wasm")]
            Self::Browser(v) => v.finished(),
        }
    }
    fn set_gain(&self, value: f64) -> Result<(), String> {
        finite(value, 0.0, 16.0)?;
        match self {
            Self::Local(v) => v.set_gain(value).map_err(String::from),
            #[cfg(target_family = "wasm")]
            Self::Browser(v) => v.command("gain", value),
        }
    }
    fn set_pan(&self, value: f64) -> Result<(), String> {
        finite(value, -1.0, 1.0)?;
        match self {
            Self::Local(v) => v.set_pan(value).map_err(String::from),
            #[cfg(target_family = "wasm")]
            Self::Browser(v) => v.command("pan", value),
        }
    }
    fn set_rate(&self, value: f64) -> Result<(), String> {
        finite(value, 0.01, 8.0)?;
        match self {
            Self::Local(v) => v.set_rate(value).map_err(String::from),
            #[cfg(target_family = "wasm")]
            Self::Browser(v) => v.command("rate", value),
        }
    }
    fn position_seconds(&self) -> f64 {
        match self {
            Self::Local(v) => v.position_seconds(),
            #[cfg(target_family = "wasm")]
            Self::Browser(v) => v.cached()["position"].as_f64().unwrap_or(0.0),
        }
    }
}
fn finite(value: f64, min: f64, max: f64) -> Result<(), String> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(())
    } else {
        Err(format!("value must be finite and in {min}..={max}"))
    }
}

#[hxn::interface(shared)]
pub struct AudioVoice {
    voice: VoiceControl,
    session: Weak<Mutex<Session>>,
}
#[hxn::methods]
impl AudioVoice {
    pub fn stop(&self) {
        self.voice.stop();
    }
    pub fn set_gain(&self, gain: f64) -> Result<(), String> {
        self.voice.set_gain(gain)
    }
    pub fn set_pan(&self, pan: f64) -> Result<(), String> {
        self.voice.set_pan(pan)
    }
    pub fn set_rate(&self, rate: f64) -> Result<(), String> {
        self.voice.set_rate(rate)
    }
    pub fn finished(&self) -> Result<bool, String> {
        if self.session.strong_count() == 0 {
            return Ok(true);
        }
        #[cfg(target_family = "wasm")]
        if let VoiceControl::Browser(v) = &self.voice {
            v.context.poll()?;
        }
        Ok(self.voice.finished())
    }
    pub fn set_bus(&self, bus: &AudioBus) -> Result<(), String> {
        if !Weak::ptr_eq(&self.session, &bus.session) {
            return Err("voice and bus belong to different outputs".into());
        }
        match &self.voice {
            VoiceControl::Local(v) => v.set_bus(bus.index).map_err(String::from),
            #[cfg(target_family = "wasm")]
            VoiceControl::Browser(v) => v
                .context
                .command(json!({"op":"voiceBus", "id":v.id, "bus":bus.index}))
                .map(|_| ()),
        }
    }
    pub fn position_seconds(&self) -> f64 {
        self.voice.position_seconds()
    }
}
impl Drop for AudioVoice {
    fn drop(&mut self) {
        self.voice.stop();
        #[cfg(target_family = "wasm")]
        if let VoiceControl::Browser(v) = &self.voice {
            v.close();
        }
    }
}

#[hxn::interface(shared, weighed)]
pub struct AudioStream {
    channels: u32,
    _weight: Arc<hxn::Weight>,
    voice: AudioVoice,
    writer: Option<Mutex<StreamWriter>>,
}
#[hxn::methods]
impl AudioStream {
    /// Returns accepted frames. Retry the remaining suffix without blocking.
    pub fn write(&self, pcm: &[u8]) -> Result<u32, String> {
        hxn_audio_core::validate_pcm(pcm, self.channels).map_err(String::from)?;
        if self.voice.finished()? {
            return Err("audio stream has ended".into());
        }
        #[cfg(target_family = "wasm")]
        if let VoiceControl::Browser(v) = &self.voice.voice {
            if pcm.len() > 32 * 1024 * 1024 {
                return Err("stream write exceeds 32 MiB".into());
            }
            return v.write(pcm);
        }
        self.writer
            .as_ref()
            .ok_or("stream writer is absent")?
            .lock()
            .map_err(|_| "stream writer lock is poisoned")?
            .write(pcm)
            .map(|n| n as u32)
            .map_err(String::from)
    }
    pub fn available_frames(&self) -> Result<u32, String> {
        #[cfg(target_family = "wasm")]
        if let VoiceControl::Browser(v) = &self.voice.voice {
            return v.available();
        }
        Ok(self
            .writer
            .as_ref()
            .ok_or("stream writer is absent")?
            .lock()
            .map_err(|_| "stream writer lock is poisoned")?
            .available_frames() as u32)
    }
    /// Marks EOF after the last successful write. Queued audio continues playing.
    pub fn finish(&self) -> Result<(), String> {
        #[cfg(target_family = "wasm")]
        if let VoiceControl::Browser(v) = &self.voice.voice {
            return v.command("finish", 0.0);
        }
        self.writer
            .as_ref()
            .ok_or("stream writer is absent")?
            .lock()
            .map_err(|_| "stream writer lock is poisoned")?
            .finish();
        Ok(())
    }
    pub fn stop(&self) {
        self.voice.stop();
    }
    pub fn finished(&self) -> Result<bool, String> {
        self.voice.finished()
    }
    pub fn set_gain(&self, gain: f64) -> Result<(), String> {
        self.voice.set_gain(gain)
    }
    pub fn set_pan(&self, pan: f64) -> Result<(), String> {
        self.voice.set_pan(pan)
    }
    pub fn position_seconds(&self) -> f64 {
        self.voice.position_seconds()
    }
}

#[hxn::interface(shared)]
pub struct AudioBus {
    index: usize,
    session: Weak<Mutex<Session>>,
}
enum BusUpdate {
    Gain(f64),
    Pan(f64),
    Lowpass(f64),
    Delay(u32, f64, f64),
    Route(usize),
}
impl AudioBus {
    fn with(&self, update: BusUpdate) -> Result<(), String> {
        let session = self.session.upgrade().ok_or("audio output is closed")?;
        let mut session = session
            .lock()
            .map_err(|_| "audio control lock is poisoned")?;
        match update {
            BusUpdate::Gain(v) => finite(v, 0.0, 16.0)?,
            BusUpdate::Pan(v) => finite(v, -1.0, 1.0)?,
            BusUpdate::Lowpass(v) => finite(v, 0.0, f64::from(session.config.sample_rate) * 0.5)?,
            BusUpdate::Delay(frames, feedback, wet) => {
                if frames as usize > session.config.max_delay_frames {
                    return Err("delay exceeds this output's preallocated capacity".into());
                }
                finite(feedback, -0.99, 0.99)?;
                finite(wet, 0.0, 1.0)?;
            }
            BusUpdate::Route(parent) => {
                if self.index == 0 {
                    return Err("master bus cannot be routed".into());
                }
                let mut cursor = parent;
                loop {
                    if cursor == self.index {
                        return Err("bus routing would form a cycle".into());
                    }
                    if cursor == 0 {
                        break;
                    }
                    cursor = session.parents[cursor];
                }
            }
        }
        #[cfg(target_family = "wasm")]
        if let Some(Backend::Browser(context)) = &session.backend {
            let mut command = match update {
                BusUpdate::Gain(v) => json!({"op":"busGain","value":v}),
                BusUpdate::Pan(v) => json!({"op":"busPan","value":v}),
                BusUpdate::Lowpass(v) => json!({"op":"lowpass","value":v}),
                BusUpdate::Delay(frames, feedback, wet) => {
                    json!({"op":"delay","frames":frames,"feedback":feedback,"wet":wet})
                }
                BusUpdate::Route(parent) => json!({"op":"route","parent":parent}),
            };
            command["bus"] = self.index.into();
            context.command(command)?;
            if let BusUpdate::Route(parent) = update {
                session.parents[self.index] = parent;
            }
            return Ok(());
        }
        match update {
            BusUpdate::Gain(v) => session.controller.set_bus_gain(self.index, v),
            BusUpdate::Pan(v) => session.controller.set_bus_pan(self.index, v),
            BusUpdate::Lowpass(v) => session.controller.set_lowpass(self.index, v),
            BusUpdate::Delay(frames, feedback, wet) => session.controller.set_delay(
                self.index,
                hxn_audio_core::Delay {
                    frames: frames as usize,
                    feedback,
                    wet,
                },
            ),
            BusUpdate::Route(parent) => {
                session
                    .controller
                    .route(self.index, parent)
                    .map_err(String::from)?;
                session.parents[self.index] = parent;
                Ok(())
            }
        }
        .map_err(String::from)
    }
}
#[hxn::methods]
impl AudioBus {
    pub fn set_gain(&self, gain: f64) -> Result<(), String> {
        self.with(BusUpdate::Gain(gain))
    }
    pub fn set_pan(&self, pan: f64) -> Result<(), String> {
        self.with(BusUpdate::Pan(pan))
    }
    /// Zero bypasses this filter. Cutoff may not exceed the mixer Nyquist rate.
    pub fn set_lowpass(&self, hz: f64) -> Result<(), String> {
        self.with(BusUpdate::Lowpass(hz))
    }
    pub fn set_delay(&self, frames: u32, feedback: f64, wet: f64) -> Result<(), String> {
        self.with(BusUpdate::Delay(frames, feedback, wet))
    }
    pub fn route_to(&self, parent: &AudioBus) -> Result<(), String> {
        if !Weak::ptr_eq(&self.session, &parent.session) {
            return Err("buses belong to different outputs".into());
        }
        self.with(BusUpdate::Route(parent.index))
    }
}
#[hxn::function]
pub fn devices() -> Result<Vec<AudioDevice>, String> {
    #[cfg(not(target_family = "wasm"))]
    {
        native::devices().map(|list| {
            list.into_iter()
                .map(|d| AudioDevice {
                    id: d.id,
                    name: d.name,
                    is_default: d.default,
                })
                .collect()
        })
    }
    #[cfg(target_family = "wasm")]
    {
        web::devices()
    }
}

use crate::{
    MixFrame, Pcm, Result, StreamWriter, finite,
    source::{self, Read, Source},
};
use rtrb::{Consumer, Producer, PushError, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::SeqCst},
};

const BLOCK: usize = 128;
const MAX_BUSES: usize = 64;
static NEXT_OWNER: AtomicU32 = AtomicU32::new(1);

#[derive(Clone, Copy, Debug)]
pub struct Config {
    pub sample_rate: u32,
    pub channels: u32,
    pub max_voices: usize,
    pub buses: usize,
    pub command_capacity: usize,
    pub max_delay_frames: usize,
    pub ramp_frames: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            max_voices: 64,
            buses: 8,
            command_capacity: 256,
            max_delay_frames: 48_000,
            ramp_frames: 32,
        }
    }
}

impl Config {
    pub fn validate(self) -> Result<Self> {
        if !(8_000..=192_000).contains(&self.sample_rate) {
            return Err("sample rate must be 8000..=192000 Hz");
        }
        if self.channels != 1 && self.channels != 2 {
            return Err("output must have one or two channels");
        }
        if !(1..=512).contains(&self.max_voices) {
            return Err("voice capacity must be 1..=512");
        }
        if !(1..=MAX_BUSES).contains(&self.buses) {
            return Err("bus count must be 1..=64");
        }
        if !(1..=4096).contains(&self.command_capacity) {
            return Err("command capacity must be 1..=4096");
        }
        if self.max_delay_frames == 0 || self.max_delay_frames > self.sample_rate as usize * 2 {
            return Err("delay capacity must be positive and at most two seconds");
        }
        if self.ramp_frames > 4096 {
            return Err("gain ramps must be at most 4096 frames");
        }
        if self.buses * self.max_delay_frames * (size_of::<MixFrame>() + size_of::<u32>())
            > 64 * 1024 * 1024
        {
            return Err("delay storage exceeds 64 MiB; reduce the bus count or delay capacity");
        }
        Ok(self)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct VoiceOptions {
    pub bus: usize,
    pub gain: f64,
    pub pan: f64,
    pub rate: f64,
    pub looping: bool,
}
impl Default for VoiceOptions {
    fn default() -> Self {
        Self {
            bus: 0,
            gain: 1.0,
            pan: 0.0,
            rate: 1.0,
            looping: false,
        }
    }
}
impl VoiceOptions {
    pub fn validate(self, buses: usize) -> Result<Self> {
        if self.bus >= buses {
            return Err("voice bus is outside this mixer");
        }
        gain(self.gain)?;
        pan(self.pan)?;
        rate(self.rate)?;
        Ok(self)
    }
}

fn gain(value: f64) -> Result<f64> {
    finite(value, 0.0, 16.0, "gain must be finite and in 0..=16")
}
fn pan(value: f64) -> Result<f64> {
    finite(value, -1.0, 1.0, "pan must be finite and in -1..=1")
}
fn rate(value: f64) -> Result<f64> {
    finite(
        value,
        0.01,
        8.0,
        "playback rate must be finite and in 0.01..=8",
    )
}

struct VoiceState {
    owner: u32,
    bus: AtomicU32,
    buses: u32,
    gain: AtomicU32,
    pan: AtomicU32,
    rate: AtomicU32,
    stop: AtomicBool,
    finished: AtomicBool,
    position: AtomicU64,
}

/// A control handle. Its methods never touch the render thread's mutable state.
#[derive(Clone)]
pub struct Voice {
    state: Arc<VoiceState>,
    source_rate: u32,
}
impl Voice {
    pub fn set_bus(&self, bus: usize) -> Result<()> {
        if bus >= self.state.buses as usize {
            return Err("voice bus is outside this mixer");
        }
        self.state.bus.store(bus as u32, SeqCst);
        Ok(())
    }
    pub fn set_gain(&self, value: f64) -> Result<()> {
        gain(value)?;
        self.state.gain.store((value as f32).to_bits(), SeqCst);
        Ok(())
    }
    pub fn set_pan(&self, value: f64) -> Result<()> {
        pan(value)?;
        self.state.pan.store((value as f32).to_bits(), SeqCst);
        Ok(())
    }
    pub fn set_rate(&self, value: f64) -> Result<()> {
        rate(value)?;
        self.state.rate.store((value as f32).to_bits(), SeqCst);
        Ok(())
    }
    pub fn stop(&self) {
        self.state.stop.store(true, SeqCst);
    }
    pub fn finished(&self) -> bool {
        self.state.finished.load(SeqCst)
    }
    pub fn position_seconds(&self) -> f64 {
        self.state.position.load(SeqCst) as f64 / f64::from(self.source_rate)
    }
    pub fn owner(&self) -> u32 {
        self.state.owner
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Delay {
    pub frames: usize,
    pub feedback: f64,
    pub wet: f64,
}

#[derive(Clone, Copy)]
struct BusSettings {
    gain: f64,
    pan: f64,
    lowpass: f64,
    delay: Delay,
    epoch: u32,
}
impl Default for BusSettings {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            lowpass: 1.0,
            delay: Delay::default(),
            epoch: 1,
        }
    }
}

#[derive(Default)]
struct SharedMeter {
    rendered: AtomicU64,
    active: AtomicU32,
    starved: AtomicU64,
    clipped: AtomicU64,
    peak: AtomicU32,
    closed: AtomicBool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Meter {
    pub rendered_frames: u64,
    pub active_voices: u32,
    pub starved_frames: u64,
    pub clipped_samples: u64,
    pub peak: f32,
}

impl SharedMeter {
    fn read(&self) -> Meter {
        Meter {
            rendered_frames: self.rendered.load(SeqCst),
            active_voices: self.active.load(SeqCst),
            starved_frames: self.starved.load(SeqCst),
            clipped_samples: self.clipped.load(SeqCst),
            peak: f32::from_bits(self.peak.load(SeqCst)),
        }
    }
}

struct Playing {
    source: Source,
    state: Arc<VoiceState>,
    level: Ramp,
    ended: bool,
}
struct Retired {
    slot: usize,
    voice: Playing,
}

enum Command {
    Play {
        slot: usize,
        voice: Playing,
    },
    Bus {
        index: usize,
        settings: BusSettings,
    },
    Route {
        parents: [u8; MAX_BUSES],
        order: [u8; MAX_BUSES],
    },
}

/// The non-real-time owner of command production and deferred destruction.
pub struct Controller {
    config: Config,
    owner: u32,
    commands: Producer<Command>,
    retired: Consumer<Retired>,
    free: Vec<usize>,
    meter: Arc<SharedMeter>,
    settings: [BusSettings; MAX_BUSES],
    parents: [u8; MAX_BUSES],
}

impl Controller {
    pub fn config(&self) -> Config {
        self.config
    }
    pub fn meter(&self) -> Meter {
        self.meter.read()
    }
    pub fn owner(&self) -> u32 {
        self.owner
    }
    pub fn available_voices(&self) -> usize {
        self.free.len()
    }
    pub fn collect(&mut self) {
        while let Ok(retired) = self.retired.pop() {
            self.free.push(retired.slot);
            drop(retired.voice);
        }
    }
    fn send(&mut self, command: Command) -> Result<()> {
        self.collect();
        if self.meter.closed.load(SeqCst) {
            return Err("mixer is closed");
        }
        self.commands
            .push(command)
            .map_err(|_| "audio command queue is full")
    }
    pub fn play(&mut self, pcm: &Pcm, options: VoiceOptions) -> Result<Voice> {
        let options = options.validate(self.config.buses)?;
        self.play_source(
            Source::Clip {
                pcm: pcm.clone(),
                position: 0.0,
                looping: options.looping,
            },
            options,
        )
    }
    pub fn stream(
        &mut self,
        capacity: usize,
        sample_rate: u32,
        channels: u32,
        options: VoiceOptions,
    ) -> Result<(Voice, StreamWriter)> {
        let options = options.validate(self.config.buses)?;
        if options.looping {
            return Err("streams cannot loop; feed another iteration instead");
        }
        let (writer, source) = source::stream(capacity, sample_rate, channels)?;
        let voice = self.play_source(source, options)?;
        Ok((voice, writer))
    }
    fn play_source(&mut self, source: Source, options: VoiceOptions) -> Result<Voice> {
        self.collect();
        if self.meter.closed.load(SeqCst) {
            return Err("mixer is closed");
        }
        let slot = self.free.pop().ok_or("mixer has no free voice slots")?;
        let state = Arc::new(VoiceState {
            owner: self.owner,
            bus: AtomicU32::new(options.bus as u32),
            buses: self.config.buses as u32,
            gain: AtomicU32::new((options.gain as f32).to_bits()),
            pan: AtomicU32::new((options.pan as f32).to_bits()),
            rate: AtomicU32::new((options.rate as f32).to_bits()),
            stop: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            position: AtomicU64::new(0),
        });
        let handle = Voice {
            state: state.clone(),
            source_rate: source.sample_rate(),
        };
        let voice = Playing {
            source,
            state,
            level: Ramp::new(if self.config.ramp_frames == 0 {
                options.gain
            } else {
                0.0
            }),
            ended: false,
        };
        if let Err(PushError::Full(_)) = self.commands.push(Command::Play { slot, voice }) {
            self.free.push(slot);
            return Err("audio command queue is full");
        }
        Ok(handle)
    }
    pub fn set_bus_gain(&mut self, bus: usize, value: f64) -> Result<()> {
        self.check_bus(bus)?;
        let mut settings = self.settings[bus];
        settings.gain = gain(value)?;
        self.bus(bus, settings)
    }
    pub fn set_bus_pan(&mut self, bus: usize, value: f64) -> Result<()> {
        self.check_bus(bus)?;
        let mut settings = self.settings[bus];
        settings.pan = pan(value)?;
        self.bus(bus, settings)
    }
    pub fn set_lowpass(&mut self, bus: usize, hz: f64) -> Result<()> {
        self.check_bus(bus)?;
        finite(
            hz,
            0.0,
            f64::from(self.config.sample_rate) * 0.5,
            "cutoff must be finite and within the output Nyquist frequency",
        )?;
        let mut settings = self.settings[bus];
        settings.lowpass = if hz == 0.0 {
            1.0
        } else {
            1.0 - (-std::f64::consts::TAU * hz / f64::from(self.config.sample_rate)).exp()
        };
        self.bus(bus, settings)
    }
    pub fn set_delay(&mut self, bus: usize, delay: Delay) -> Result<()> {
        self.check_bus(bus)?;
        if delay.frames > self.config.max_delay_frames {
            return Err("delay exceeds this mixer's preallocated capacity");
        }
        finite(
            delay.feedback,
            -0.99,
            0.99,
            "delay feedback must be finite and in -0.99..=0.99",
        )?;
        finite(delay.wet, 0.0, 1.0, "delay mix must be finite and in 0..=1")?;
        let mut settings = self.settings[bus];
        if settings.delay.frames != delay.frames || (settings.delay.wet == 0.0 && delay.wet > 0.0) {
            settings.epoch = settings
                .epoch
                .checked_add(1)
                .ok_or("delay update generation exhausted")?;
        }
        settings.delay = delay;
        self.bus(bus, settings)
    }
    fn check_bus(&self, bus: usize) -> Result<()> {
        if bus < self.config.buses {
            Ok(())
        } else {
            Err("bus is outside this mixer")
        }
    }
    fn bus(&mut self, index: usize, settings: BusSettings) -> Result<()> {
        self.send(Command::Bus { index, settings })?;
        self.settings[index] = settings;
        Ok(())
    }
    pub fn route(&mut self, bus: usize, parent: usize) -> Result<()> {
        self.check_bus(bus)?;
        self.check_bus(parent)?;
        if bus == 0 {
            return Err("the master bus cannot be routed");
        }
        let mut parents = self.parents;
        parents[bus] = parent as u8;
        let mut cursor = parent;
        for _ in 0..self.config.buses {
            if cursor == bus {
                return Err("bus routing would form a cycle");
            }
            if cursor == 0 {
                break;
            }
            cursor = parents[cursor] as usize;
        }
        let mut order = [0u8; MAX_BUSES];
        for (i, value) in order.iter_mut().enumerate().take(self.config.buses) {
            *value = i as u8;
        }
        order[..self.config.buses].sort_unstable_by_key(|&id| {
            let mut depth = 0;
            let mut node = id as usize;
            while node != 0 {
                depth += 1;
                node = parents[node] as usize;
            }
            std::cmp::Reverse(depth)
        });
        self.send(Command::Route { parents, order })?;
        self.parents = parents;
        Ok(())
    }
    pub fn close(&self) {
        self.meter.closed.store(true, SeqCst);
    }
}

impl Drop for Controller {
    fn drop(&mut self) {
        self.close();
    }
}

struct Bus {
    settings: BusSettings,
    level: Ramp,
    filtered: MixFrame,
    delay: Vec<MixFrame>,
    tags: Vec<u32>,
    head: usize,
}

impl Bus {
    fn process(&mut self, mut frame: MixFrame, ramp: u32) -> MixFrame {
        let level = self.level.next(self.settings.gain, ramp);
        let balance = pan_gains(self.settings.pan, true);
        for channel in 0..2 {
            let input = frame[channel] * level * balance[channel];
            if self.settings.lowpass == 1.0 {
                self.filtered[channel] = input;
            } else {
                self.filtered[channel] += self.settings.lowpass * (input - self.filtered[channel]);
            }
            frame[channel] = self.filtered[channel];
        }
        let effect = self.settings.delay;
        if effect.frames > 0 && effect.wet > 0.0 {
            let index = (self.head + self.delay.len() - effect.frames) % self.delay.len();
            let delayed = if self.tags[index] == self.settings.epoch {
                self.delay[index]
            } else {
                [0.0; 2]
            };
            for channel in 0..2 {
                self.delay[self.head][channel] =
                    frame[channel] + delayed[channel] * effect.feedback;
                frame[channel] =
                    frame[channel] * (1.0 - effect.wet) + delayed[channel] * effect.wet;
            }
            self.tags[self.head] = self.settings.epoch;
            self.head = (self.head + 1) % self.delay.len();
        }
        frame
    }
}

/// Owned by the audio callback. All buffers and tables are sized at creation.
pub struct Processor {
    config: Config,
    commands: Consumer<Command>,
    retired: Producer<Retired>,
    meter: Arc<SharedMeter>,
    voices: Vec<Option<Playing>>,
    buses: Vec<Bus>,
    parents: [u8; MAX_BUSES],
    order: [u8; MAX_BUSES],
    scratch: Vec<MixFrame>,
}

impl Processor {
    pub fn config(&self) -> Config {
        self.config
    }
    pub fn render(&mut self, output: &mut [f32]) -> Result<()> {
        let channels = self.config.channels as usize;
        if !output.len().is_multiple_of(channels) {
            return Err("output contains an incomplete frame");
        }
        for block in output.chunks_mut(BLOCK * channels) {
            self.block(block);
        }
        Ok(())
    }
    fn commands(&mut self) {
        for _ in 0..self.config.command_capacity.min(64) {
            let Ok(command) = self.commands.pop() else {
                break;
            };
            match command {
                Command::Play { slot, voice } => {
                    debug_assert!(self.voices[slot].is_none());
                    self.voices[slot] = Some(voice);
                }
                Command::Bus { index, settings } => self.buses[index].settings = settings,
                Command::Route { parents, order } => {
                    self.parents = parents;
                    self.order = order;
                }
            }
        }
    }
    fn block(&mut self, output: &mut [f32]) {
        self.commands();
        let frames = output.len() / self.config.channels as usize;
        self.scratch.fill([0.0; 2]);
        let mut active = 0;
        let mut starved = 0;
        for slot in 0..self.voices.len() {
            let Some(voice) = self.voices[slot].as_mut() else {
                continue;
            };
            if !voice.ended {
                let bus = voice.state.bus.load(SeqCst) as usize;
                let stopping = voice.state.stop.load(SeqCst) || self.meter.closed.load(SeqCst);
                let target = if stopping {
                    0.0
                } else {
                    f64::from(f32::from_bits(voice.state.gain.load(SeqCst)))
                };
                let balance = pan_gains(
                    f64::from(f32::from_bits(voice.state.pan.load(SeqCst))),
                    voice.source.channels() == 2,
                );
                let step = f64::from(voice.source.sample_rate())
                    / f64::from(self.config.sample_rate)
                    * f64::from(f32::from_bits(voice.state.rate.load(SeqCst)));
                for index in 0..frames {
                    let level = voice.level.next(target, self.config.ramp_frames);
                    if stopping && level == 0.0 {
                        voice.ended = true;
                        break;
                    }
                    match voice.source.next(step) {
                        Read::Frame(frame) => {
                            for channel in 0..2 {
                                self.scratch[bus * BLOCK + index][channel] +=
                                    frame[channel] * level * balance[channel];
                            }
                        }
                        Read::Pending => starved += 1,
                        Read::End => {
                            voice.ended = true;
                            break;
                        }
                    }
                }
                voice.state.position.store(voice.source.position(), SeqCst);
            }
            if voice.ended {
                voice.state.finished.store(true, SeqCst);
                let voice = self.voices[slot].take().unwrap();
                if let Err(PushError::Full(retired)) = self.retired.push(Retired { slot, voice }) {
                    // Keep ownership until the control side can retire it. Never
                    // destroy the last PCM/stream reference in the render call.
                    self.voices[slot] = Some(retired.voice);
                }
            } else {
                active += 1;
            }
        }
        let mut clipped = 0;
        let mut peak = 0.0f32;
        for &id in &self.order[..self.config.buses] {
            let bus = id as usize;
            for index in 0..frames {
                let frame = self.buses[bus]
                    .process(self.scratch[bus * BLOCK + index], self.config.ramp_frames);
                if bus != 0 {
                    let destination = self.parents[bus] as usize * BLOCK + index;
                    for channel in 0..2 {
                        self.scratch[destination][channel] += frame[channel];
                    }
                } else {
                    for channel in 0..self.config.channels as usize {
                        let value = if self.config.channels == 1 {
                            (frame[0] + frame[1]) * std::f64::consts::FRAC_1_SQRT_2
                        } else {
                            frame[channel]
                        };
                        if value.abs() > 1.0 {
                            clipped += 1;
                        }
                        let value = value.clamp(-1.0, 1.0) as f32;
                        peak = peak.max(value.abs());
                        output[index * self.config.channels as usize + channel] = value;
                    }
                }
            }
        }
        self.meter.rendered.fetch_add(frames as u64, SeqCst);
        self.meter.active.store(active, SeqCst);
        self.meter.starved.fetch_add(starved, SeqCst);
        self.meter.clipped.fetch_add(clipped, SeqCst);
        self.meter.peak.store(peak.to_bits(), SeqCst);
    }
}

struct Ramp {
    value: f64,
    target: f64,
    step: f64,
    remaining: u32,
}
impl Ramp {
    fn new(value: f64) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }
    fn next(&mut self, target: f64, frames: u32) -> f64 {
        if target != self.target {
            self.target = target;
            self.remaining = frames;
            self.step = if frames == 0 {
                0.0
            } else {
                (target - self.value) / f64::from(frames)
            };
        }
        if self.remaining == 0 {
            self.value = self.target;
        } else {
            self.value += self.step;
            self.remaining -= 1;
            if self.remaining == 0 {
                self.value = self.target;
            }
        }
        self.value
    }
}

fn pan_gains(pan: f64, stereo: bool) -> MixFrame {
    let angle = (pan + 1.0) * std::f64::consts::FRAC_PI_4;
    let scale = if stereo {
        std::f64::consts::SQRT_2
    } else {
        1.0
    };
    [angle.cos() * scale, angle.sin() * scale]
}

pub fn mixer(config: Config) -> Result<(Controller, Processor)> {
    let config = config.validate()?;
    let mut owner = NEXT_OWNER.load(SeqCst);
    loop {
        let next = owner.checked_add(1).ok_or("mixer identifiers exhausted")?;
        match NEXT_OWNER.compare_exchange(owner, next, SeqCst, SeqCst) {
            Ok(_) => break,
            Err(actual) => owner = actual,
        }
    }
    let (commands, receive) = RingBuffer::new(config.command_capacity);
    let (retire, retired) = RingBuffer::new(config.max_voices);
    let meter = Arc::new(SharedMeter::default());
    let mut buses = Vec::with_capacity(config.buses);
    for _ in 0..config.buses {
        buses.push(Bus {
            settings: BusSettings::default(),
            level: Ramp::new(1.0),
            filtered: [0.0; 2],
            delay: vec![[0.0; 2]; config.max_delay_frames],
            tags: vec![0; config.max_delay_frames],
            head: 0,
        });
    }
    let mut order = [0; MAX_BUSES];
    for (index, value) in order.iter_mut().enumerate().take(config.buses) {
        *value = (config.buses - 1 - index) as u8;
    }
    let controller = Controller {
        config,
        owner,
        commands,
        retired,
        free: (0..config.max_voices).rev().collect(),
        meter: meter.clone(),
        settings: [BusSettings::default(); MAX_BUSES],
        parents: [0; MAX_BUSES],
    };
    let processor = Processor {
        config,
        commands: receive,
        retired: retire,
        meter,
        voices: std::iter::repeat_with(|| None)
            .take(config.max_voices)
            .collect(),
        buses,
        parents: [0; MAX_BUSES],
        order,
        scratch: vec![[0.0; 2]; BLOCK * config.buses],
    };
    Ok((controller, processor))
}

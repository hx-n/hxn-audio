//! Device ownership stays on the control side; only the renderer enters callbacks.
use cpal::{
    FromSample, SampleFormat, SizedSample, StreamConfig,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use hxn_audio_core::Processor;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicU32, AtomicU64, Ordering::SeqCst},
};
use std::time::Duration;

#[derive(Default)]
pub struct Status {
    pub event: AtomicU32,
    pub xruns: AtomicU64,
    pub latency_ns: AtomicU64,
    pub callbacks: AtomicU64,
}

pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}
pub fn devices() -> Result<Vec<Device>, String> {
    let host = cpal::default_host();
    let default_device = host.default_output_device();
    let default = default_device.as_ref().and_then(|d| d.id().ok());
    let mut result: Vec<Device> = host
        .output_devices()
        .map_err(|e| e.to_string())?
        .map(|d| {
            let id = d.id().map_err(|e| e.to_string())?;
            Ok(Device {
                default: Some(&id) == default.as_ref(),
                id: id.to_string(),
                name: d.description().map_err(|e| e.to_string())?.name().into(),
            })
        })
        .collect::<Result<_, String>>()?;
    if let (Some(device), Some(id)) = (default_device, default)
        && !result.iter().any(|d| d.id == id.to_string())
    {
        result.insert(
            0,
            Device {
                id: id.to_string(),
                name: device
                    .description()
                    .map_err(|e| e.to_string())?
                    .name()
                    .into(),
                default: true,
            },
        );
    }
    Ok(result)
}

struct Renderer {
    processor: Processor,
    rate: u32,
    scratch: [f32; 256],
    cursor: usize,
    a: [f32; 2],
    b: [f32; 2],
    phase: f64,
    primed: bool,
}
impl Renderer {
    fn read(&mut self) -> [f32; 2] {
        if self.cursor == 128 {
            if self.processor.render(&mut self.scratch).is_err() {
                self.scratch.fill(0.0);
            }
            self.cursor = 0;
        }
        let index = self.cursor * 2;
        self.cursor += 1;
        [self.scratch[index], self.scratch[index + 1]]
    }
    fn next(&mut self, device_rate: u32) -> [f32; 2] {
        if !self.primed {
            self.a = self.read();
            self.b = self.read();
            self.primed = true;
        }
        while self.phase >= 1.0 {
            self.a = self.b;
            self.b = self.read();
            self.phase -= 1.0;
        }
        let frame = std::array::from_fn(|i| {
            (f64::from(self.a[i]) + (f64::from(self.b[i]) - f64::from(self.a[i])) * self.phase)
                as f32
        });
        self.phase += f64::from(self.rate) / f64::from(device_rate);
        frame
    }
}

type Parking = Arc<(Mutex<Option<Renderer>>, Condvar)>;
struct Lease {
    renderer: Option<Renderer>,
    parking: Parking,
}
impl Drop for Lease {
    fn drop(&mut self) {
        // CPAL releases the callback when the stream closes. Returning its state
        // preserves voices and bus history across a device re-open.
        if let Some(renderer) = self.renderer.take() {
            *self.parking.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(renderer);
            self.parking.1.notify_all();
        }
    }
}

pub struct Output {
    stream: Option<cpal::Stream>,
    parking: Parking,
    pub status: Arc<Status>,
    pub device: String,
    pub device_rate: u32,
    pub device_channels: u32,
    requested_frames: u32,
    paused: bool,
}
impl Output {
    pub fn new(processor: Processor, rate: u32, device: &str, frames: u32) -> Result<Self, String> {
        let renderer = Renderer {
            processor,
            rate,
            scratch: [0.0; 256],
            cursor: 128,
            a: [0.0; 2],
            b: [0.0; 2],
            phase: 0.0,
            primed: false,
        };
        let mut result = Self {
            stream: None,
            parking: Arc::new((Mutex::new(Some(renderer)), Condvar::new())),
            status: Arc::new(Status::default()),
            device: String::new(),
            device_rate: 0,
            device_channels: 0,
            requested_frames: frames,
            paused: false,
        };
        result.select(device)?;
        Ok(result)
    }
    pub fn paused(&self) -> bool {
        self.paused
    }
    pub fn pause(&mut self) -> Result<(), String> {
        self.stream
            .as_ref()
            .ok_or("audio output has no device")?
            .pause()
            .map_err(|e| e.to_string())?;
        self.paused = true;
        hxn::log::debug!(target: "hxn.audio", "output paused");
        Ok(())
    }
    pub fn resume(&mut self) -> Result<(), String> {
        self.stream
            .as_ref()
            .ok_or("audio output has no device")?
            .play()
            .map_err(|e| e.to_string())?;
        self.paused = false;
        hxn::log::debug!(target: "hxn.audio", "output resumed");
        Ok(())
    }
    pub fn select(&mut self, id: &str) -> Result<(), String> {
        // Resolve first so an invalid device name leaves a running stream intact.
        let device = if id.is_empty() {
            cpal::default_host().default_output_device()
        } else {
            let parsed: cpal::DeviceId = id.parse().map_err(|e: cpal::Error| e.to_string())?;
            let host = cpal::host_from_id(parsed.host()).map_err(|e| e.to_string())?;
            let default = host.default_output_device();
            if default.as_ref().and_then(|d| d.id().ok()).as_ref() == Some(&parsed) {
                default
            } else {
                host.device_by_id(&parsed)
            }
        }
        .ok_or("audio output device is unavailable")?;
        let supported = device.default_output_config().map_err(|e| e.to_string())?;
        if supported.channels() == 0 || supported.sample_rate() == 0 {
            return Err("device returned an invalid output format".into());
        }
        let mut config: StreamConfig = supported.into();
        if let cpal::SupportedBufferSize::Range { min, max } = supported.buffer_size()
            && self.requested_frames > 0
        {
            config.buffer_size = cpal::BufferSize::Fixed(self.requested_frames.clamp(*min, *max));
        }
        drop(self.stream.take());
        let guard = self
            .parking
            .0
            .lock()
            .map_err(|_| "audio renderer lock is poisoned")?;
        let (mut guard, _) = self
            .parking
            .1
            .wait_timeout_while(guard, Duration::from_secs(2), |value| value.is_none())
            .map_err(|_| "audio renderer lock is poisoned")?;
        let renderer = guard
            .take()
            .ok_or("audio device did not release its renderer")?;
        drop(guard);
        let lease = Lease {
            renderer: Some(renderer),
            parking: self.parking.clone(),
        };
        self.status.event.store(0, SeqCst);
        self.status.latency_ns.store(0, SeqCst);
        self.status.callbacks.store(0, SeqCst);
        let rate = config.sample_rate;
        let channels = config.channels as u32;
        let stream = match supported.sample_format() {
            SampleFormat::F32 => build::<f32>(&device, config, lease, self.status.clone()),
            SampleFormat::F64 => build::<f64>(&device, config, lease, self.status.clone()),
            SampleFormat::I8 => build::<i8>(&device, config, lease, self.status.clone()),
            SampleFormat::I16 => build::<i16>(&device, config, lease, self.status.clone()),
            SampleFormat::I24 => build::<cpal::I24>(&device, config, lease, self.status.clone()),
            SampleFormat::I32 => build::<i32>(&device, config, lease, self.status.clone()),
            SampleFormat::I64 => build::<i64>(&device, config, lease, self.status.clone()),
            SampleFormat::U8 => build::<u8>(&device, config, lease, self.status.clone()),
            SampleFormat::U16 => build::<u16>(&device, config, lease, self.status.clone()),
            SampleFormat::U32 => build::<u32>(&device, config, lease, self.status.clone()),
            SampleFormat::U64 => build::<u64>(&device, config, lease, self.status.clone()),
            _ => return Err("unsupported device sample format".into()),
        }?;
        self.device = device.id().map_err(|e| e.to_string())?.to_string();
        self.device_rate = rate;
        self.device_channels = channels;
        if !self.paused {
            stream.play().map_err(|e| e.to_string())?;
        }
        self.stream = Some(stream);
        hxn::log::debug!(target: "hxn.audio", "selected output: {rate} Hz, {channels} channels");
        Ok(())
    }
}
fn build<T: SizedSample + FromSample<f32>>(
    device: &cpal::Device,
    config: StreamConfig,
    mut lease: Lease,
    status: Arc<Status>,
) -> Result<cpal::Stream, String> {
    let channels = config.channels as usize;
    let rate = config.sample_rate;
    let errors = status.clone();
    device
        .build_output_stream(
            config,
            move |output: &mut [T], info: &cpal::OutputCallbackInfo| {
                let renderer = lease.renderer.as_mut().unwrap();
                for frame in output.chunks_mut(channels) {
                    let mixed = renderer.next(rate);
                    for (channel, sample) in frame.iter_mut().enumerate() {
                        *sample = T::from_sample(match (channels, channel) {
                            (1, _) => (mixed[0] + mixed[1]) * 0.5,
                            (_, 0) => mixed[0],
                            (_, 1) => mixed[1],
                            _ => 0.0,
                        });
                    }
                }
                let timestamp = info.timestamp();
                if let Some(latency) = timestamp
                    .playback
                    .checked_duration_since(timestamp.callback)
                {
                    let queued = Duration::from_secs_f64(
                        (128 - renderer.cursor + 2) as f64 / f64::from(renderer.rate),
                    );
                    status.latency_ns.store(
                        (latency + queued).as_nanos().min(u128::from(u64::MAX)) as u64,
                        SeqCst,
                    );
                }
                status.callbacks.fetch_add(1, SeqCst);
            },
            move |error| {
                let event = match error.kind() {
                    cpal::ErrorKind::Xrun => {
                        errors.xruns.fetch_add(1, SeqCst);
                        return;
                    }
                    cpal::ErrorKind::DeviceChanged => 1,
                    cpal::ErrorKind::RealtimeDenied => 4,
                    cpal::ErrorKind::DeviceNotAvailable => 2,
                    _ => 3,
                };
                errors.event.store(event, SeqCst);
            },
            Some(Duration::from_secs(2)),
        )
        .map_err(|e| e.to_string())
}

/// Initializes the dedicated CLI check Activity before any Rust audio call.
///
/// # Safety
/// The host calls once before its program starts, with a valid JavaVM and a
/// global Context reference kept alive until process exit. Production hosts
/// that already initialize ndk-context do not call this entry point.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn hxn_audio_check_context(
    vm: *mut std::ffi::c_void,
    context: *mut std::ffi::c_void,
) {
    unsafe { ndk_context::initialize_android_context(vm, context) };
}

use crate::{Frame, MixFrame, Result};
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

/// Immutable PCM, shared by every voice playing it.
#[derive(Clone)]
pub struct Pcm {
    pub(crate) frames: Arc<[Frame]>,
    pub sample_rate: u32,
    pub channels: u32,
}

impl Pcm {
    pub fn from_le_bytes(bytes: &[u8], sample_rate: u32, channels: u32) -> Result<Self> {
        validate_format(sample_rate, channels)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err("PCM buffer exceeds 32 MiB; use a stream for longer audio");
        }
        let stride = channels as usize * 4;
        if bytes.is_empty() || !bytes.len().is_multiple_of(stride) {
            return Err("PCM must contain complete, nonempty interleaved f32 frames");
        }
        validate_samples(bytes)?;
        let mut frames = Vec::new();
        frames
            .try_reserve_exact(bytes.len() / stride)
            .map_err(|_| "PCM allocation failed")?;
        for frame in bytes.chunks_exact(stride) {
            frames.push(read_frame(frame, channels));
        }
        Ok(Self {
            frames: frames.into(),
            sample_rate,
            channels,
        })
    }

    /// A control-side copy for adapters whose renderer has its own memory.
    pub fn to_le_bytes(&self) -> Vec<u8> {
        self.frames
            .iter()
            .flat_map(|frame| {
                frame[..self.channels as usize]
                    .iter()
                    .flat_map(|sample| sample.to_le_bytes())
            })
            .collect()
    }
    pub fn frames(&self) -> usize {
        self.frames.len()
    }
    /// Stable allocation identity while any PCM clone is alive.
    pub fn storage_id(&self) -> usize {
        Arc::as_ptr(&self.frames) as *const Frame as usize
    }
    pub fn allocation_bytes(&self) -> usize {
        self.frames.len() * size_of::<Frame>()
    }
}

fn validate_format(rate: u32, channels: u32) -> Result<()> {
    if !(8_000..=192_000).contains(&rate) {
        return Err("sample rate must be 8000..=192000 Hz");
    }
    if channels != 1 && channels != 2 {
        return Err("PCM must have one or two channels");
    }
    Ok(())
}

fn validate_samples(bytes: &[u8]) -> Result<()> {
    if bytes
        .as_chunks::<4>()
        .0
        .iter()
        .any(|b| !f32::from_le_bytes(*b).is_finite())
    {
        return Err("PCM contains a non-finite sample");
    }
    Ok(())
}

fn read_frame(bytes: &[u8], channels: u32) -> Frame {
    let left = f32::from_le_bytes(bytes[..4].try_into().unwrap());
    [
        left,
        if channels == 1 {
            left
        } else {
            f32::from_le_bytes(bytes[4..8].try_into().unwrap())
        },
    ]
}

pub struct StreamWriter {
    producer: Producer<Frame>,
    finished: Arc<AtomicBool>,
    pub sample_rate: u32,
    pub channels: u32,
}

impl StreamWriter {
    /// Writes complete frames only. The caller retains and retries the suffix.
    pub fn write(&mut self, bytes: &[u8]) -> Result<usize> {
        if self.finished.load(Ordering::SeqCst) {
            return Err("stream is finished");
        }
        let stride = self.channels as usize * 4;
        if !bytes.len().is_multiple_of(stride) {
            return Err("PCM contains an incomplete frame");
        }
        validate_samples(bytes)?;
        let count = (bytes.len() / stride).min(self.producer.slots());
        for frame in bytes[..count * stride].chunks_exact(stride) {
            self.producer
                .push(read_frame(frame, self.channels))
                .map_err(|_| "stream capacity changed")?;
        }
        Ok(count)
    }
    pub fn available_frames(&self) -> usize {
        self.producer.slots()
    }
    pub fn finish(&self) {
        self.finished.store(true, Ordering::SeqCst);
    }
}

impl Drop for StreamWriter {
    fn drop(&mut self) {
        self.finish();
    }
}

pub(crate) fn stream(
    capacity: usize,
    sample_rate: u32,
    channels: u32,
) -> Result<(StreamWriter, Source)> {
    validate_stream(capacity, sample_rate, channels)?;
    let (producer, consumer) = RingBuffer::new(capacity);
    let finished = Arc::new(AtomicBool::new(false));
    Ok((
        StreamWriter {
            producer,
            finished: finished.clone(),
            sample_rate,
            channels,
        },
        Source::Stream(StreamSource {
            consumer,
            finished,
            sample_rate,
            channels,
            a: None,
            b: None,
            phase: 0.0,
            position: 0,
            eof: false,
        }),
    ))
}

pub(crate) enum Source {
    Clip {
        pcm: Pcm,
        position: f64,
        looping: bool,
    },
    Stream(StreamSource),
}

pub(crate) enum Read {
    Frame(MixFrame),
    Pending,
    End,
}

impl Source {
    pub fn channels(&self) -> u32 {
        match self {
            Self::Clip { pcm, .. } => pcm.channels,
            Self::Stream(s) => s.channels,
        }
    }
    pub fn sample_rate(&self) -> u32 {
        match self {
            Self::Clip { pcm, .. } => pcm.sample_rate,
            Self::Stream(s) => s.sample_rate,
        }
    }
    pub fn position(&self) -> u64 {
        match self {
            Self::Clip { position, .. } => *position as u64,
            Self::Stream(s) => s.position,
        }
    }
    pub fn next(&mut self, step: f64) -> Read {
        match self {
            Self::Clip {
                pcm,
                position,
                looping,
            } => {
                let length = pcm.frames.len() as f64;
                if *position >= length {
                    if *looping {
                        *position %= length;
                    } else {
                        return Read::End;
                    }
                }
                let index = *position as usize;
                let next = if index + 1 < pcm.frames.len() {
                    index + 1
                } else if *looping {
                    0
                } else {
                    index
                };
                let fraction = position.fract();
                let result = interpolate(pcm.frames[index], pcm.frames[next], fraction);
                *position += step;
                Read::Frame(result)
            }
            Self::Stream(s) => s.next(step),
        }
    }
}

pub(crate) struct StreamSource {
    consumer: Consumer<Frame>,
    finished: Arc<AtomicBool>,
    sample_rate: u32,
    channels: u32,
    a: Option<Frame>,
    b: Option<Frame>,
    phase: f64,
    position: u64,
    eof: bool,
}

impl StreamSource {
    fn next(&mut self, step: f64) -> Read {
        if self.a.is_none() {
            self.a = self.pop();
        }
        if self.a.is_none() {
            return self.empty();
        }
        while self.phase >= 1.0 {
            let next = self.b.take().or_else(|| self.pop());
            let Some(next) = next else {
                if self.eof {
                    // The last held frame was rendered before reaching EOF.
                    self.position += 1;
                    self.a = None;
                    self.phase = 0.0;
                }
                return self.empty();
            };
            self.a = Some(next);
            self.position += 1;
            self.phase -= 1.0;
        }
        if self.b.is_none() {
            self.b = self.pop();
        }
        let a = self.a.unwrap();
        let b = match self.b {
            Some(b) => b,
            None if self.eof => a,
            None if self.phase == 0.0 => a,
            None => return Read::Pending,
        };
        let frame = interpolate(a, b, self.phase);
        self.phase += step;
        Read::Frame(frame)
    }
    fn pop(&mut self) -> Option<Frame> {
        match self.consumer.pop() {
            Ok(frame) => Some(frame),
            Err(_) if self.finished.load(Ordering::SeqCst) => {
                // Finish is published after the producer's last push. Refresh
                // the consumer after observing it before declaring EOF.
                match self.consumer.pop() {
                    Ok(frame) => Some(frame),
                    Err(_) => {
                        self.eof = true;
                        None
                    }
                }
            }
            Err(_) => None,
        }
    }
    fn empty(&self) -> Read {
        if self.eof { Read::End } else { Read::Pending }
    }
}

fn interpolate(a: Frame, b: Frame, fraction: f64) -> MixFrame {
    std::array::from_fn(|i| f64::from(a[i]) + (f64::from(b[i]) - f64::from(a[i])) * fraction)
}

/// Validates control-side stream options without allocating a ring.
pub fn validate_stream(capacity: usize, sample_rate: u32, channels: u32) -> Result<()> {
    validate_format(sample_rate, channels)?;
    if !(2..=1_048_576).contains(&capacity) {
        return Err("stream capacity must be 2..=1048576 frames");
    }
    Ok(())
}

/// Checks a whole write before an adapter reserves any ring capacity.
pub fn validate_pcm(bytes: &[u8], channels: u32) -> Result<usize> {
    if channels != 1 && channels != 2 {
        return Err("PCM must have one or two channels");
    }
    let stride = channels as usize * 4;
    if !bytes.len().is_multiple_of(stride) {
        return Err("PCM contains an incomplete frame");
    }
    validate_samples(bytes)?;
    Ok(bytes.len() / stride)
}

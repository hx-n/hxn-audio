//! A bounded Rust mixer. Its render path neither allocates nor takes a lock.
#![forbid(unsafe_code)]

mod mixer;
mod source;

pub use mixer::{Config, Controller, Delay, Meter, Processor, Voice, VoiceOptions, mixer};
pub use source::{Pcm, StreamWriter, validate_pcm, validate_stream};

pub type Frame = [f32; 2];
pub type MixFrame = [f64; 2];
pub type Result<T> = std::result::Result<T, &'static str>;

fn finite(value: f64, min: f64, max: f64, message: &'static str) -> Result<f64> {
    if value.is_finite() && (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(message)
    }
}

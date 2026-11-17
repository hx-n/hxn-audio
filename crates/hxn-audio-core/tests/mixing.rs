use hxn_audio_core::{Config, Delay, Pcm, VoiceOptions, mixer};

fn config() -> Config {
    Config {
        max_voices: 4,
        buses: 4,
        max_delay_frames: 32,
        ramp_frames: 0,
        ..Config::default()
    }
}
fn bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_le_bytes()).collect()
}
fn pcm(samples: &[f32], channels: u32) -> Pcm {
    Pcm::from_le_bytes(&bytes(samples), 48_000, channels).unwrap()
}
fn close(actual: f32, expected: f32) {
    assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
}

#[test]
fn overlapping_voices_have_independent_pan_gain_and_lifetime() {
    let (mut control, mut output) = mixer(config()).unwrap();
    let sound = pcm(&[0.25; 4], 1);
    let left = control
        .play(
            &sound,
            VoiceOptions {
                pan: -1.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    let right = control
        .play(
            &sound,
            VoiceOptions {
                pan: 1.0,
                gain: 2.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    drop(sound);
    let mut block = [0.0; 8];
    output.render(&mut block).unwrap();
    for frame in block.as_chunks::<2>().0.iter() {
        close(frame[0], 0.25);
        close(frame[1], 0.5);
    }
    assert_eq!(control.meter().active_voices, 2);
    output.render(&mut block).unwrap();
    assert_eq!(block, [0.0; 8]);
    assert!(left.finished() && right.finished());
    control.collect();
    for _ in 0..4 {
        control
            .play(&pcm(&[0.1], 1), VoiceOptions::default())
            .unwrap();
    }
    assert!(
        control
            .play(&pcm(&[0.1], 1), VoiceOptions::default())
            .is_err()
    );
}

#[test]
fn clipping_is_after_mixing_and_never_emits_nan_or_infinity() {
    let (mut control, mut output) = mixer(config()).unwrap();
    let large = pcm(&[f32::MAX, -f32::MAX], 2);
    control
        .play(
            &large,
            VoiceOptions {
                gain: 16.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    let mut block = [0.0; 2];
    output.render(&mut block).unwrap();
    assert_eq!(block, [1.0, -1.0]);
    assert_eq!(control.meter().clipped_samples, 2);
    assert!(Pcm::from_le_bytes(&bytes(&[f32::NAN]), 48_000, 1).is_err());
    assert!(Pcm::from_le_bytes(&[0; 3], 48_000, 1).is_err());
    assert!(Pcm::from_le_bytes(&bytes(&[0.0]), 48_000, 0).is_err());
}

#[test]
fn resampling_interpolates_and_looping_wraps() {
    let (mut control, mut output) = mixer(config()).unwrap();
    let voice = control
        .play(
            &pcm(&[0.0, 1.0], 1),
            VoiceOptions {
                pan: -1.0,
                rate: 0.5,
                looping: true,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    let mut block = [0.0; 12];
    output.render(&mut block).unwrap();
    for (frame, value) in block
        .as_chunks::<2>()
        .0
        .iter()
        .zip([0.0, 0.5, 1.0, 0.5, 0.0, 0.5])
    {
        close(frame[0], value);
        close(frame[1], 0.0);
    }
    assert!(!voice.finished());
    voice.stop();
    output.render(&mut block).unwrap();
    assert!(voice.finished());
    assert_eq!(block, [0.0; 12]);
}

#[test]
fn streamed_frames_have_backpressure_survive_wrap_and_drain_at_eof() {
    let (mut control, mut output) = mixer(config()).unwrap();
    let (voice, mut writer) = control
        .stream(
            3,
            48_000,
            1,
            VoiceOptions {
                pan: -1.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    assert_eq!(writer.write(&bytes(&[0.1, 0.2, 0.3, 0.4])).unwrap(), 3);
    assert_eq!(writer.write(&bytes(&[0.4])).unwrap(), 0);
    let mut first = [0.0; 4];
    output.render(&mut first).unwrap();
    close(first[0], 0.1);
    close(first[2], 0.2);
    assert_eq!(writer.write(&bytes(&[0.4, 0.5])).unwrap(), 2);
    writer.finish();
    assert!(writer.write(&bytes(&[0.6])).is_err());
    let mut last = [0.0; 8];
    output.render(&mut last).unwrap();
    for (frame, value) in last.as_chunks::<2>().0.iter().zip([0.3, 0.4, 0.5, 0.0]) {
        close(frame[0], value);
    }
    assert!(voice.finished());
    assert_eq!(control.meter().starved_frames, 0);
    assert_eq!(voice.position_seconds(), 5.0 / 48_000.0);
}

#[test]
fn underrun_is_silence_and_does_not_consume_future_stream_data() {
    let (mut control, mut output) = mixer(config()).unwrap();
    let (voice, mut writer) = control
        .stream(
            3,
            48_000,
            1,
            VoiceOptions {
                pan: -1.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    let mut block = [1.0; 8];
    output.render(&mut block).unwrap();
    assert_eq!(block, [0.0; 8]);
    assert_eq!(control.meter().starved_frames, 4);
    assert!(!voice.finished());
    writer.write(&bytes(&[0.25, 0.5])).unwrap();
    drop(writer);
    output.render(&mut block).unwrap();
    assert_eq!(block, [0.25, 0.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]);
    assert!(voice.finished());
}

#[test]
fn bus_order_is_topological_and_cycles_are_rejected_atomically() {
    let (mut control, mut output) = mixer(config()).unwrap();
    control.route(1, 2).unwrap();
    control.set_bus_gain(2, 0.5).unwrap();
    assert!(control.route(2, 1).is_err());
    assert!(control.route(0, 2).is_err());
    control
        .play(
            &pcm(&[0.5], 1),
            VoiceOptions {
                bus: 1,
                pan: -1.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    let mut block = [0.0; 2];
    output.render(&mut block).unwrap();
    close(block[0], 0.25);
    close(block[1], 0.0);
}

#[test]
fn effects_have_persistent_state_and_delay_parameter_edits_keep_the_tail() {
    let (mut control, mut output) = mixer(config()).unwrap();
    control
        .set_delay(
            1,
            Delay {
                frames: 2,
                feedback: 0.5,
                wet: 1.0,
            },
        )
        .unwrap();
    control
        .play(
            &pcm(&[1.0], 1),
            VoiceOptions {
                bus: 1,
                pan: -1.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    let mut first = [0.0; 6];
    output.render(&mut first).unwrap();
    close(first[0], 0.0);
    close(first[2], 0.0);
    close(first[4], 1.0);
    control
        .set_delay(
            1,
            Delay {
                frames: 2,
                feedback: 0.25,
                wet: 1.0,
            },
        )
        .unwrap();
    let mut second = [0.0; 4];
    output.render(&mut second).unwrap();
    close(second[2], 0.5);
    control.set_delay(1, Delay::default()).unwrap();
    control.set_lowpass(1, 500.0).unwrap();
    control
        .play(
            &pcm(&[1.0, 0.0], 1),
            VoiceOptions {
                bus: 1,
                pan: -1.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    output.render(&mut second).unwrap();
    assert!(second[0] > 0.0 && second[0] < 0.1);
    assert!(second[2] > 0.0 && second[2] < second[0]);
}

#[test]
fn fades_use_the_requested_number_of_frames_and_invalid_values_do_not_change_them() {
    let (mut control, mut output) = mixer(Config {
        ramp_frames: 4,
        ..config()
    })
    .unwrap();
    let voice = control
        .play(
            &pcm(&[1.0; 20], 1),
            VoiceOptions {
                pan: -1.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    assert!(voice.set_gain(f64::NAN).is_err());
    assert!(voice.set_pan(2.0).is_err());
    assert!(voice.set_rate(0.0).is_err());
    let mut block = [0.0; 8];
    output.render(&mut block).unwrap();
    for (frame, value) in block.as_chunks::<2>().0.iter().zip([0.25, 0.5, 0.75, 1.0]) {
        close(frame[0], value);
    }
    voice.stop();
    output.render(&mut block).unwrap();
    for (frame, value) in block.as_chunks::<2>().0.iter().zip([0.75, 0.5, 0.25, 0.0]) {
        close(frame[0], value);
    }
    assert!(voice.finished());
}

#[test]
fn full_command_queues_return_backpressure_without_leaking_voice_slots() {
    let (mut control, mut output) = mixer(Config {
        command_capacity: 1,
        ..config()
    })
    .unwrap();
    let clip = pcm(&[0.2], 1);
    control.play(&clip, VoiceOptions::default()).unwrap();
    assert!(control.play(&clip, VoiceOptions::default()).is_err());
    let mut block = [0.0; 4];
    output.render(&mut block).unwrap();
    control.play(&clip, VoiceOptions::default()).unwrap();
    output.render(&mut block).unwrap();
    control.collect();
    assert_eq!(control.meter().active_voices, 0);
}

#[test]
fn a_live_voice_can_move_between_buses_and_rejects_an_invalid_route() {
    let (mut control, mut processor) = mixer(config()).unwrap();
    control.set_bus_gain(1, 0.25).unwrap();
    let pcm = Pcm::from_le_bytes(&bytes(&[0.4; 8]), 48_000, 1).unwrap();
    let voice = control
        .play(
            &pcm,
            VoiceOptions {
                bus: 1,
                pan: -1.0,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    let mut output = [0.0; 2];
    processor.render(&mut output).unwrap();
    close(output[0], 0.1);
    voice.set_bus(0).unwrap();
    assert!(voice.set_bus(usize::MAX).is_err());
    processor.render(&mut output).unwrap();
    close(output[0], 0.4);
}

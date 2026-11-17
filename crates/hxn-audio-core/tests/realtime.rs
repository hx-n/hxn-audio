use hxn_audio_core::{Config, Delay, Pcm, VoiceOptions, mixer};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    cell::Cell,
};

thread_local! {
    static AUDIT: Cell<bool> = const { Cell::new(false) };
    static OPERATIONS: Cell<usize> = const { Cell::new(0) };
}
struct Audited;
#[global_allocator]
static ALLOCATOR: Audited = Audited;

fn allocation_operation() {
    let _ = AUDIT.try_with(|enabled| {
        if enabled.get() {
            OPERATIONS.with(|count| count.set(count.get() + 1));
        }
    });
}

// The test delegates memory ownership unchanged to System. Only the rendering
// test thread is counted, so the parallel test harness cannot affect the audit.
unsafe impl GlobalAlloc for Audited {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation_operation();
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        allocation_operation();
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation_operation();
        unsafe { System.realloc(ptr, layout, size) }
    }
}

fn audited(operation: impl FnOnce()) {
    OPERATIONS.with(|v| v.set(0));
    AUDIT.with(|v| v.set(true));
    operation();
    AUDIT.with(|v| v.set(false));
    assert_eq!(
        OPERATIONS.with(Cell::get),
        0,
        "render allocated or freed memory"
    );
}

#[test]
fn commands_streaming_effects_stops_and_retirement_do_not_allocate_or_free_on_render() {
    let (mut control, mut processor) = mixer(Config {
        max_voices: 4,
        max_delay_frames: 512,
        ..Config::default()
    })
    .unwrap();
    let samples: Vec<u8> = (0..256)
        .flat_map(|i| ((i as f32 * 0.1).sin() * 0.1).to_le_bytes())
        .collect();
    let clip = Pcm::from_le_bytes(&samples, 44_100, 1).unwrap();
    control.route(1, 2).unwrap();
    control.set_lowpass(2, 1500.0).unwrap();
    control
        .set_delay(
            1,
            Delay {
                frames: 128,
                feedback: 0.5,
                wet: 0.5,
            },
        )
        .unwrap();
    let voice = control
        .play(
            &clip,
            VoiceOptions {
                bus: 1,
                ..VoiceOptions::default()
            },
        )
        .unwrap();
    let (stream, mut writer) = control
        .stream(256, 32_000, 1, VoiceOptions::default())
        .unwrap();
    writer.write(&samples).unwrap();
    writer.finish();
    drop(clip);
    drop(writer);
    let mut output = [0.0; 1024];
    audited(|| {
        processor.render(&mut output).unwrap();
        voice.stop();
        stream.stop();
        processor.render(&mut output).unwrap();
        processor.render(&mut output).unwrap();
    });
    assert!(voice.finished() && stream.finished());
    control.collect();
}

#[test]
fn a_control_thread_can_fill_streams_and_update_voices_while_rendering() {
    for _ in 0..4 {
        let (mut control, mut processor) = mixer(Config {
            max_voices: 2,
            max_delay_frames: 64,
            ..Config::default()
        })
        .unwrap();
        let (voice, mut writer) = control
            .stream(1024, 48_000, 1, VoiceOptions::default())
            .unwrap();
        let render = std::thread::spawn(move || {
            let mut block = [0.0; 256];
            let mut audible = 0;
            while !voice.finished() {
                processor.render(&mut block).unwrap();
                assert!(block.iter().all(|s| s.is_finite()));
                audible += block
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .filter(|frame| frame[0] != 0.0)
                    .count();
                std::thread::yield_now();
            }
            audible
        });
        let chunk: Vec<u8> = [0.05f32; 64].iter().flat_map(|v| v.to_le_bytes()).collect();
        for _ in 0..1000 {
            while writer.available_frames() < 64 {
                std::thread::yield_now();
            }
            assert_eq!(writer.write(&chunk).unwrap(), 64);
        }
        writer.finish();
        assert_eq!(
            render.join().unwrap(),
            64_000,
            "stream lost or duplicated a frame"
        );
        assert_eq!(control.meter().active_voices, 0);
        control.collect();
    }
}

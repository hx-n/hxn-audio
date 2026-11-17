//! Browser control transport. Only the page agent and Worklet own WebAudio state.
use crate::{AudioOptions, PlayOptions};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{
    Arc, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, AtomicU32, Ordering::SeqCst},
};

#[repr(C)]
struct Mailbox {
    sent: AtomicU32,
    done: AtomicU32,
    request: AtomicU32,
    request_len: AtomicU32,
    reply: AtomicU32,
    capacity: AtomicU32,
    reply_len: AtomicU32,
    reserved: AtomicU32,
}
struct Broker {
    mailbox: Box<Mailbox>,
    started: bool,
}
static BROKER: OnceLock<Mutex<Broker>> = OnceLock::new();
#[cfg(target_os = "wasi")]
unsafe extern "C" {
    fn ash_host_agent(name: *const u8, len: u32, address: u32) -> i32;
}
#[cfg(target_os = "emscripten")]
unsafe extern "C" {
    fn hxn_audio_agent(address: u32) -> i32;
}
#[cfg(all(target_os = "emscripten", not(target_feature = "atomics")))]
unsafe extern "C" {
    fn hxn_audio_execute();
}
fn request(value: Value) -> Result<Value, String> {
    let broker = BROKER.get_or_init(|| {
        Mutex::new(Broker {
            mailbox: Box::new(Mailbox {
                sent: AtomicU32::new(0),
                done: AtomicU32::new(0),
                request: AtomicU32::new(0),
                request_len: AtomicU32::new(0),
                reply: AtomicU32::new(0),
                capacity: AtomicU32::new(0),
                reply_len: AtomicU32::new(0),
                reserved: AtomicU32::new(0),
            }),
            started: false,
        })
    });
    let mut broker = broker
        .lock()
        .map_err(|_| "browser audio transport lock is poisoned")?;
    if !broker.started {
        let address = (&*broker.mailbox) as *const Mailbox as u32;
        #[cfg(target_os = "wasi")]
        let started = unsafe { ash_host_agent(b"hxn_audio".as_ptr(), 9, address) };
        #[cfg(target_os = "emscripten")]
        let started = unsafe { hxn_audio_agent(address) };
        #[cfg(not(any(target_os = "wasi", target_os = "emscripten")))]
        let started = {
            let _ = address;
            0
        };
        if started == 0 {
            return Err("audio output requires a browser page host".into());
        }
        broker.started = true;
    }
    let bytes = serde_json::to_vec(&value).map_err(|e| e.to_string())?;
    let mut reply = vec![0u8; 65_536];
    let m = &broker.mailbox;
    m.request.store(bytes.as_ptr() as u32, SeqCst);
    m.request_len.store(bytes.len() as u32, SeqCst);
    m.reply.store(reply.as_mut_ptr() as u32, SeqCst);
    m.capacity.store(reply.len() as u32, SeqCst);
    #[cfg(all(target_os = "emscripten", not(target_feature = "atomics")))]
    unsafe {
        hxn_audio_execute();
    }
    #[cfg(target_feature = "atomics")]
    {
        let sent = m.sent.fetch_add(1, SeqCst).wrapping_add(1);
        unsafe {
            core::arch::wasm32::memory_atomic_notify(m.sent.as_ptr().cast(), u32::MAX);
        }
        while m.done.load(SeqCst) != sent {
            let seen = m.done.load(SeqCst);
            if seen == sent {
                break;
            }
            unsafe {
                core::arch::wasm32::memory_atomic_wait32(
                    m.done.as_ptr().cast(),
                    seen as i32,
                    1_000_000_000,
                );
            }
        }
    }
    let len = m.reply_len.load(SeqCst) as usize;
    if len > reply.len() {
        return Err("invalid browser audio reply length".into());
    }
    let response: Value = serde_json::from_slice(&reply[..len]).map_err(|e| e.to_string())?;
    if let Some(error) = response["error"].as_str() {
        return Err(error.into());
    }
    Ok(response["ok"].clone())
}

struct CachedBuffer {
    buffer: Weak<crate::StoredPcm>,
    _weight: hxn::Weight,
}
pub struct Context {
    id: u32,
    closed: AtomicBool,
    cache: Mutex<Value>,
    next_voice: AtomicU32,
    buffers: Mutex<HashMap<u32, CachedBuffer>>,
}
impl Context {
    pub fn open(options: &AudioOptions) -> Result<Arc<Self>, String> {
        let result = request(
            json!({"op":"open", "sampleRate":options.sample_rate, "voices":options.voices, "buses":options.buses, "device":options.device}),
        )?;
        let id = result["id"]
            .as_u64()
            .ok_or("invalid browser output identifier")? as u32;
        Ok(Arc::new(Self {
            id,
            closed: AtomicBool::new(false),
            cache: Mutex::new(Value::Null),
            next_voice: AtomicU32::new(1),
            buffers: Mutex::new(HashMap::new()),
        }))
    }
    pub fn command(&self, mut command: Value) -> Result<Value, String> {
        if self.closed.load(SeqCst) {
            return Err("audio output is closed".into());
        }
        command["output"] = self.id.into();
        request(command)
    }
    pub fn poll(&self) -> Result<Value, String> {
        let info = self.command(json!({"op":"info"}))?;
        *self
            .cache
            .lock()
            .map_err(|_| "audio status lock is poisoned")? = info.clone();
        Ok(info)
    }
    pub fn close(&self) {
        if !self.closed.swap(true, SeqCst) {
            let _ = request(json!({"op":"close", "output":self.id}));
            let buffers = self
                .buffers
                .lock()
                .ok()
                .map(|mut entries| std::mem::take(&mut *entries));
            drop(buffers);
        }
    }
    fn id(&self) -> Result<u32, String> {
        let mut current = self.next_voice.load(SeqCst);
        loop {
            let next = current
                .checked_add(1)
                .ok_or("browser voice identifiers exhausted")?;
            match self
                .next_voice
                .compare_exchange(current, next, SeqCst, SeqCst)
            {
                Ok(_) => return Ok(current),
                Err(actual) => current = actual,
            }
        }
    }
    pub fn play(
        self: &Arc<Self>,
        buffer: &Arc<crate::StoredPcm>,
        options: &PlayOptions,
    ) -> Result<Voice, String> {
        let id = self.id()?;
        let key = Arc::as_ptr(buffer) as usize as u32;
        let known = self
            .buffers
            .lock()
            .map_err(|_| "audio buffer cache lock is poisoned")?
            .contains_key(&key);
        let bytes = if known {
            Vec::new()
        } else {
            buffer.pcm.to_le_bytes()
        };
        self.command(json!({"op":"playBuffer", "id":id, "buffer":key, "sampleRate":buffer.pcm.sample_rate, "channels":buffer.pcm.channels, "address":bytes.as_ptr() as u32, "length":bytes.len(), "bus":options.bus, "gain":options.gain,"pan":options.pan,"rate":options.rate,"looping":options.looping}))?;
        self.buffers
            .lock()
            .map_err(|_| "audio buffer cache lock is poisoned")?
            .entry(key)
            .or_insert_with(|| CachedBuffer {
                buffer: Arc::downgrade(buffer),
                _weight: hxn::Weight::new(buffer.pcm.allocation_bytes()),
            });
        Ok(Voice {
            context: self.clone(),
            id,
            closed: Arc::new(AtomicBool::new(false)),
            finished: Arc::new(AtomicBool::new(false)),
        })
    }
    pub fn collect_buffers(&self) -> Result<(), String> {
        let dead: Vec<u32> = self
            .buffers
            .lock()
            .map_err(|_| "audio buffer cache lock is poisoned")?
            .iter()
            .filter_map(|(&id, entry)| (entry.buffer.strong_count() == 0).then_some(id))
            .collect();
        for id in dead {
            self.command(json!({"op":"unload", "buffer":id}))?;
            let removed = self
                .buffers
                .lock()
                .map_err(|_| "audio buffer cache lock is poisoned")?
                .remove(&id);
            drop(removed);
        }
        Ok(())
    }
    pub fn stream(
        self: &Arc<Self>,
        rate: u32,
        channels: u32,
        capacity: u32,
        options: &PlayOptions,
    ) -> Result<Voice, String> {
        let id = self.id()?;
        self.command(json!({"op":"stream", "id":id, "sampleRate":rate, "channels":channels, "capacity":capacity, "bus":options.bus,"gain":options.gain,"pan":options.pan,"rate":options.rate,"looping":options.looping}))?;
        Ok(Voice {
            context: self.clone(),
            id,
            closed: Arc::new(AtomicBool::new(false)),
            finished: Arc::new(AtomicBool::new(false)),
        })
    }
}
#[derive(Clone)]
pub struct Voice {
    pub context: Arc<Context>,
    pub id: u32,
    closed: Arc<AtomicBool>,
    finished: Arc<AtomicBool>,
}
impl Voice {
    pub fn close(&self) {
        self.closed.store(true, SeqCst);
        if self.finished() {
            let _ = self.command("forget", 0.0);
        } else {
            let _ = self.command("stop", 0.0);
        }
    }
    pub fn release_if_closed(&self) {
        if self.closed.load(SeqCst) {
            let _ = self.command("forget", 0.0);
        }
    }
    pub fn command(&self, op: &str, value: f64) -> Result<(), String> {
        self.context
            .command(json!({"op":op, "id":self.id, "value":value}))
            .map(|_| ())
    }
    pub fn cached(&self) -> Value {
        self.context
            .cache
            .lock()
            .ok()
            .and_then(|v| {
                v["voices"].as_array().and_then(|voices| {
                    voices
                        .iter()
                        .find(|v| v["id"].as_u64() == Some(u64::from(self.id)))
                        .cloned()
                })
            })
            .unwrap_or(Value::Null)
    }
    pub fn finished(&self) -> bool {
        if self.context.closed.load(SeqCst) || self.cached()["finished"].as_bool().unwrap_or(false)
        {
            self.finished.store(true, SeqCst);
        }
        self.finished.load(SeqCst)
    }
    pub fn write(&self, pcm: &[u8]) -> Result<u32, String> {
        self.context.command(json!({"op":"write", "id":self.id, "address":pcm.as_ptr() as u32, "length":pcm.len()})).and_then(|v| v["accepted"].as_u64().map(|n| n as u32).ok_or("invalid browser stream write reply".into()))
    }
    pub fn available(&self) -> Result<u32, String> {
        self.context
            .command(json!({"op":"available","id":self.id}))
            .map(|v| v["available"].as_u64().unwrap_or(0) as u32)
    }
}
pub fn devices() -> Result<Vec<crate::AudioDevice>, String> {
    let list = request(json!({"op":"devices"}))?;
    Ok(list
        .as_array()
        .ok_or("invalid browser device list")?
        .iter()
        .map(|v| crate::AudioDevice {
            id: v["id"].as_str().unwrap_or("").into(),
            name: v["name"].as_str().unwrap_or("").into(),
            is_default: v["isDefault"].as_bool().unwrap_or(false),
        })
        .collect())
}

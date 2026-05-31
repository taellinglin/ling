//! ling-py — PyO3 bindings for ling-audio, ling-physics, and WebSocket netplay.
//!
//! Python classes exposed:
//!   AudioEngine    — 4D spatial audio synthesis (wraps ling_audio::AudioEngine)
//!   HyperbolicWorld — Poincaré-ball hyperbolic sphere world (wraps ling_physics::hyperbolic)
//!   NetClient      — async WebSocket client for netplay

use pyo3::prelude::*;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

// ── AudioEngine ───────────────────────────────────────────────────────────────

#[pyclass(name = "AudioEngine")]
struct PyAudioEngine {
    inner: ling_audio::AudioEngine,
}

#[pymethods]
impl PyAudioEngine {
    #[new]
    fn new() -> PyResult<Self> {
        ling_audio::AudioEngine::new()
            .map(|e| PyAudioEngine { inner: e })
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("audio init: {e}"))
            })
    }

    /// set_tone(idx, x, y, z, w, freq, amp, lfo_rate, lfo_depth)
    ///
    /// Places a synthesised tone at 4D world position (x,y,z,w).
    /// W drives a sub-oscillator cross-modulator for a hyperdimensional shimmer.
    #[pyo3(signature = (
        idx,
        x=0.0, y=0.0, z=0.0, w=1.0,
        freq=220.0, amp=0.15,
        lfo_rate=0.5, lfo_depth=0.02
    ))]
    fn set_tone(
        &self,
        idx: usize,
        x: f32, y: f32, z: f32, w: f32,
        freq: f32, amp: f32,
        lfo_rate: f32, lfo_depth: f32,
    ) {
        self.inner.set_tone(idx, ling_audio::ToneParams {
            x, y, z, w, freq, amp, lfo_rate, lfo_depth,
        });
    }

    fn clear_tone(&self, idx: usize) {
        self.inner.clear_tone(idx);
    }

    /// Update listener orientation to match the camera (yaw + pitch trig values).
    fn set_listener(&self, cry: f32, sry: f32, crx: f32, srx: f32) {
        self.inner.set_listener(cry, sry, crx, srx);
    }

    /// Load a WAV file as looping background music.
    fn load_bgm(&self, path: &str, vol: f32) {
        self.inner.load_bgm(path, vol);
    }

    fn set_bgm_volume(&self, vol: f32) {
        self.inner.set_bgm_volume(vol);
    }

    fn set_master_volume(&self, vol: f32) {
        self.inner.set_master_volume(vol);
    }
}

// ── HyperbolicWorld ───────────────────────────────────────────────────────────

use ling_physics::hyperbolic::HyperbolicSphereWorld;
use glam::Vec3;

#[pyclass(name = "HyperbolicWorld")]
struct PyHyperbolicWorld {
    inner: HyperbolicSphereWorld,
}

#[pymethods]
impl PyHyperbolicWorld {
    #[new]
    #[pyo3(signature = (radius=100.0, curvature=-1.0, gravity=9.81))]
    fn new(radius: f32, curvature: f32, gravity: f32) -> Self {
        PyHyperbolicWorld {
            inner: HyperbolicSphereWorld { radius, curvature, gravity },
        }
    }

    /// Returns the outward gravity direction at world-space pos (x,y,z).
    fn gravity_dir(&self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        let v = self.inner.gravity_dir(Vec3::new(x, y, z));
        (v.x, v.y, v.z)
    }

    /// Returns the gravity force vector (outward, magnitude = gravity * mass).
    fn gravity_force(&self, x: f32, y: f32, z: f32, mass: f32) -> (f32, f32, f32) {
        let v = self.inner.gravity_force(Vec3::new(x, y, z), mass);
        (v.x, v.y, v.z)
    }

    /// Returns the "up" direction at pos — points inward toward sphere centre.
    fn up_at(&self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        let v = self.inner.up_at(Vec3::new(x, y, z));
        (v.x, v.y, v.z)
    }

    /// Hyperbolic distance between two world-space points.
    fn world_distance(
        &self,
        ax: f32, ay: f32, az: f32,
        bx: f32, by: f32, bz: f32,
    ) -> f32 {
        self.inner.world_distance(Vec3::new(ax, ay, az), Vec3::new(bx, by, bz))
    }

    /// Convert world-space position to Poincaré ball coordinate.
    fn to_poincare(&self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        let v = self.inner.to_poincare(Vec3::new(x, y, z));
        (v.x, v.y, v.z)
    }

    /// Convert Poincaré coordinate back to world space.
    fn from_poincare(&self, x: f32, y: f32, z: f32) -> (f32, f32, f32) {
        let v = self.inner.from_poincare(Vec3::new(x, y, z));
        (v.x, v.y, v.z)
    }
}

// Stand-alone hyperbolic math functions exposed to Python.
#[pyfunction]
fn hyp_distance(ax: f32, ay: f32, az: f32, bx: f32, by: f32, bz: f32) -> f32 {
    ling_physics::hyperbolic::distance(Vec3::new(ax, ay, az), Vec3::new(bx, by, bz))
}

#[pyfunction]
fn exp_map(
    bx: f32, by: f32, bz: f32,
    vx: f32, vy: f32, vz: f32,
) -> (f32, f32, f32) {
    let v = ling_physics::hyperbolic::exp_map(Vec3::new(bx, by, bz), Vec3::new(vx, vy, vz));
    (v.x, v.y, v.z)
}

#[pyfunction]
fn log_map(
    bx: f32, by: f32, bz: f32,
    tx: f32, ty: f32, tz: f32,
) -> (f32, f32, f32) {
    let v = ling_physics::hyperbolic::log_map(Vec3::new(bx, by, bz), Vec3::new(tx, ty, tz));
    (v.x, v.y, v.z)
}

// ── NetClient ─────────────────────────────────────────────────────────────────

/// Asynchronous WebSocket client for netplay.
///
/// Usage from Python:
///   net = ling_py.NetClient()
///   net.connect("ws://host:8765")   # non-blocking, spawns background thread
///   net.send('{"type":"state",...}')
///   msg = net.try_recv()            # returns None or str
#[pyclass(name = "NetClient")]
struct PyNetClient {
    outbox:    Arc<Mutex<Vec<String>>>,
    inbox:     Arc<Mutex<VecDeque<String>>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
}

#[pymethods]
impl PyNetClient {
    #[new]
    fn new() -> Self {
        PyNetClient {
            outbox:    Arc::new(Mutex::new(Vec::new())),
            inbox:     Arc::new(Mutex::new(VecDeque::new())),
            connected: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Connect to a WebSocket server (non-blocking; returns immediately).
    fn connect(&self, url: String) {
        let outbox    = Arc::clone(&self.outbox);
        let inbox     = Arc::clone(&self.inbox);
        let connected = Arc::clone(&self.connected);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime build");
            rt.block_on(ws_run(url, outbox, inbox, connected));
        });
    }

    /// Queue a message to be sent to the server.
    fn send(&self, msg: String) {
        if let Ok(mut ob) = self.outbox.lock() {
            ob.push(msg);
        }
    }

    /// Pop one incoming message, or return None.
    fn try_recv(&self) -> Option<String> {
        self.inbox.lock().ok()?.pop_front()
    }

    fn is_connected(&self) -> bool {
        self.connected.load(std::sync::atomic::Ordering::Relaxed)
    }
}

async fn ws_run(
    url:       String,
    outbox:    Arc<Mutex<Vec<String>>>,
    inbox:     Arc<Mutex<VecDeque<String>>>,
    connected: Arc<std::sync::atomic::AtomicBool>,
) {
    use futures_util::{SinkExt, StreamExt};
    use tokio::time::{sleep, Duration};
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    let ws = match connect_async(&url).await {
        Ok((ws, _)) => ws,
        Err(e) => { eprintln!("[ling-py netplay] connect failed ({url}): {e}"); return; }
    };
    connected.store(true, std::sync::atomic::Ordering::Relaxed);
    eprintln!("[ling-py netplay] connected to {url}");

    let (mut write, mut read) = ws.split();

    loop {
        // Drain outbox → WebSocket
        let msgs: Vec<String> = if let Ok(mut ob) = outbox.lock() {
            std::mem::take(&mut *ob)
        } else {
            vec![]
        };
        for m in msgs {
            if write.send(Message::Text(m)).await.is_err() { break; }
        }

        // Read one incoming frame or yield after 1 ms
        tokio::select! {
            frame = read.next() => {
                match frame {
                    Some(Ok(Message::Text(s))) => {
                        if let Ok(mut ib) = inbox.lock() {
                            ib.push_back(s.to_string());
                            while ib.len() > 128 { ib.pop_front(); }
                        }
                    }
                    Some(Ok(_)) => {} // binary, ping, pong — ignore
                    _           => break, // error or close frame
                }
            }
            _ = sleep(Duration::from_millis(1)) => {}
        }
    }

    connected.store(false, std::sync::atomic::Ordering::Relaxed);
    eprintln!("[ling-py netplay] disconnected from {url}");
}

// ── Module root ───────────────────────────────────────────────────────────────

#[pymodule]
fn ling_py(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyAudioEngine>()?;
    m.add_class::<PyHyperbolicWorld>()?;
    m.add_class::<PyNetClient>()?;

    m.add_function(wrap_pyfunction!(hyp_distance, m)?)?;
    m.add_function(wrap_pyfunction!(exp_map, m)?)?;
    m.add_function(wrap_pyfunction!(log_map, m)?)?;

    Ok(())
}

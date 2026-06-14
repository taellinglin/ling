//! Pluggable device backends.
//!
//! The crate core is platform-agnostic: it has no idea where input comes from.
//! A *backend* bridges an OS/runtime API to two simple jobs — **poll** raw
//! device data into [`InputEvent`]s (updating the [`DeviceRegistry`]), and
//! **apply** outbound [`Rumble`] commands. Implement [`InputBackend`] for any
//! source: `gilrs` (native, behind the `gilrs` feature), SDL3, OpenXR, the web
//! Gamepad API, or your test harness.

use crate::device::{DeviceId, DeviceRegistry};
use crate::event::InputEvent;
use crate::haptics::Rumble;

/// Bridges a platform input source to the Ling input core.
pub trait InputBackend {
    /// Drain the platform's pending input, append translated events to `out`,
    /// and reflect connect/disconnect into `registry`.
    fn poll(&mut self, registry: &mut DeviceRegistry, out: &mut Vec<InputEvent>);

    /// Apply a rumble command to a device (no-op if it has no motors).
    fn set_rumble(&mut self, _device: DeviceId, _rumble: Rumble) {}

    /// Human-readable backend name (for diagnostics).
    fn name(&self) -> &str {
        "backend"
    }
}

/// A backend you drive by hand — push events in, read rumble out. Ideal for
/// tests, replays, web/JS bridges, and platforms without a native crate yet.
#[derive(Debug, Default)]
pub struct ManualBackend {
    queued: Vec<InputEvent>,
    /// The last rumble requested per device (a renderer/host can apply these).
    pub rumble: Vec<(DeviceId, Rumble)>,
}

impl ManualBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue an event to be delivered on the next [`poll`](InputBackend::poll).
    pub fn push(&mut self, ev: InputEvent) {
        self.queued.push(ev);
    }
}

impl InputBackend for ManualBackend {
    fn poll(&mut self, _registry: &mut DeviceRegistry, out: &mut Vec<InputEvent>) {
        out.append(&mut self.queued);
    }

    fn set_rumble(&mut self, device: DeviceId, rumble: Rumble) {
        self.rumble.push((device, rumble));
    }

    fn name(&self) -> &str {
        "manual"
    }
}

#[cfg(feature = "gilrs")]
#[path = "backend_gilrs.rs"]
mod gilrs_backend;
#[cfg(feature = "gilrs")]
pub use gilrs_backend::GilrsBackend;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_backend_round_trips_events_and_rumble() {
        let mut b = ManualBackend::new();
        b.push(InputEvent::DeviceDisconnected(1));
        let mut reg = DeviceRegistry::new();
        let mut out = Vec::new();
        b.poll(&mut reg, &mut out);
        assert_eq!(out.len(), 1);
        b.set_rumble(0, Rumble::both(1.0));
        assert_eq!(b.rumble.len(), 1);
    }
}

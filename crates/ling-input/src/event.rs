//! Unified input events — one stream for every device.
//!
//! Backends translate platform input into [`InputEvent`]s; the
//! [`Sensorium`](crate::Sensorium) consumes them. Events are serializable so
//! they can be recorded for replays or shipped over the wire for rollback
//! netcode.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

use crate::device::{DeviceId, DeviceInfo};
use crate::gamepad::{GamepadAxis, GamepadButton};
use crate::gesture::Gesture;
use crate::motion::MotionSample;
use crate::touch::Touch;
use crate::vr::Pose;

/// Lifecycle phase of an action event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum ActionPhase {
    Started,
    Ongoing,
    Ended,
}

/// A single timestamped-ish input event (ordering carries the timing).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum InputEvent {
    DeviceConnected(DeviceInfo),
    DeviceDisconnected(DeviceId),
    Button { device: DeviceId, button: GamepadButton, pressed: bool },
    Axis { device: DeviceId, axis: GamepadAxis, value: f32 },
    Touch(Touch),
    Gesture(Gesture),
    Motion { device: DeviceId, sample: MotionSample },
    Pose { device: DeviceId, pose: Pose },
    /// A resolved high-level action fired (name + analog value).
    Action { name: String, phase: ActionPhase, value: f32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variants_construct() {
        let e = InputEvent::DeviceDisconnected(3);
        assert_eq!(e, InputEvent::DeviceDisconnected(3));
    }
}

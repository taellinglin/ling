//! Device registry — enumeration, capabilities, hotplug, player assignment.
//!
//! The registry is the source of truth for *what is plugged in*. Each device
//! reports a [`DeviceKind`] and a [`Capabilities`] set so the game can adapt
//! (offer rumble only if present, show touch controls only on a touchscreen,
//! enable gyro-aim only on an IMU device). Hotplug is event-driven.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Stable handle for a connected device within one session.
pub type DeviceId = u32;

/// Broad category of an input device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum DeviceKind {
    Gamepad,
    Joystick,
    Wheel,
    Keyboard,
    Mouse,
    Touchscreen,
    Pen,
    VrHeadset,
    VrController,
    MotionSensor,
    Other,
}

/// What a device can do — drives feature gating in the game.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Capabilities {
    pub rumble: bool,
    pub trigger_haptics: bool,
    pub adaptive_triggers: bool,
    pub gyro: bool,
    pub accelerometer: bool,
    pub touchpad: bool,
    pub leds: bool,
    pub battery: bool,
    pub pose_6dof: bool,
    pub hand_tracking: bool,
    pub eye_tracking: bool,
    pub pressure: bool,
}

impl Capabilities {
    /// A typical modern wireless gamepad (rumble + motion + battery).
    #[must_use]
    pub fn modern_gamepad() -> Self {
        Self {
            rumble: true,
            trigger_haptics: true,
            adaptive_triggers: true,
            gyro: true,
            accelerometer: true,
            touchpad: true,
            leds: true,
            battery: true,
            ..Self::default()
        }
    }

    /// A typical XR controller.
    #[must_use]
    pub fn xr_controller() -> Self {
        Self {
            rumble: true,
            gyro: true,
            accelerometer: true,
            battery: true,
            pose_6dof: true,
            hand_tracking: true,
            ..Self::default()
        }
    }
}

/// Description + live status of one device.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub kind: DeviceKind,
    pub name: String,
    pub vendor_id: u16,
    pub product_id: u16,
    pub caps: Capabilities,
    /// Assigned player slot (`0`-based), if any.
    pub player: Option<u8>,
    pub connected: bool,
    /// Battery level `0..=1`, or `<0` when unknown / wired.
    pub battery: f32,
}

impl DeviceInfo {
    #[must_use]
    pub fn new(id: DeviceId, kind: DeviceKind, name: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            vendor_id: 0,
            product_id: 0,
            caps: Capabilities::default(),
            player: None,
            connected: true,
            battery: -1.0,
        }
    }
}

/// The set of devices the system currently knows about.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct DeviceRegistry {
    devices: Vec<DeviceInfo>,
    next_id: DeviceId,
}

impl DeviceRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a device, assigning and returning a fresh [`DeviceId`].
    pub fn add(&mut self, mut info: DeviceInfo) -> DeviceId {
        let id = self.next_id;
        self.next_id += 1;
        info.id = id;
        info.connected = true;
        self.devices.push(info);
        id
    }

    /// Mark a device disconnected (kept for a frame so listeners can react).
    pub fn disconnect(&mut self, id: DeviceId) {
        if let Some(d) = self.get_mut(id) {
            d.connected = false;
            d.player = None;
        }
    }

    /// Drop disconnected devices.
    pub fn prune(&mut self) {
        self.devices.retain(|d| d.connected);
    }

    #[must_use]
    pub fn get(&self, id: DeviceId) -> Option<&DeviceInfo> {
        self.devices.iter().find(|d| d.id == id)
    }

    pub fn get_mut(&mut self, id: DeviceId) -> Option<&mut DeviceInfo> {
        self.devices.iter_mut().find(|d| d.id == id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &DeviceInfo> {
        self.devices.iter()
    }

    pub fn of_kind(&self, kind: DeviceKind) -> impl Iterator<Item = &DeviceInfo> {
        self.devices
            .iter()
            .filter(move |d| d.kind == kind && d.connected)
    }

    /// First connected device assigned to `player`.
    #[must_use]
    pub fn for_player(&self, player: u8) -> Option<&DeviceInfo> {
        self.devices
            .iter()
            .find(|d| d.connected && d.player == Some(player))
    }

    /// Assign the lowest free player slot to a device; returns the slot.
    pub fn assign_player(&mut self, id: DeviceId) -> Option<u8> {
        let taken: Vec<u8> = self.devices.iter().filter_map(|d| d.player).collect();
        let slot = (0u8..=255).find(|s| !taken.contains(s))?;
        self.get_mut(id)?.player = Some(slot);
        Some(slot)
    }

    #[must_use]
    pub fn count(&self) -> usize {
        self.devices.iter().filter(|d| d.connected).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_assign_disconnect() {
        let mut r = DeviceRegistry::new();
        let mut info = DeviceInfo::new(0, DeviceKind::Gamepad, "Pad 1");
        info.caps = Capabilities::modern_gamepad();
        let id = r.add(info);
        assert_eq!(r.assign_player(id), Some(0));
        assert_eq!(r.for_player(0).map(|d| d.id), Some(id));
        assert!(r.get(id).unwrap().caps.rumble);
        r.disconnect(id);
        r.prune();
        assert_eq!(r.count(), 0);
    }
}

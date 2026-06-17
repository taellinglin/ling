//! Game audio stubs — delegates to ling-audio at runtime.

/// A handle to a loaded sound asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SoundHandle(pub u32);

/// Simple in-game audio mixer state.
#[derive(Debug, Default)]
pub struct AudioMixer {
    next_handle: u32,
}

impl AudioMixer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a sound; returns a reusable handle.
    pub fn register(&mut self) -> SoundHandle {
        let h = SoundHandle(self.next_handle);
        self.next_handle += 1;
        h
    }
}

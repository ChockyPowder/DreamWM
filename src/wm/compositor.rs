//! Capability checks for dreamWM's planned in-process X11 compositor.
//!
//! This module deliberately does not redirect windows yet. Redirecting without
//! a working repaint path would make the desktop appear blank. The probe gives
//! startup a clear, actionable report before the compositor is enabled.

use std::error::Error;

use x11rb::connection::Connection;

/// X11 extensions required by the native compositor's first implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompositorCapabilities {
    pub composite: bool,
    pub damage: bool,
    pub render: bool,
    pub present: bool,
}

impl CompositorCapabilities {
    /// Query extension availability on the active X server.
    pub(super) fn query<C: Connection>(conn: &C) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            composite: conn.extension_information("Composite")?.is_some(),
            damage: conn.extension_information("DAMAGE")?.is_some(),
            render: conn.extension_information("RENDER")?.is_some(),
            present: conn.extension_information("Present")?.is_some(),
        })
    }

    /// Whether the server exposes the complete baseline extension set.
    pub(super) fn supports_native_compositing(self) -> bool {
        self.composite && self.damage && self.render && self.present
    }
}

#[cfg(test)]
mod tests {
    use super::CompositorCapabilities;

    #[test]
    fn native_compositing_requires_every_baseline_extension() {
        let complete = CompositorCapabilities {
            composite: true,
            damage: true,
            render: true,
            present: true,
        };
        assert!(complete.supports_native_compositing());

        assert!(!CompositorCapabilities {
            present: false,
            ..complete
        }
        .supports_native_compositing());
        assert!(!CompositorCapabilities {
            composite: false,
            ..complete
        }
        .supports_native_compositing());
    }
}

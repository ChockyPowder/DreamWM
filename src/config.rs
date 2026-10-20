//! User-tunable constants and keyboard layout assumptions.

pub(crate) const BORDER_PX: u16 = 2;
pub(crate) const MIN_SIZE: u16 = 1;
pub(crate) const MASTER_STEP: f32 = 0.05;
pub(crate) const TAG_COUNT: u8 = 9;

// Default US keyboard keycodes. A keysym-based keymap is a follow-up task;
// these match the conventional Xorg/XKB US layout.
pub(crate) const KEY_Q: u8 = 24;
pub(crate) const KEY_P: u8 = 33;
pub(crate) const KEY_RETURN: u8 = 36;
pub(crate) const KEY_F: u8 = 41;
pub(crate) const KEY_H: u8 = 43;
pub(crate) const KEY_J: u8 = 44;
pub(crate) const KEY_K: u8 = 45;
pub(crate) const KEY_L: u8 = 46;
pub(crate) const KEY_M: u8 = 58;
pub(crate) const KEY_SPACE: u8 = 65;
pub(crate) const KEY_T: u8 = 28;
pub(crate) const KEY_1: u8 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Layout {
    Tile,
    Monocle,
    Floating,
}

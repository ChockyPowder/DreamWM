//! X11 window-manager identity properties (EWMH).

use std::error::Error;
use x11rb::{connection::Connection, wrapper::ConnectionExt as _, protocol::xproto::{AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, WindowClass}};
use super::DreamWm;

impl DreamWm {
    // Publish the EWMH identity properties used by tools such as neofetch.
    pub(super) fn setup_wm_identity(&self) -> Result<(), Box<dyn Error>> {
        let supporting_wm_check = self.conn
            .intern_atom(false, b"_NET_SUPPORTING_WM_CHECK")?
            .reply()?
            .atom;
        let net_wm_name = self.conn
            .intern_atom(false, b"_NET_WM_NAME")?
            .reply()?
            .atom;
        let utf8_string = self.conn
            .intern_atom(false, b"UTF8_STRING")?
            .reply()?
            .atom;

        let screen = self.conn.setup().roots
            .iter()
            .find(|screen| screen.root == self.root)
            .ok_or("dreamWM: couldn't find the X11 screen for the root window")?;
        let support_window = self.conn.generate_id()?;
        self.conn.create_window(
            screen.root_depth,
            support_window,
            self.root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &CreateWindowAux::new(),
        )?.check()?;

        self.conn.change_property32(
            PropMode::REPLACE,
            self.root,
            supporting_wm_check,
            AtomEnum::WINDOW,
            &[support_window],
        )?.check()?;
        self.conn.change_property32(
            PropMode::REPLACE,
            support_window,
            supporting_wm_check,
            AtomEnum::WINDOW,
            &[support_window],
        )?.check()?;
        self.conn.change_property8(
            PropMode::REPLACE,
            support_window,
            net_wm_name,
            utf8_string,
            b"dreamWM",
        )?.check()?;
        self.conn.change_property8(
            PropMode::REPLACE,
            support_window,
            AtomEnum::WM_NAME,
            AtomEnum::STRING,
            b"dreamWM",
        )?.check()?;
        Ok(())
    }
}

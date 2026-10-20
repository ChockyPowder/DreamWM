//! Keyboard grabs, shortcuts, focus navigation, and child processes.

use std::{error::Error, process::Command};
use x11rb::{connection::Connection, protocol::{xproto::{AtomEnum, ClientMessageData, ClientMessageEvent, ConnectionExt as _, EventMask, GrabMode, KeyButMask, ModMask}, Event}};
use crate::config::*;
use super::DreamWm;

impl DreamWm {
    pub(super) fn grab_keys(&self) -> Result<(), Box<dyn Error>> {
        // The modifier/key combinations follow the standard dwm defaults:
        // Mod1 is Alt; Shift modifies tag, quit, terminal and floating actions.
        let mut bindings: Vec<(u8, ModMask)> = vec![
            (KEY_J, ModMask::M1),
            (KEY_K, ModMask::M1),
            (KEY_RETURN, ModMask::M1),
            (KEY_RETURN, ModMask::M1 | ModMask::SHIFT),
            (KEY_H, ModMask::M1),
            (KEY_L, ModMask::M1),
            (KEY_T, ModMask::M1),
            (KEY_F, ModMask::M1),
            (KEY_M, ModMask::M1),
            (KEY_SPACE, ModMask::M1),
            (KEY_SPACE, ModMask::M1 | ModMask::SHIFT),
            (KEY_P, ModMask::M1),
            (KEY_Q, ModMask::M1 | ModMask::SHIFT),
            (KEY_C, ModMask::M1 | ModMask::SHIFT),
        ];
        for index in 0..TAG_COUNT {
            bindings.push((KEY_1 + index, ModMask::M1));
            bindings.push((KEY_1 + index, ModMask::M1 | ModMask::SHIFT));
        }

        for (keycode, modifiers) in bindings {
            self.conn.grab_key(
                false, self.root, modifiers, keycode, GrabMode::ASYNC, GrabMode::ASYNC,
            )?;
            self.conn.grab_key(
                false, self.root, modifiers | ModMask::LOCK, keycode,
                GrabMode::ASYNC, GrabMode::ASYNC,
            )?;
        }
        Ok(())
    }
    pub(super) fn keypress(&mut self, keycode: u8, state: KeyButMask) -> Result<(), Box<dyn Error>> {
        if !state.contains(KeyButMask::MOD1) {
            return Ok(());
        }
        let shift = state.contains(KeyButMask::SHIFT);
        match (keycode, shift) {
            (KEY_Q, true) => self.running = false,
            (KEY_C, true) => self.close_focused()?,
            (KEY_J, false) => self.focus_step(1)?,
            (KEY_K, false) => self.focus_step(-1)?,
            (KEY_RETURN, false) => self.zoom()?,
            (KEY_RETURN, true) => self.spawn_from_env("DREAMWM_TERMINAL", "st"),
            (KEY_H, false) => {
                self.master_factor = (self.master_factor - MASTER_STEP).clamp(0.1, 0.9);
                self.arrange()?;
            }
            (KEY_L, false) => {
                self.master_factor = (self.master_factor + MASTER_STEP).clamp(0.1, 0.9);
                self.arrange()?;
            }
            (KEY_T, false) => {
                self.layout = Layout::Tile;
                self.arrange()?;
            }
            (KEY_F, false) => {
                self.layout = Layout::Floating;
                self.arrange()?;
            }
            (KEY_M, false) => {
                self.layout = Layout::Monocle;
                self.arrange()?;
            }
            (KEY_SPACE, false) => {
                self.layout = match self.layout {
                    Layout::Tile => Layout::Floating,
                    Layout::Floating => Layout::Monocle,
                    Layout::Monocle => Layout::Tile,
                };
                self.arrange()?;
            }
            (KEY_SPACE, true) => self.toggle_floating()?,
            (KEY_P, false) => self.bar.open(&self.conn, self.root)?,
            (key, shifted) if (KEY_1..KEY_1 + TAG_COUNT).contains(&key) => {
                let tag = key - KEY_1;
                if shifted {
                    self.tag_focused(tag)?;
                } else {
                    self.view_tag(tag)?;
                }
            }
            _ => {}
        }
        self.conn.flush()?;
        Ok(())
    }
    pub(super) fn close_focused(&self) -> Result<(), Box<dyn Error>> {
        let Some(window) = self.focused else { return Ok(()) };
        let protocols = self.conn.intern_atom(false, b"WM_PROTOCOLS")?.reply()?.atom;
        let delete = self.conn.intern_atom(false, b"WM_DELETE_WINDOW")?.reply()?.atom;
        let supports_delete = self.conn
            .get_property(false, window, protocols, AtomEnum::ATOM, 0, 32)?
            .reply()?
            .value32()
            .is_some_and(|atoms| atoms.into_iter().any(|atom| atom == delete));
        if supports_delete {
            let event = ClientMessageEvent::new(
                32,
                window,
                protocols,
                ClientMessageData::from([delete, x11rb::CURRENT_TIME, 0, 0, 0]),
            );
            self.conn.send_event(false, window, EventMask::NO_EVENT, event)?;
        } else {
            // Some simple X11 clients do not implement WM_DELETE_WINDOW.
            self.conn.kill_client(window)?;
        }
        self.conn.flush()?;
        Ok(())
    }

    pub(super) fn spawn_from_env(&self, variable: &str, fallback: &str) {
        let program = std::env::var(variable).unwrap_or_else(|_| fallback.to_string());
        self.spawn_command(&program);
    }
    pub(super) fn spawn_command(&self, program: &str) {
        match Command::new(program).spawn() {
            Ok(_) => {}
            Err(error) => eprintln!("dreamWM: couldn't launch {program}: {error}"),
        }
    }
    pub(super) fn focus_step(&mut self, direction: isize) -> Result<(), Box<dyn Error>> {
        let visible = self.visible_clients();
        if visible.is_empty() {
            return Ok(());
        }
        let current = self.focused
            .and_then(|window| visible.iter().position(|&w| w == window))
            .unwrap_or(0);
        let count = visible.len() as isize;
        let next = (current as isize + direction).rem_euclid(count) as usize;
        self.focus(Some(visible[next]))
    }
    pub(super) fn zoom(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(window) = self.focused {
            if let Some(index) = self.clients.iter().position(|&w| w == window) {
                self.clients.remove(index);
                self.clients.insert(0, window);
                self.arrange()?;
                self.focus(Some(window))?;
            }
        }
        Ok(())
    }
    pub(super) fn view_tag(&mut self, tag: u8) -> Result<(), Box<dyn Error>> {
        self.current_tag = tag;
        self.bar.set_workspace(&self.conn, tag)?;
        self.sync_visibility()
    }
    pub(super) fn tag_focused(&mut self, tag: u8) -> Result<(), Box<dyn Error>> {
        if let Some(window) = self.focused {
            self.client_tags.insert(window, self.tag_mask(tag));
            self.sync_visibility()?;
        }
        Ok(())
    }
    pub(super) fn toggle_floating(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(window) = self.focused {
            if !self.floating_clients.remove(&window) {
                self.floating_clients.insert(window);
            }
            self.arrange()?;
        }
        Ok(())
    }
}

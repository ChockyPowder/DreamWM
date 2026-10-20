use std::{error::Error, process};

use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{
            ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _,
            EventMask, GrabMode, InputFocus, KeyButMask, ModMask, StackMode, Window,
        },
        Event,
    },
    rust_connection::RustConnection,
};

const BORDER_PX: u16 = 2;
const MASTER_FACTOR: f32 = 0.55;
const MIN_SIZE: u16 = 1;

// Default US XKB keycodes. These are intentionally kept in one place; proper
// keysym-based bindings can be added without changing the window manager core.
const KEY_Q: u8 = 24;
const KEY_RETURN: u8 = 36;
const KEY_H: u8 = 43;
const KEY_J: u8 = 44;
const KEY_K: u8 = 45;
const KEY_L: u8 = 46;
const KEY_SPACE: u8 = 65;
const KEY_1: u8 = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    Tile,
    Monocle,
    Floating,
}

struct DreamWm {
    conn: RustConnection,
    root: Window,
    screen_width: u16,
    screen_height: u16,
    clients: Vec<Window>,
    focused: Option<Window>,
    layout: Layout,
    running: bool,
}

impl DreamWm {
    fn new() -> Result<Self, Box<dyn Error>> {
        let (conn, screen_num) = x11rb::connect(None)?;
        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let screen_width = screen.width_in_pixels;
        let screen_height = screen.height_in_pixels;

        // Fail immediately and clearly if another window manager owns the root.
        conn.change_window_attributes(
            root,
            &ChangeWindowAttributesAux::new().event_mask(
                EventMask::SUBSTRUCTURE_REDIRECT
                    | EventMask::SUBSTRUCTURE_NOTIFY
                    | EventMask::BUTTON_PRESS
                    | EventMask::STRUCTURE_NOTIFY,
            ),
        )?
        .check()?;

        let mut wm = Self {
            conn,
            root,
            screen_width,
            screen_height,
            clients: Vec::new(),
            focused: None,
            layout: Layout::Tile,
            running: true,
        };
        wm.grab_keys()?;
        wm.conn.flush()?;
        Ok(wm)
    }

    fn grab_keys(&self) -> Result<(), Box<dyn Error>> {
        let bindings = [
            KEY_Q, KEY_RETURN, KEY_H, KEY_J, KEY_K, KEY_L, KEY_SPACE,
            KEY_1, KEY_1 + 1, KEY_1 + 2, KEY_1 + 3, KEY_1 + 4,
            KEY_1 + 5, KEY_1 + 6, KEY_1 + 7, KEY_1 + 8,
        ];
        for keycode in bindings {
            self.conn.grab_key(
                false,
                self.root,
                ModMask::M4,
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )?;
            // Also accept Caps Lock, a common dwm quality-of-life detail.
            self.conn.grab_key(
                false,
                self.root,
                ModMask::M4 | ModMask::LOCK,
                keycode,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
            )?;
        }
        Ok(())
    }

    fn run(&mut self) -> Result<(), Box<dyn Error>> {
        eprintln!("dreamWM: running on X11 (early development build)");
        while self.running {
            let event = self.conn.wait_for_event()?;
            match event {
                Event::MapRequest(event) => self.manage(event.window)?,
                Event::ConfigureRequest(event) => {
                    let aux = ConfigureWindowAux::new()
                        .x(event.x as i32)
                        .y(event.y as i32)
                        .width(event.width as u32)
                        .height(event.height as u32)
                        .border_width(event.border_width as u32)
                        .sibling(event.sibling)
                        .stack_mode(event.stack_mode);
                    self.conn.configure_window(event.window, &aux)?;
                    self.conn.flush()?;
                }
                Event::DestroyNotify(event) => self.unmanage(event.window)?,
                Event::UnmapNotify(event) => self.unmanage(event.window)?,
                Event::KeyPress(event) => self.keypress(event.detail, event.state)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn manage(&mut self, window: Window) -> Result<(), Box<dyn Error>> {
        if self.clients.contains(&window) {
            self.conn.map_window(window)?;
            return Ok(());
        }

        self.conn.configure_window(
            window,
            &ConfigureWindowAux::new().border_width(BORDER_PX as u32),
        )?;
        self.conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().border_pixel(0x444444),
        )?;
        self.conn.map_window(window)?;
        self.clients.push(window);
        self.focus(Some(window))?;
        self.arrange()?;
        Ok(())
    }

    fn unmanage(&mut self, window: Window) -> Result<(), Box<dyn Error>> {
        if let Some(index) = self.clients.iter().position(|&client| client == window) {
            self.clients.remove(index);
            if self.focused == Some(window) {
                self.focused = self.clients.last().copied();
            }
            self.focus(self.focused)?;
            self.arrange()?;
        }
        Ok(())
    }

    fn focus(&mut self, window: Option<Window>) -> Result<(), Box<dyn Error>> {
        self.focused = window;
        if let Some(window) = window {
            self.conn.change_window_attributes(
                window,
                &ChangeWindowAttributesAux::new().border_pixel(0x8fbcbb),
            )?;
            self.conn.set_input_focus(InputFocus::POINTER_ROOT, window, x11rb::CURRENT_TIME)?;
            self.conn.configure_window(
                window,
                &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )?;
        }
        for &client in &self.clients {
            if Some(client) != window {
                self.conn.change_window_attributes(
                    client,
                    &ChangeWindowAttributesAux::new().border_pixel(0x444444),
                )?;
            }
        }
        self.conn.flush()?;
        Ok(())
    }

    fn keypress(&mut self, keycode: u8, state: KeyButMask) -> Result<(), Box<dyn Error>> {
        if !state.contains(KeyButMask::MOD4) {
            return Ok(());
        }
        match keycode {
            KEY_Q => self.running = false,
            KEY_J => self.focus_step(1)?,
            KEY_K => self.focus_step(-1)?,
            KEY_RETURN => self.zoom()?,
            KEY_H => {
                // Keep the first milestone deliberately simple; the master
                // factor becomes configurable once the core is split into modules.
                self.layout = Layout::Tile;
                self.arrange()?;
            }
            KEY_L => {
                self.layout = Layout::Tile;
                self.arrange()?;
            }
            KEY_SPACE => {
                self.layout = match self.layout {
                    Layout::Tile => Layout::Monocle,
                    Layout::Monocle => Layout::Floating,
                    Layout::Floating => Layout::Tile,
                };
                self.arrange()?;
            }
            key if (KEY_1..KEY_1 + 9).contains(&key) => {
                // Tags/workspaces are the next milestone. Number keys are
                // grabbed now so the final keybinding model remains dwm-like.
            }
            _ => {}
        }
        self.conn.flush()?;
        Ok(())
    }

    fn focus_step(&mut self, direction: isize) -> Result<(), Box<dyn Error>> {
        if self.clients.is_empty() {
            return Ok(());
        }
        let current = self.focused
            .and_then(|window| self.clients.iter().position(|&w| w == window))
            .unwrap_or(0);
        let count = self.clients.len() as isize;
        let next = (current as isize + direction).rem_euclid(count) as usize;
        self.focus(Some(self.clients[next]))
    }

    fn zoom(&mut self) -> Result<(), Box<dyn Error>> {
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

    fn arrange(&mut self) -> Result<(), Box<dyn Error>> {
        if self.clients.is_empty() {
            return Ok(());
        }
        match self.layout {
            Layout::Floating => {}
            Layout::Monocle => {
                for &window in &self.clients {
                    self.place(window, 0, 0, self.screen_width, self.screen_height)?;
                }
            }
            Layout::Tile => {
                let n = self.clients.len();
                if n == 1 {
                    self.place(self.clients[0], 0, 0, self.screen_width, self.screen_height)?;
                } else {
                    let master_width = ((self.screen_width as f32) * MASTER_FACTOR) as u16;
                    self.place(self.clients[0], 0, 0, master_width, self.screen_height)?;
                    let stack_count = (n - 1) as u16;
                    let stack_width = self.screen_width.saturating_sub(master_width);
                    let each_height = (self.screen_height / stack_count).max(MIN_SIZE);
                    for (index, &window) in self.clients.iter().skip(1).enumerate() {
                        let y = (index as u32 * each_height as u32) as u16;
                        let height = if index + 1 == stack_count as usize {
                            self.screen_height.saturating_sub(y)
                        } else {
                            each_height
                        };
                        self.place(window, master_width as i32, y as i32, stack_width, height)?;
                    }
                }
            }
        }
        self.conn.flush()?;
        Ok(())
    }

    fn place(
        &self,
        window: Window,
        x: i32,
        y: i32,
        width: u16,
        height: u16,
    ) -> Result<(), Box<dyn Error>> {
        self.conn.configure_window(
            window,
            &ConfigureWindowAux::new()
                .x(x)
                .y(y)
                .width(width.saturating_sub(BORDER_PX * 2) as u32)
                .height(height.saturating_sub(BORDER_PX * 2) as u32)
                .border_width(BORDER_PX as u32),
        )?;
        Ok(())
    }
}

fn main() {
    match DreamWm::new().and_then(|mut wm| wm.run()) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("dreamWM: {error}");
            process::exit(1);
        }
    }
}

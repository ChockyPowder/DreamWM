use std::{
    collections::{HashMap, HashSet},
    error::Error,
    process::{self, Command},
};

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
const MIN_SIZE: u16 = 1;
const MASTER_STEP: f32 = 0.05;
const TAG_COUNT: u8 = 9;

// Default US keyboard keycodes. A keysym-based keymap is a follow-up task;
// these match the conventional Xorg/XKB US layout.
const KEY_Q: u8 = 24;
const KEY_P: u8 = 33;
const KEY_RETURN: u8 = 36;
const KEY_F: u8 = 41;
const KEY_H: u8 = 43;
const KEY_J: u8 = 44;
const KEY_K: u8 = 45;
const KEY_L: u8 = 46;
const KEY_M: u8 = 58;
const KEY_SPACE: u8 = 65;
const KEY_T: u8 = 28;
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
    client_tags: HashMap<Window, u16>,
    floating_clients: HashSet<Window>,
    hidden_clients: HashSet<Window>,
    focused: Option<Window>,
    current_tag: u8,
    layout: Layout,
    master_factor: f32,
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
            client_tags: HashMap::new(),
            floating_clients: HashSet::new(),
            hidden_clients: HashSet::new(),
            focused: None,
            current_tag: 0,
            layout: Layout::Tile,
            master_factor: 0.55,
            running: true,
        };
        wm.grab_keys()?;
        wm.scan_existing()?;
        wm.sync_visibility()?;
        wm.conn.flush()?;
        Ok(wm)
    }

    fn scan_existing(&mut self) -> Result<(), Box<dyn Error>> {
        let children = self.conn.query_tree(self.root)?.reply()?.children;
        for window in children {
            let attributes = self.conn.get_window_attributes(window)?.reply()?;
            if !attributes.override_redirect
                && attributes.map_state != x11rb::protocol::xproto::MapState::UNMAPPED
            {
                self.manage(window)?;
            }
        }
        Ok(())
    }

    fn grab_keys(&self) -> Result<(), Box<dyn Error>> {
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

    fn run(&mut self) -> Result<(), Box<dyn Error>> {
        eprintln!("dreamWM: running on X11 (early development build)");
        while self.running {
            match self.conn.wait_for_event()? {
                Event::MapRequest(event) => self.manage(event.window)?,
                Event::ConfigureRequest(event) => {
                    if self.layout == Layout::Floating
                        || self.floating_clients.contains(&event.window)
                        || !self.clients.contains(&event.window)
                    {
                        let aux = ConfigureWindowAux::new()
                            .x(event.x as i32)
                            .y(event.y as i32)
                            .width(event.width as u32)
                            .height(event.height as u32)
                            .border_width(event.border_width as u32)
                            .sibling(event.sibling)
                            .stack_mode(event.stack_mode);
                        self.conn.configure_window(event.window, &aux)?;
                    } else {
                        self.arrange()?;
                    }
                    self.conn.flush()?;
                }
                Event::DestroyNotify(event) => self.unmanage(event.window)?,
                Event::UnmapNotify(event) => {
                    // Unmaps initiated by tag switching are not client exits.
                    if !self.hidden_clients.contains(&event.window) {
                        self.unmanage(event.window)?;
                    }
                }
                Event::KeyPress(event) => self.keypress(event.detail, event.state)?,
                _ => {}
            }
        }
        Ok(())
    }

    fn manage(&mut self, window: Window) -> Result<(), Box<dyn Error>> {
        if self.clients.contains(&window) {
            if self.is_visible(window) {
                self.conn.map_window(window)?;
            }
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
        self.clients.push(window);
        self.client_tags.insert(window, self.tag_mask(self.current_tag));
        if self.is_visible(window) {
            self.conn.map_window(window)?;
            self.focus(Some(window))?;
        } else {
            self.hidden_clients.insert(window);
        }
        self.arrange()?;
        Ok(())
    }

    fn unmanage(&mut self, window: Window) -> Result<(), Box<dyn Error>> {
        if let Some(index) = self.clients.iter().position(|&client| client == window) {
            self.clients.remove(index);
            self.client_tags.remove(&window);
            self.floating_clients.remove(&window);
            self.hidden_clients.remove(&window);
            if self.focused == Some(window) {
                self.focused = self.visible_clients().last().copied();
            }
            self.focus(self.focused)?;
            self.arrange()?;
        }
        Ok(())
    }

    fn focus(&mut self, window: Option<Window>) -> Result<(), Box<dyn Error>> {
        let window = window.filter(|w| self.is_visible(*w));
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
            if Some(client) != window && !self.hidden_clients.contains(&client) {
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
        if !state.contains(KeyButMask::MOD1) {
            return Ok(());
        }
        let shift = state.contains(KeyButMask::SHIFT);
        match (keycode, shift) {
            (KEY_Q, true) => self.running = false,
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
            (KEY_P, false) => self.spawn_command("dmenu_run"),
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

    fn spawn_from_env(&self, variable: &str, fallback: &str) {
        let program = std::env::var(variable).unwrap_or_else(|_| fallback.to_string());
        self.spawn_command(&program);
    }

    fn spawn_command(&self, program: &str) {
        match Command::new(program).spawn() {
            Ok(_) => {}
            Err(error) => eprintln!("dreamWM: couldn't launch {program}: {error}"),
        }
    }

    fn focus_step(&mut self, direction: isize) -> Result<(), Box<dyn Error>> {
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

    fn view_tag(&mut self, tag: u8) -> Result<(), Box<dyn Error>> {
        self.current_tag = tag;
        self.sync_visibility()
    }

    fn tag_focused(&mut self, tag: u8) -> Result<(), Box<dyn Error>> {
        if let Some(window) = self.focused {
            self.client_tags.insert(window, self.tag_mask(tag));
            self.sync_visibility()?;
        }
        Ok(())
    }

    fn toggle_floating(&mut self) -> Result<(), Box<dyn Error>> {
        if let Some(window) = self.focused {
            if !self.floating_clients.remove(&window) {
                self.floating_clients.insert(window);
            }
            self.arrange()?;
        }
        Ok(())
    }

    fn tag_mask(&self, tag: u8) -> u16 {
        1u16 << tag
    }

    fn is_visible(&self, window: Window) -> bool {
        self.client_tags
            .get(&window)
            .is_some_and(|tags| tags & self.tag_mask(self.current_tag) != 0)
    }

    fn visible_clients(&self) -> Vec<Window> {
        self.clients.iter().copied().filter(|w| self.is_visible(*w)).collect()
    }

    fn sync_visibility(&mut self) -> Result<(), Box<dyn Error>> {
        let clients = self.clients.clone();
        for window in clients {
            if self.is_visible(window) {
                if self.hidden_clients.remove(&window) {
                    self.conn.map_window(window)?;
                }
            } else if self.hidden_clients.insert(window) {
                self.conn.unmap_window(window)?;
            }
        }
        if self.focused.is_some_and(|window| !self.is_visible(window)) {
            self.focused = self.visible_clients().last().copied();
        }
        self.focus(self.focused)?;
        self.arrange()
    }

    fn arrange(&mut self) -> Result<(), Box<dyn Error>> {
        if self.layout == Layout::Floating {
            return Ok(());
        }
        let tiled: Vec<Window> = self.visible_clients().into_iter()
            .filter(|window| !self.floating_clients.contains(window))
            .collect();
        if tiled.is_empty() {
            return Ok(());
        }

        match self.layout {
            Layout::Floating => {}
            Layout::Monocle => {
                for window in tiled {
                    self.place(window, 0, 0, self.screen_width, self.screen_height)?;
                }
            }
            Layout::Tile => {
                if tiled.len() == 1 {
                    self.place(tiled[0], 0, 0, self.screen_width, self.screen_height)?;
                } else {
                    let master_width =
                        ((self.screen_width as f32) * self.master_factor) as u16;
                    self.place(tiled[0], 0, 0, master_width, self.screen_height)?;
                    let stack_count = tiled.len() - 1;
                    let stack_width = self.screen_width.saturating_sub(master_width);
                    let base_height = (self.screen_height as usize / stack_count).max(1);
                    for (index, &window) in tiled.iter().skip(1).enumerate() {
                        let y = index * base_height;
                        let height = if index + 1 == stack_count {
                            (self.screen_height as usize).saturating_sub(y)
                        } else {
                            base_height
                        };
                        self.place(
                            window,
                            master_width as i32,
                            y as i32,
                            stack_width,
                            height.min(u16::MAX as usize) as u16,
                        )?;
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

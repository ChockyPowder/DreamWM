//! X11 window-manager lifecycle and client state.

mod bar;
mod compositor;
mod identity;
mod keys;
mod layout;

use std::{collections::{HashMap, HashSet}, error::Error, time::{Duration, Instant}};
use x11rb::{
    connection::Connection,
    protocol::{xproto::{ChangeWindowAttributesAux, ConfigureWindowAux, ConnectionExt as _, EventMask, InputFocus, StackMode, Window}, Event},
    rust_connection::RustConnection,
};
use crate::config::{Layout, BORDER_PX};
use bar::{BarAction, SearchBar};

pub struct DreamWm {
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
    bar: SearchBar,
}

impl DreamWm {
    pub fn new() -> Result<Self, Box<dyn Error>> {
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

        let bar = SearchBar::new(&conn, root, screen_width)?;
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
            bar,
        };
        wm.setup_wm_identity()?;
        let compositor = compositor::CompositorCapabilities::query(&wm.conn)?;
        eprintln!(
            "dreamWM: native compositor prerequisites: Composite={}, DAMAGE={}, RENDER={}, Present={}",
            compositor.composite, compositor.damage, compositor.render, compositor.present
        );
        if !compositor.supports_native_compositing() {
            eprintln!("dreamWM: tear-free compositing unavailable: the X server lacks one or more required extensions");
        }
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
    pub fn run(&mut self) -> Result<(), Box<dyn Error>> {
        eprintln!("dreamWM: running on X11 (early development build)");
        let mut last_status = Instant::now();
        while self.running {
            if last_status.elapsed() >= Duration::from_secs(1) {
                self.bar.tick(&self.conn)?;
                last_status = Instant::now();
            }
            match self.conn.poll_for_event()? {
                Some(event) => match event {
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
                        if !self.hidden_clients.contains(&event.window) {
                            self.unmanage(event.window)?;
                        }
                    }
                    Event::KeyPress(event) => {
                        if self.bar.is_active() {
                            match self.bar.handle_key(&self.conn, event.detail, event.state.bits(), self.focused)? {
                                BarAction::Launched(program) => SearchBar::launch(&program),
                                BarAction::SwitchWorkspace(workspace) => self.view_tag(workspace)?,
                                BarAction::Ignored | BarAction::Updated | BarAction::Closed => {}
                            }
                        } else {
                            self.keypress(event.detail, event.state)?;
                        }
                    }
                    Event::ButtonPress(event) => {
                        if !self.bar.is_active() {
                            if let Some(workspace) = self.bar.handle_button(event.event_x) {
                                self.view_tag(workspace)?;
                            }
                        }
                    }
                    Event::Expose(_) => self.bar.redraw(&self.conn)?,
                    _ => {}
                },
                None => std::thread::sleep(Duration::from_millis(25)),
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
        self.conn.configure_window(
            self.bar.window_id(),
            &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
        )?;
        self.conn.flush()?;
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
}

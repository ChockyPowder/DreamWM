//! Persistent top bar and keyboard-driven application search.

use std::{collections::BTreeSet, env, error::Error, fs, os::unix::fs::PermissionsExt, path::PathBuf, process::Command};
use x11rb::{
    connection::Connection,
    protocol::xproto::{
        AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
        EventMask, GrabMode, InputFocus, StackMode, Window, WindowClass,
    },
    rust_connection::RustConnection,
};

pub(super) const BAR_HEIGHT: u16 = 28;
const BG: u32 = 0x20242b;
const FG: u32 = 0xd8dee9;
const ACCENT: u32 = 0x88c0d0;

pub(super) enum BarAction {
    Ignored,
    Updated,
    Closed,
    Launched(String),
}

pub(super) struct SearchBar {
    window: Window,
    gc: u32,
    width: u16,
    active: bool,
    query: String,
    matches: Vec<String>,
    selected: usize,
    programs: Vec<String>,
}

impl SearchBar {
    pub(super) fn new(conn: &RustConnection, root: Window, width: u16) -> Result<Self, Box<dyn Error>> {
        let window = conn.generate_id()?;
        conn.create_window(
            x11rb::COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            width,
            BAR_HEIGHT,
            0,
            WindowClass::INPUT_OUTPUT,
            x11rb::COPY_FROM_PARENT,
            &CreateWindowAux::new()
                .override_redirect(1)
                .background_pixel(BG)
                .event_mask(EventMask::EXPOSURE | EventMask::KEY_PRESS),
        )?;
        let font = conn.generate_id()?;
        conn.open_font(font, b"fixed")?.check()?;
        let gc = conn.generate_id()?;
        conn.create_gc(
            gc,
            window,
            &CreateGCAux::new().foreground(FG).background(BG).font(font),
        )?;
        conn.map_window(window)?;
        conn.configure_window(window, &x11rb::protocol::xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        let programs = Self::discover_programs();
        let mut bar = Self {
            window, gc, width, active: false, query: String::new(),
            matches: Vec::new(), selected: 0, programs,
        };
        bar.refresh_matches();
        bar.draw(conn)?;
        Ok(bar)
    }

    fn discover_programs() -> Vec<String> {
        let mut names = BTreeSet::new();
        for directory in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
            let Ok(entries) = fs::read_dir(directory) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                let Ok(metadata) = fs::metadata(&path) else { continue };
                if metadata.is_file() && metadata.permissions().mode() & 0o111 != 0 {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if !name.is_empty() && !name.contains('/') {
                            names.insert(name.to_string());
                        }
                    }
                }
            }
        }
        names.into_iter().collect()
    }

    pub(super) fn is_active(&self) -> bool { self.active }

    pub(super) fn open(&mut self, conn: &RustConnection, root: Window) -> Result<(), Box<dyn Error>> {
        self.active = true;
        self.query.clear();
        self.selected = 0;
        self.refresh_matches();
        conn.grab_keyboard(false, root, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply()?;
        conn.map_window(self.window)?;
        conn.configure_window(self.window, &x11rb::protocol::xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        self.draw(conn)?;
        conn.flush()?;
        Ok(())
    }

    pub(super) fn handle_key(&mut self, conn: &RustConnection, keycode: u8, state: u16, root: Window, restore_focus: Option<Window>) -> Result<BarAction, Box<dyn Error>> {
        if !self.active { return Ok(BarAction::Ignored); }
        match keycode {
            9 => {
                self.close(conn, restore_focus)?;
                return Ok(BarAction::Closed);
            }
            36 => {
                if let Some(program) = self.matches.get(self.selected).cloned() {
                    self.close(conn, restore_focus)?;
                    return Ok(BarAction::Launched(program));
                }
                self.close(conn, restore_focus)?;
                return Ok(BarAction::Closed);
            }
            22 => { self.query.pop(); }
            111 => {
                if !self.matches.is_empty() {
                    self.selected = (self.selected + self.matches.len() - 1) % self.matches.len();
                }
            }
            116 => {
                if !self.matches.is_empty() {
                    self.selected = (self.selected + 1) % self.matches.len();
                }
            }
            23 => {
                if !self.matches.is_empty() {
                    self.selected = (self.selected + 1) % self.matches.len();
                }
            }
            key if key >= 10 => {
                if let Some(ch) = Self::keycode_to_char(key, state & 1 != 0) {
                    if !ch.is_control() && self.query.len() < 128 { self.query.push(ch); }
                }
            }
            _ => {}
        }
        self.refresh_matches();
        self.draw(conn)?;
        conn.flush()?;
        let _ = root;
        Ok(BarAction::Updated)
    }

    fn keycode_to_char(key: u8, shift: bool) -> Option<char> {
        let plain = match key {
            10..=18 => char::from(b'1' + key - 10),
            19 => '0',
            24..=33 => ['q','w','e','r','t','y','u','i','o','p'][(key - 24) as usize],
            34..=43 => ['[',']','a','s','d','f','g','h','j','k'][(key - 34) as usize],
            44..=53 => ['l',';','\'','`','z','x','c','v','b','n'][(key - 44) as usize],
            54..=58 => ['m',',','.','/','*'][(key - 54) as usize],
            65 => ' ',
            20 => '-',
            21 => '=',
            59 => ',',
            60 => '.',
            61 => '/',
            _ => return None,
        };
        if !shift { return Some(plain); }
        Some(match plain {
            'a'..='z' => plain.to_ascii_uppercase(),
            '1' => '!', '2' => '@', '3' => '#', '4' => '$', '5' => '%',
            '6' => '^', '7' => '&', '8' => '*', '9' => '(', '0' => ')',
            '-' => '_', '=' => '+', '[' => '{', ']' => '}', ';' => ':',
            '\'' => '"', '`' => '~', ',' => '<', '.' => '>', '/' => '?',
            other => other,
        })
    }

    fn refresh_matches(&mut self) {
        let needle = self.query.to_lowercase();
        self.matches = self.programs.iter()
            .filter(|program| needle.is_empty() || program.to_lowercase().contains(&needle))
            .take(8)
            .cloned()
            .collect();
        if self.selected >= self.matches.len() { self.selected = 0; }
    }

    fn draw(&self, conn: &RustConnection) -> Result<(), Box<dyn Error>> {
        conn.change_window_attributes(self.window, &ChangeWindowAttributesAux::new().background_pixel(BG))?;
        conn.clear_area(false, self.window, 0, 0, self.width, BAR_HEIGHT)?;
        let label = if self.active {
            format!(" dream search: {}_", self.query)
        } else {
            " dreamWM   |   Alt+P: dream search".to_string()
        };
        conn.change_gc(self.gc, &CreateGCAux::new().foreground(ACCENT))?;
        conn.image_text8(self.window, self.gc, 10, 18, label.as_bytes())?;
        if self.active {
            conn.change_gc(self.gc, &CreateGCAux::new().foreground(FG))?;
            let mut x = 10i16;
            for (index, program) in self.matches.iter().enumerate() {
                let marker = if index == self.selected { "  > " } else { "    " };
                let item = format!("{marker}{program}");
                if x as u16 + (item.len() as u16 * 8) > self.width.saturating_sub(10) { break; }
                conn.image_text8(self.window, self.gc, x, 18, item.as_bytes())?;
                x += (item.len() as i16 * 8).max(16);
            }
        }
        Ok(())
    }

    fn close(&mut self, conn: &RustConnection, restore_focus: Option<Window>) -> Result<(), Box<dyn Error>> {
        self.active = false;
        conn.ungrab_keyboard(x11rb::CURRENT_TIME)?;
        if let Some(window) = restore_focus {
            conn.set_input_focus(InputFocus::POINTER_ROOT, window, x11rb::CURRENT_TIME)?;
        }
        self.draw(conn)?;
        conn.flush()?;
        Ok(())
    }

    pub(super) fn launch(program: &str) {
        match Command::new(program).spawn() {
            Ok(_) => {}
            Err(error) => eprintln!("dreamWM: couldn't launch {program}: {error}"),
        }
    }
}

//! Persistent top bar and keyboard-driven application search.

use std::{
    collections::BTreeSet,
    env, error::Error, fs,
    io::{BufReader, Read},
    os::unix::fs::PermissionsExt,
    process::{Child, Command, Stdio},
    sync::mpsc::{self, Receiver},
    thread,
    time::{Duration, Instant, SystemTime},
};
use x11rb::{
    connection::Connection,
    protocol::xproto::{
        ChangeGCAux, ChangeWindowAttributesAux, ConnectionExt as _, CreateGCAux, CreateWindowAux,
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
    SwitchWorkspace(u8),
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
    current_workspace: u8,
    previous_cpu: Option<(u64, u64)>,
    cpu_history: Vec<u8>,
    status: String,
    cpu_segment: String,
    slow_segment: String,
    visualizer: String,
    cava_rx: Option<Receiver<Vec<u8>>>,
    cava_child: Option<Child>,
    last_cpu_sample: Instant,
    last_slow_sample: Instant,
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
                .event_mask(EventMask::EXPOSURE | EventMask::KEY_PRESS | EventMask::BUTTON_PRESS),
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
            matches: Vec::new(), selected: 0, programs, current_workspace: 0,
            previous_cpu: None, cpu_history: vec![0; 16], status: String::new(),
            cpu_segment: "CPU ................   0%".to_string(),
            slow_segment: String::new(),
            visualizer: "............".to_string(),
            cava_rx: None, cava_child: None,
            last_cpu_sample: Instant::now(), last_slow_sample: Instant::now() - Duration::from_secs(2),
        };
        bar.refresh_matches();
        bar.start_visualizer();
        bar.sample_status(true, true);
        bar.redraw(conn)?;
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
    pub(super) fn window_id(&self) -> Window { self.window }

    pub(super) fn set_workspace(&mut self, conn: &RustConnection, workspace: u8) -> Result<(), Box<dyn Error>> {
        self.current_workspace = workspace;
        self.redraw(conn)?;
        conn.flush()?;
        Ok(())
    }

    pub(super) fn handle_button(&self, x: i16) -> Option<u8> {
        // Nine clickable workspace labels, each 26 pixels wide.
        let relative = x as i32 - 8;
        if (0..(9 * 26)).contains(&relative) {
            Some((relative / 26) as u8)
        } else {
            None
        }
    }

    pub(super) fn open(&mut self, conn: &RustConnection, root: Window) -> Result<(), Box<dyn Error>> {
        self.active = true;
        self.query.clear();
        self.selected = 0;
        self.refresh_matches();
        conn.grab_keyboard(false, root, x11rb::CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)?.reply()?;
        conn.map_window(self.window)?;
        conn.configure_window(self.window, &x11rb::protocol::xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
        self.redraw(conn)?;
        conn.flush()?;
        Ok(())
    }

    pub(super) fn handle_key(&mut self, conn: &RustConnection, keycode: u8, state: u16, restore_focus: Option<Window>) -> Result<BarAction, Box<dyn Error>> {
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
        self.redraw(conn)?;
        conn.flush()?;
        Ok(BarAction::Updated)
    }

    fn keycode_to_char(key: u8, shift: bool) -> Option<char> {
        let plain = match key {
            10..=18 => char::from(b'1' + key - 10), 19 => '0',
            24..=33 => ['q','w','e','r','t','y','u','i','o','p'][(key - 24) as usize],
            34 => '[', 35 => ']',
            38..=46 => ['a','s','d','f','g','h','j','k','l'][(key - 38) as usize],
            47 => ';', 48 => '\'', 49 => '`',
            52..=58 => ['z','x','c','v','b','n','m'][(key - 52) as usize],
            59 => ',', 60 => '.', 61 => '/', 65 => ' ', 20 => '-', 21 => '=',
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

    fn start_visualizer(&mut self) {
        let Some(home) = env::var_os("HOME") else { return };
        let config = std::path::PathBuf::from(home).join(".config/cava/config_dwm");
        if !config.is_file() { return; }
        let child = Command::new("cava")
            .arg("-p").arg(config)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else { return };
        let Some(stdout) = child.stdout.take() else { let _ = child.kill(); return };
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut token = Vec::new();
            let mut frame = Vec::with_capacity(12);
            let mut byte = [0u8; 1];
            loop {
                match reader.read_exact(&mut byte) {
                    Ok(()) => {
                        if byte[0] == b';' || byte[0] == b'\\n' || byte[0] == b'\\r' {
                            if !token.is_empty() {
                                let value = std::str::from_utf8(&token).ok()
                                    .and_then(|s| s.trim().parse::<u8>().ok())
                                    .unwrap_or_else(|| token.first().copied().unwrap_or(b'0').saturating_sub(b'0'))
                                    .min(7);
                                frame.push(value);
                                token.clear();
                            }
                            if frame.len() >= 12 {
                                let _ = tx.send(frame.drain(..).take(12).collect());
                            }
                        } else {
                            token.push(byte[0]);
                            if token.len() > 16 { token.clear(); }
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        self.cava_rx = Some(rx);
        self.cava_child = Some(child);
    }

    pub(super) fn tick(&mut self, conn: &RustConnection) -> Result<(), Box<dyn Error>> {
        if let Some(rx) = &self.cava_rx {
            while let Ok(frame) = rx.try_recv() {
                const LEVELS: [char; 8] = ['.', ':', '-', '=', '+', '*', '#', '@'];
                self.visualizer = frame.iter().map(|v| LEVELS[(*v).min(7) as usize]).collect();
            }
        }
        let now = Instant::now();
        let cpu_due = now.duration_since(self.last_cpu_sample) >= Duration::from_millis(250);
        let slow_due = now.duration_since(self.last_slow_sample) >= Duration::from_secs(1);
        if cpu_due { self.last_cpu_sample = now; }
        if slow_due { self.last_slow_sample = now; }
        if cpu_due || slow_due {
            self.sample_status(cpu_due, slow_due);
            self.redraw(conn)?;
            conn.flush()?;
        }
        Ok(())
    }

    fn sample_status(&mut self, update_cpu: bool, update_slow: bool) {
        if update_cpu {
            let (cpu_label, cpu_usage) = Self::cpu_usage(&mut self.previous_cpu);
            self.cpu_history.push(cpu_usage);
            if self.cpu_history.len() > 16 { self.cpu_history.remove(0); }
            const LEVELS: [char; 8] = ['.', ':', '-', '=', '+', '*', '#', '@'];
            let graph: String = self.cpu_history.iter()
                .map(|value| LEVELS[((*value as usize) * 7 / 100).min(7)])
                .collect();
            self.cpu_segment = format!("CPU {graph} {cpu_label}");
        }
        if update_slow {
            let battery = Self::battery_status();
            let memory = Self::memory_status();
            let volume = Self::volume_status();
            let clock = Command::new("date").arg("+%a %d %b %H:%M").output()
                .ok().filter(|out| out.status.success())
                .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
                .unwrap_or_else(|| "--:--".to_string());
            let battery_segment = if battery.is_empty() { String::new() } else { format!("{battery}  ") };
            self.slow_segment = format!("{battery_segment}MEM {memory}  VOL {volume}  {}  {clock}", self.visualizer);
        } else if !self.slow_segment.is_empty() {
            // Update only the visualizer between one-second status refreshes.
            let mut segments: Vec<String> = self.slow_segment.split("  ").map(str::to_string).collect();
            if segments.len() >= 4 {
                let visualizer_index = segments.len() - 2;
                segments[visualizer_index] = self.visualizer.clone();
                self.slow_segment = segments.join("  ");
            }
        }
        self.status = format!("{}  {}", self.cpu_segment, self.slow_segment);
    }

    fn memory_status() -> String {
        fs::read_to_string("/proc/meminfo").ok().and_then(|contents| {
            let mut total = 0u64;
            let mut available = 0u64;
            for line in contents.lines() {
                if let Some(value) = line.strip_prefix("MemTotal:") {
                    total = value.split_whitespace().next()?.parse().ok()?;
                } else if let Some(value) = line.strip_prefix("MemAvailable:") {
                    available = value.split_whitespace().next()?.parse().ok()?;
                }
            }
            (total > 0).then_some(((total.saturating_sub(available)) * 100 / total) as u8)
        }).map(|value| format!("{value:>3}%")).unwrap_or_else(|| "--".to_string())
    }

    fn volume_status() -> String {
        let output = Command::new("wpctl").args(["get-volume", "@DEFAULT_AUDIO_SINK@"])
            .output();
        let Ok(output) = output else { return "--".to_string() };
        let text = String::from_utf8_lossy(&output.stdout);
        if text.contains("MUTED") { return "mute".to_string(); }
        let Some(value) = text.split_whitespace().find_map(|part| part.parse::<f32>().ok()) else {
            return "--".to_string();
        };
        format!("{:>3}%", (value * 100.0).round() as u32)
    }

    fn cpu_usage(previous: &mut Option<(u64, u64)>) -> (String, u8) {
        let parsed = fs::read_to_string("/proc/stat").ok().and_then(|contents| {
            let line = contents.lines().next()?;
            let mut values = line.split_whitespace();
            if values.next()? != "cpu" { return None; }
            let values: Vec<u64> = values.map(str::parse).collect::<Result<_, _>>().ok()?;
            if values.len() < 4 { return None; }
            let idle = values[3] + values.get(4).copied().unwrap_or(0);
            let total: u64 = values.iter().sum();
            Some((total, idle))
        });
        let Some((total, idle)) = parsed else { return (" --%".to_string(), 0) };
        let usage = (*previous).map(|(old_total, old_idle)| {
            let total_delta = total.saturating_sub(old_total);
            let idle_delta = idle.saturating_sub(old_idle);
            if total_delta == 0 { 0 } else {
                (((total_delta.saturating_sub(idle_delta)) * 100) / total_delta).min(100) as u8
            }
        }).unwrap_or(0);
        *previous = Some((total, idle));
        (format!("{usage:>3}%"), usage)
    }

    fn battery_status() -> String {
        let Ok(entries) = fs::read_dir("/sys/class/power_supply") else { return String::new() };
        for entry in entries.flatten() {
            let path = entry.path();
            if fs::read_to_string(path.join("type")).unwrap_or_default().trim() != "Battery" { continue; }
            let capacity = fs::read_to_string(path.join("capacity")).ok()
                .and_then(|value| value.trim().parse::<u8>().ok());
            let status = fs::read_to_string(path.join("status")).unwrap_or_default();
            if let Some(capacity) = capacity {
                let marker = match status.trim() {
                    "Charging" => "+", "Discharging" => "-", "Full" | "Not charging" => "=", _ => "",
                };
                return format!("BAT {capacity}%{marker}");
            }
        }
        String::new()
    }

        pub(super) fn redraw(&self, conn: &RustConnection) -> Result<(), Box<dyn Error>> {
        conn.change_window_attributes(
            self.window,
            &ChangeWindowAttributesAux::new().background_pixel(BG),
        )?;
        conn.clear_area(false, self.window, 0, 0, self.width, BAR_HEIGHT)?;

        // DWM-style workspace labels at the left; the selected workspace is highlighted.
        for workspace in 0..9u8 {
            let x = 8 + (workspace as i16 * 26);
            let selected = workspace == self.current_workspace;
            conn.change_gc(
                self.gc,
                &ChangeGCAux::new().foreground(if selected { ACCENT } else { BG }),
            )?;
            conn.poly_fill_rectangle(
                self.window,
                self.gc,
                &[x11rb::protocol::xproto::Rectangle {
                    x,
                    y: 4,
                    width: 22,
                    height: 20,
                }],
            )?;
            // Keep every workspace number bright and readable, including the selected one.
            conn.change_gc(self.gc, &ChangeGCAux::new().foreground(FG))?;
            let number = (workspace + 1).to_string();
            conn.image_text8(self.window, self.gc, x + 7, 18, number.as_bytes())?;
        }

        let label = if self.active {
            format!("dream search: {}_", self.query)
        } else {
            "dreamWM  |  Alt+P: search".to_string()
        };
        conn.change_gc(self.gc, &ChangeGCAux::new().foreground(ACCENT))?;
        conn.image_text8(self.window, self.gc, 250, 18, label.as_bytes())?;

        let status_x = (self.width as i32 - (self.status.len() as i32 * 8) - 10).max(250) as i16;
        conn.change_gc(self.gc, &ChangeGCAux::new().foreground(FG))?;
        conn.image_text8(self.window, self.gc, status_x, 18, self.status.as_bytes())?;

        if self.active {
            conn.change_gc(self.gc, &ChangeGCAux::new().foreground(FG))?;
            let mut x = 250i32 + (label.len() as i32 * 8) + 12;
            for (index, program) in self.matches.iter().enumerate() {
                let item = if index == self.selected {
                    format!("> {program}")
                } else {
                    format!("  {program}")
                };
                let item_width = item.len() as i32 * 8;
                let status_start = (self.width as i32 - (self.status.len() as i32 * 8) - 20).max(250);
                if x + item_width > status_start {
                    break;
                }
                conn.image_text8(self.window, self.gc, x as i16, 18, item.as_bytes())?;
                x += item_width + 8;
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
        self.redraw(conn)?;
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

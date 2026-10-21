//! Native X11 compositor. Frames are built off-screen and queued with Present.
use std::{collections::{HashMap, HashSet}, error::Error};

use x11rb::{
    connection::Connection,
    protocol::{
        composite::{self, ConnectionExt as CompositeExt, Redirect},
        damage::{self, ConnectionExt as DamageExt, ReportLevel},
        present::{self, ConnectionExt as PresentExt},
        render::{self, ConnectionExt as RenderExt, CreatePictureAux, PictOp, Pictformat},
        xproto::{ConnectionExt as XprotoExt, Pixmap, Window},
        Event,
    },
    rust_connection::RustConnection,
    NONE,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct CompositorCapabilities {
    pub composite: bool,
    pub damage: bool,
    pub render: bool,
    pub present: bool,
}
impl CompositorCapabilities {
    pub(super) fn query<C: Connection>(conn: &C) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            composite: conn.extension_information("Composite")?.is_some(),
            damage: conn.extension_information("DAMAGE")?.is_some(),
            render: conn.extension_information("RENDER")?.is_some(),
            present: conn.extension_information("Present")?.is_some(),
        })
    }
    pub(super) fn supports_native_compositing(self) -> bool {
        self.composite && self.damage && self.render && self.present
    }
}

pub(super) struct NativeCompositor {
    root: Window,
    overlay: Window,
    width: u16,
    height: u16,
    depth: u8,
    root_format: Pictformat,
    visual_formats: HashMap<u32, Pictformat>,
    present_event: u32,
    next_serial: u32,
    last_msc: u64,
    pending: HashMap<u32, Pixmap>,
    damage_by_window: HashMap<Window, u32>,
}

impl NativeCompositor {
    pub(super) fn new(
        conn: &RustConnection, screen_num: usize, root: Window,
        width: u16, height: u16, depth: u8,
    ) -> Result<Option<Self>, Box<dyn Error>> {
        let caps = CompositorCapabilities::query(conn)?;
        if !caps.supports_native_compositing() {
            eprintln!("dreamWM: native compositor disabled (Composite/DAMAGE/RENDER/Present required)");
            return Ok(None);
        }
        let formats = render::query_pict_formats(conn)?.reply()?;
        let screen = conn.setup().roots.get(screen_num).ok_or("invalid X11 screen index")?;
        let visual_formats: HashMap<u32, Pictformat> = formats.screens.iter()
            .flat_map(|s| s.depths.iter())
            .flat_map(|d| d.visuals.iter())
            .map(|v| (v.visual, v.format))
            .collect();
        let root_format = *visual_formats.get(&screen.root_visual)
            .ok_or("XRender has no format for the root visual")?;

        // Do not redirect until all resources needed to draw a frame are known.
        composite::redirect_subwindows(conn, root, Redirect::MANUAL)?.check()?;
        let overlay = composite::get_overlay_window(conn, root)?.reply()?.overlay_win;
        let present_event = conn.generate_id()?;
        conn.present_select_input(
            present_event, overlay,
            present::EventMask::COMPLETE_NOTIFY | present::EventMask::IDLE_NOTIFY,
        )?.check()?;

        let mut compositor = Self {
            root, overlay, width, height, depth, root_format, visual_formats, present_event,
            next_serial: 1, last_msc: 0, pending: HashMap::new(),
            damage_by_window: HashMap::new(),
        };
        compositor.repaint(conn)?;
        eprintln!("dreamWM: native compositor active; frames queued through X Present");
        Ok(Some(compositor))
    }

    /// Returns true when the event requires a new composed frame.
    pub(super) fn handle_event(
        &mut self, conn: &RustConnection, event: &Event,
    ) -> Result<bool, Box<dyn Error>> {
        match event {
            Event::DamageNotify(e) => {
                let _ = damage::subtract(conn, e.damage, NONE, NONE);
                Ok(true)
            }
            Event::PresentCompleteNotify(e) if e.event == self.present_event => {
                self.last_msc = self.last_msc.max(e.msc);
                Ok(false)
            }
            Event::PresentIdleNotify(e) if e.event == self.present_event => {
                if let Some(pixmap) = self.pending.remove(&e.serial) {
                    let _ = conn.free_pixmap(pixmap);
                }
                Ok(false)
            }
            Event::MapNotify(_) | Event::UnmapNotify(_) | Event::DestroyNotify(_)
            | Event::ConfigureNotify(_) | Event::ConfigureRequest(_) => Ok(true),
            _ => Ok(false),
        }
    }

    pub(super) fn repaint(&mut self, conn: &RustConnection) -> Result<(), Box<dyn Error>> {
        self.sync_damage_objects(conn)?;
        let pixmap = conn.generate_id()?;
        conn.create_pixmap(self.depth, pixmap, self.root, self.width.max(1), self.height.max(1))?.check()?;
        let dst = conn.generate_id()?;
        conn.render_create_picture(dst, pixmap, self.root_format, &CreatePictureAux::new())?.check()?;
        // Copy the root drawable first so the wallpaper/background survives
        // underneath transparent and uncovered areas of the scene.
        let root_picture = conn.generate_id()?;
        conn.render_create_picture(root_picture, self.root, self.root_format, &CreatePictureAux::new())?.check()?;
        conn.render_composite(
            PictOp::SRC, root_picture, NONE, dst, 0, 0, 0, 0, 0, 0, self.width, self.height,
        )?.check()?;
        conn.render_free_picture(root_picture)?.check()?;

        // query_tree returns bottom-to-top stacking order, so later windows
        // composite over earlier ones. Each named pixmap is temporary; Present
        // retains only the finished frame buffer.
        let children = conn.query_tree(self.root)?.reply()?.children;
        for window in children {
            if window == self.overlay { continue; }
            let attrs = match conn.get_window_attributes(window)?.reply() {
                Ok(a) if a.map_state == x11rb::protocol::xproto::MapState::VIEWABLE => a,
                _ => continue,
            };
            let Ok(g) = conn.get_geometry(window)?.reply() else { continue };
            if g.width == 0 || g.height == 0 { continue; }
            let Some(&fmt) = self.visual_formats.get(&attrs.visual) else { continue };

            let named = conn.generate_id()?;
            let Ok(cookie) = composite::name_window_pixmap(conn, window, named) else { continue };
            if cookie.check().is_err() { continue; }
            let src = conn.generate_id()?;
            let Ok(cookie) = conn.render_create_picture(src, named, fmt, &CreatePictureAux::new()) else {
                let _ = conn.free_pixmap(named);
                continue;
            };
            if cookie.check().is_err() {
                let _ = conn.free_pixmap(named);
                continue;
            }
            conn.render_composite(
                PictOp::OVER, src, NONE, dst, 0, 0, 0, 0,
                g.x, g.y, g.width, g.height,
            )?.check()?;
            conn.render_free_picture(src)?.check()?;
            conn.free_pixmap(named)?.check()?;
        }
        conn.render_free_picture(dst)?.check()?;

        let serial = self.next_serial;
        self.next_serial = self.next_serial.wrapping_add(1).max(1);
        // Queue against the next observed media-stream counter (MSC). Present
        // sends IdleNotify when the buffer is safe to release.
        let result = conn.present_pixmap(
            self.overlay, pixmap, serial, NONE, NONE, 0, 0, NONE, NONE, NONE,
            present::Option::NONE, self.last_msc.saturating_add(1), 0, 0, &[],
        )?.check();
        if let Err(error) = result {
            let _ = conn.free_pixmap(pixmap);
            return Err(Box::new(error));
        }
        self.pending.insert(serial, pixmap);
        conn.flush()?;
        Ok(())
    }

    fn sync_damage_objects(&mut self, conn: &RustConnection) -> Result<(), Box<dyn Error>> {
        let mut live = HashSet::new();
        for window in conn.query_tree(self.root)?.reply()?.children {
            if window == self.overlay { continue; }
            let Ok(attrs) = conn.get_window_attributes(window)?.reply() else { continue };
            if attrs.map_state != x11rb::protocol::xproto::MapState::VIEWABLE { continue; }
            live.insert(window);
            if !self.damage_by_window.contains_key(&window) {
                let id = conn.generate_id()?;
                if let Ok(cookie) = damage::create(conn, id, window, ReportLevel::NON_EMPTY) {
                    if cookie.check().is_ok() {
                        self.damage_by_window.insert(window, id);
                    }
                }
            }
        }
        let stale: Vec<_> = self.damage_by_window.keys().filter(|w| !live.contains(w)).copied().collect();
        for window in stale {
            if let Some(id) = self.damage_by_window.remove(&window) {
                let _ = damage::destroy(conn, id);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::CompositorCapabilities;
    #[test]
    fn native_compositing_requires_all_four_extensions() {
        let all = CompositorCapabilities { composite: true, damage: true, render: true, present: true };
        assert!(all.supports_native_compositing());
        assert!(!CompositorCapabilities { present: false, ..all }.supports_native_compositing());
        assert!(!CompositorCapabilities { render: false, ..all }.supports_native_compositing());
        assert!(!CompositorCapabilities { damage: false, ..all }.supports_native_compositing());
        assert!(!CompositorCapabilities { composite: false, ..all }.supports_native_compositing());
    }
}

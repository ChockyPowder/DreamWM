//! Window placement algorithms.

use x11rb::{connection::Connection, protocol::xproto::{ConfigureWindowAux, ConnectionExt as _, Window}};
use crate::config::{Layout, BORDER_PX};
use super::DreamWm;

impl DreamWm {
    pub(super) fn arrange(&mut self) -> Result<(), Box<dyn Error>> {
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

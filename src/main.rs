//! dreamWM binary entry point.

mod config;
mod wm;

use std::process;

fn main() {
    match wm::DreamWm::new().and_then(|mut wm| wm.run()) {
        Ok(()) => {}
        Err(error) => {
            eprintln!("dreamWM: {error}");
            process::exit(1);
        }
    }
}

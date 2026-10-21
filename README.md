# dreamWM

A small, keyboard-driven X11 window manager written in Rust, inspired by [dwm](https://dwm.suckless.org/).

> Early development: dreamWM is not yet ready to replace your daily window manager. Test it in a nested X server or virtual machine first.

## Goals

- Preserve dwm's familiar dynamic tiling workflow and default keybindings.
- Use Rust and `x11rb` with a small dependency footprint.
- Support tile, monocle, and floating layouts; nine tags; keyboard-driven focus and window actions.
- Keep the core understandable and deliberately small.

## Build

Install Rust and the X11 development/runtime libraries for your distribution, then run:

```sh
cargo build --release
```

Run only in a test X session for now. Do not replace your session's window manager until dreamWM's event handling and ICCCM/EWMH behavior have been tested thoroughly.

## Default keybindings

The initial bindings follow the conventional `dwm` defaults, using **Alt (Mod1)**:

| Binding | Action |
| --- | --- |
| `Alt + j / k` | Focus next / previous client |
| `Alt + Enter` | Promote focused client to master |
| `Alt + h / l` | Shrink / grow master area |
| `Alt + t` | Tile layout |
| `Alt + f` | Floating layout |
| `Alt + m` | Monocle layout |
| `Alt + Space` | Cycle layouts |
| `Alt + Shift + Space` | Toggle floating for focused client |
| `Alt + 1–9` | View tag 1–9 |
| `Alt + Shift + 1–9` | Send focused client to tag 1–9 |
| `Alt + p` | Open dream search in the persistent top bar |
| `Alt + Shift + Enter` | Launch terminal |
| `Alt + Shift + q` | Quit dreamWM |

The terminal command defaults to `st`; set `DREAMWM_TERMINAL` to another executable if desired (for example, `DREAMWM_TERMINAL=xterm`). The built-in dream search scans executable programs in `PATH`; type to filter, use Up/Down or Tab to select, Enter to launch, and Esc to close search. The top bar stays visible when search is closed. Keyboard keycodes currently assume the conventional US XKB layout. dreamWM publishes `_NET_SUPPORTING_WM_CHECK` and `_NET_WM_NAME` so X11 utilities can identify the window manager.

## Screen tearing and compositing

dreamWM now has an initial in-process X11 compositor using Composite for redirection, DAMAGE for redraw notifications, Render for scene composition, and Present to queue off-screen frames against the display media-stream counter (MSC). It does not require Picom. If the X server lacks one of these extensions, dreamWM falls back to its non-composited mode.

This is an early implementation, not yet a guarantee of tear-free output on every machine. It needs runtime testing on a real Xorg session, including checking the Present completion mode and behavior with the active graphics driver. Tear-free presentation depends on the X server, driver, and hardware honoring synchronized presentation. Test in a nested X server or VM before using it as your daily window manager.

## Status

Early prototype. The current code has basic X11 window management, layouts, tag switching, and core keybindings. ICCCM/EWMH support, robust focus handling, configurable keysyms, and nested-X integration tests are still outstanding.

## Project structure

- `src/main.rs` — binary entry point and startup error reporting.
- `src/config.rs` — layout type, default keycodes, and small configuration constants.
- `src/wm/mod.rs` — X11 connection, window-manager lifecycle, client state, and visibility/focus handling.
- `src/wm/bar.rs` — persistent top bar, executable discovery, and the Alt+P dream search interface.
- `src/wm/keys.rs` — key grabs, keyboard actions, and launching terminal/menu processes.
- `src/wm/layout.rs` — tile, monocle, and floating layout placement.
- `src/wm/identity.rs` — EWMH properties used to identify dreamWM to X11 tools.

# dreamWM

A small, keyboard-driven X11 tiling window manager written in Rust, inspired by [dwm](https://dwm.suckless.org/).

> Early development: dreamWM is not yet ready to replace your daily window manager. Test it in a nested X server or virtual machine first.

## Goals

- Preserve dwm's familiar dynamic tiling workflow and keybindings.
- Use Rust and `x11rb` with a small dependency footprint.
- Support tile, monocle, and floating layouts; nine tags; keyboard-driven focus and window actions.
- Keep the core understandable and deliberately small.

## Build

Install Rust and the X11 development/runtime libraries for your distribution, then run:

```sh
cargo build --release
```

Run only in a test X session for now. Do not replace your session's window manager until dreamWM's event handling and ICCCM/EWMH behavior have been tested thoroughly.

## Status

Initial project setup. Window-management features are being implemented incrementally.

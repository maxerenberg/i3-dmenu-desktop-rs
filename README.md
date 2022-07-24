# i3-dmenu-desktop-rs
This is a rewrite of the [i3-dmenu-desktop](https://github.com/i3/i3/blob/stable/i3-dmenu-desktop)
program in Rust.

In general, it tries to behave the exact same way as the original, although there might be some
cases where this does not happen. In particular, escaping/unquoting of special characters in
desktop entry files has not been thoroughly tested.

## Installation
Make sure you have `cargo` installed, then run
```sh
cargo install --path .
```
By default, this will install the program to `~/.cargo/bin/i3-dmenu-desktop-rs`.

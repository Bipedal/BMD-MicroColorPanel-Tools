# micropanel: the helper's source

> **To use the helper you don't need this folder:** download it and follow the [user guide](../docs/GUIDE.md).
> This page is about building and extending it.

A small tray program that lets the DaVinci Resolve Micro Color Panel control other applications.
It reads the panel over USB or Bluetooth and turns keys, knobs, rings and balls into keyboard
shortcuts, mouse movement, MIDI, Mackie Control or OSC, depending on which program is in front.

| | Windows 10/11 | Linux | macOS |
|---|---|---|---|
| Panel, layers, lights, presets, mapping editor | yes | built by CI, never tried with a panel | not tried |
| OSC / OSCQuery | yes | same | not tried |
| Keys, mouse, MIDI and Mackie Control, cheat-sheet window, tray, "which program is in front" | yes | not written yet | not written yet |

## Build

You need [Rust](https://rustup.rs) 1.88 or newer; on Windows the MSVC toolchain, which is the default.

```
cd integrations
cargo build --release
```

The result is the single file `target/release/micropanel(.exe)`. Built in `helper` instead, it has no part for any
particular program. `cargo test` (in either) runs the tests; none of
them needs a panel. One more test compares the decoder with `panel.py` on all recorded reports; it
only runs after `python tools/make_golden.py`.

`cargo test --features portable` on Windows builds and tests the code other systems get.

### Linux

No C libraries are needed. Your user must be allowed to open the panel; for example as
`/etc/udev/rules.d/70-micropanel.rules`, then replug the panel:

```
KERNEL=="hidraw*", ATTRS{idVendor}=="1edb", ATTRS{idProduct}=="da0f", MODE="0660", TAG+="uaccess"
KERNEL=="hidraw*", KERNELS=="*:1EDB:DA0F.*", MODE="0660", TAG+="uaccess"
```

(The second line is meant for Bluetooth. Both are untested.) There is no tray: start `micropanel`, open
`http://127.0.0.1:58790/`, stop it with `micropanel --quit`. What is missing is listed in
[`src/sys/mod.rs`](src/sys/mod.rs); a port replaces the functions in
[`src/sys/portable.rs`](src/sys/portable.rs) one by one.

## Integrations

A program with its own interface can get an integration: its own actions, its own places (modules, tools)
and its own list in the editor. See [`INTEGRATIONS.md`](INTEGRATIONS.md); `src/osc.rs` is the worked example.

## Files and command line

Everything the editor sets is in a text file, `profiles.toml`; the editor's TOML tab edits it directly.
[`PROFILES.md`](PROFILES.md) describes the format.

Config folder: `%APPDATA%\MicroPanel`, on Linux `~/.config/micropanel`; `MICROPANEL_HOME` overrides it. It
holds `profiles.toml`, `presets/`, `cheatsheet.json` and `helper.log`.

```
micropanel                        tray (Linux: no tray)
micropanel --console              also print the log
micropanel --quit                 stop a running helper
micropanel --check FILE           is this profiles file all right?
micropanel --install-autostart | --remove-autostart
micropanel --no-tray --dry --run-for 30    key and mouse output only logged
micropanel --editor-only --port 58791 [--connect]    only the editor, next to a running helper
```

`PANEL_TRANSPORT=usb` or `bt` forces one connection.

The editor only listens on 127.0.0.1 and refuses requests from other web pages. The helper makes no other
network connections than the OSC ones you set up.

## Safety

- Keep Windows' own HID driver for the panel. Never install WinUSB (Zadig): Resolve needs the stock driver.
- Never hold AUTO COLOR and STOP together: that resets the panel and clears its Bluetooth pairing.
- The helper never writes feature report 0x07, whose meaning isn't known.

## Licence

CC0 1.0, like the rest of this repository. The libraries it is built from have their own licences, all of
them permissive: [`THIRD_PARTY.md`](THIRD_PARTY.md).

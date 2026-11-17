# BMD Micro Color Panel Tools

Protocol info and tools for using the Blackmagic Design Micro Color Panel outside Resolve. This project was researched and written with **Claude**, Anthropic's AI model. Claude
wrote the test scripts, the driver and the spec, and analysed the captures. A human ran
every test on a real panel, did the pressing, rolling and looking, and reviewed each result
before it went into the spec.

An open, test-backed description of how the Blackmagic Design **DaVinci Resolve Micro
Color Panel** talks to a computer over **USB and Bluetooth**, plus a small reference driver
in Python. The goal is to let the panel drive software other than DaVinci Resolve
(MIDI/OSC bridges, photo editors, etc.). Such backends are not part of this repository yet.

> Not affiliated with or endorsed by Blackmagic Design. "DaVinci Resolve" and
> "Blackmagic Design" are trademarks of Blackmagic Design Pty Ltd. Everything here was
> found by observing the panel's own USB/Bluetooth traffic.

## What's here

| Path | What it is |
|---|---|
| [`PANEL_PROTOCOL.md`](PANEL_PROTOCOL.md) | **The spec.** Every statement is tagged **[V]** verified (with the log that shows it), **[A]** assumed, **[D]** from documentation, or **[?]** open. |
| [`panel.py`](panel.py) | Reference driver (hidapi): init and keep-alive, decoded buttons / balls / rings / knobs, key LEDs, backlight, battery, firmware/serial info, state re-sync, automatic reconnect across USB and Bluetooth. |
| [`panel_tester.py`](panel_tester.py) | A window that draws the panel and shows every key, knob, ball and ring live, with a backlight slider and clickable key LEDs. `--demo` runs without a panel. |
| [`tests/`](tests) | The standalone test scripts that produced the evidence. Most guided tests end on a panel gesture (the same key pressed 5× within 3 s). |
| [`logs/`](logs) | The raw logs and USB captures cited in the spec (JSONL, text, pcapng). |
| [`tools/make_publish.py`](tools/make_publish.py) | Builds this public copy and redacts unit identifiers. |

## The protocol in one paragraph

The panel is a vendor-defined HID device (VID `0x1EDB`, PID `0xDA0F`; on USB use interface
2). Over Bluetooth it is a BLE HID-over-GATT device with a byte-identical report
descriptor, so the same code works on both. Send feature report `0a 01` to enable it and
again within 60 s to keep it streaming (Resolve does this every 30 s); `03 nn nn` sets the
key backlight brightness (0–100). Buttons arrive as a 64-bit snapshot (report 0x02), the
three balls and rings as nine signed 24-bit deltas (0x05), the twelve knobs as signed
32-bit deltas (0x06), the battery as `[charging, level %]` (0x08). Output report 0x04
switches the eleven coloured status LEDs. Details, scales, signs and pitfalls are in the
spec.

## Quick start

Requirements: Python 3 and [hidapi](https://pypi.org/project/hidapi/).

```
pip install -r requirements.txt
python panel.py                         # print live events (USB if plugged in, else Bluetooth)
PANEL_TRANSPORT=bt python panel.py      # force one transport: usb or bt (POSIX shell syntax)
python panel_tester.py                  # live picture of the panel; backlight and LED controls
```

Close DaVinci Resolve first, or both programs will write to the panel.

### Important

- **Windows:** keep the stock Microsoft HID driver. Do **not** replace it with WinUSB
  (e.g. via Zadig): Resolve needs the stock driver.
- **Never hold AUTO_COLOR + STOP together**: that factory-resets the panel and clears its
  Bluetooth pairing (product manual).
- **Never write feature report 0x07**: its meaning is only partly known.
- Read GET_REPORTs with their **exact** size (table in spec §2), or the panel stalls the
  request.

## Writing your own driver

Everything needed is in the spec; this is the order to read it in. Any language with
HID access works (hidapi, Windows HID API, Linux hidraw, macOS IOHID).

1. **Find the device** (spec §1, §11.2). USB: VID `0x1EDB`, PID `0xDA0F`, HID
   interface 2 (`MI_02` on Windows). Bluetooth: the same VID/PID over BLE HID (hidapi
   bus type 2). The top-level usage page is vendor `0xFF00` on both.
   Over Bluetooth there is a second way in: the Blackmagic vendor GATT service mirrors
   every report and needs only a BLE GATT client, no HID (spec §11.1).
2. **Enable it** (spec §3). Send feature report `0a 01`, then `03 64 64` (backlight at
   100 %). Nothing streams until `0a 01` has been sent.
3. **Keep it alive** (spec §3). Resend `0a 01` every 20–30 s. After 60 s without it (or
   without a read of feature 0x0A) the stream stops and the backlight turns off.
4. **Read in a loop that never sleeps** (spec §7), with a buffer of 65 bytes. Dispatch
   on byte 0, the report ID, and decode only each report's real payload:
   - `0x02` buttons: a full 64-bit snapshot; diff it against the previous one (spec §4
     has the bit-to-key map).
   - `0x05` balls and rings: nine signed 24-bit deltas (spec §5: slots, signs, scale).
   - `0x06` knobs: twelve signed 32-bit deltas, 360 raw per count (spec §6).
   - `0x08` battery: `[charging, level %]`, sent only on change (spec §2).
5. **Drive the lights** (spec §4a, §2). Output report `04` + 6 bytes switches the 11
   coloured status LEDs, one bit each. Feature `03 nn nn` sets the white backlight
   (0–100).
6. **Recover** (spec §4, §11.4). Plugging in USB drops Bluetooth and unplugging brings
   it back within seconds. Reopen whichever transport appears, resend `0a 01`, and
   re-read the held buttons with GET input report `0x02` (exact length 9). After any
   re-enable, resend the backlight (`03`) and LEDs (`04`): the panel forgets both at a
   timeout (spec §3).
7. **Mind the pitfalls**: exact sizes for every GET_REPORT (spec §2 table); never write
   feature 0x07; check the device list at most once a second (spec §11.4).

For a working host to imitate, see spec §12: exactly what DaVinci Resolve sends at
startup, every 30 s, on key presses and at quit.

`panel.py` implements all seven steps: when the cable is plugged in or pulled out it
follows the panel to the other transport, restores the lights and re-reads the held buttons.

## Status

Tested on one panel (firmware 2.2.0) with Windows 11, Python 3.14 and hidapi 0.15, over USB
and Bluetooth, between 2026-11-15 and 2026-11-17. Linux and macOS notes in the spec are
untested. Open questions are listed in spec §9.

## About the logs

- The panel's serial number and Bluetooth address have been replaced by `X…` placeholders
  (or zero bytes in binary captures) everywhere, including the pcapng files. Placeholders
  have the same length, so the captures still open in Wireshark.
- Raw Bluetooth captures are not published; they contain pairing keys. The decoded GATT
  table (`logs/btgatt_pairing_*.txt`) is included.
- The USB captures contain only the panel's own traffic, recorded with USBPcap. Their
  file headers still name the capture machine's CPU and Windows build.

## Licence

**CC0 1.0 Universal (public domain dedication).** Code, spec and logs alike: use it for
anything, no attribution required. See [`LICENSE`](LICENSE).

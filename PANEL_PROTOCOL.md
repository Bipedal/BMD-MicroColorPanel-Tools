# DaVinci Resolve Micro Color Panel: USB and Bluetooth HID protocol

Accepted 2026-11-16; Bluetooth (§11), transport hand-over and keep-alive-by-read added 2026-11-16. Built only from tests written and run in this project (`tests/`, `logs/`). Earlier files from another tool were not used as evidence.

**Status tags**
- **[V]** verified by a test here; the log is cited.
- **[A]** assumed: accepted without an exact measurement.
- **[D]** from documentation (the product manual or OS docs), not tested here.
- **[?]** open or unverified.

Reference implementation: `panel.py`. Control maps with their evidence: `tests/panel_map.py`.

---

## 1. Device and interface

| Item | Value | Status |
|---|---|---|
| VID / PID | `0x1EDB` / `0xDA0F` | [V] dump_caps |
| Strings | "Blackmagic Design" / "DaVinci Resolve Micro Color Panel", per-unit serial | [V] dump_caps |
| HID interface | interface 2 (`MI_02` in the Windows path); the only HID collection the panel exposes | [V] dump_caps |
| Top-level usage | page `0xFF00` (vendor), usage `0x0000` | [V] dump_caps |
| Windows driver | stock Microsoft HID (hidclass). Do not replace it with WinUSB; Resolve needs the stock driver. | project rule |

Report lengths from `HidP_GetCaps` **include the report-ID byte** [V] `logs/dump_caps_20261115_200228.txt`:

| Type | Byte length |
|---|---|
| Input | **65** (largest is 0x06: ID + 64) |
| Output | 9 (0x02: ID + 8) |
| Feature | **33**: that's feature 0x09 (32 B + ID) [V]. hidapi's rebuilt descriptor leaves 0x09 out, which made 33 look unexplained (§2). |

## 2. Report descriptor summary [V] dump_caps

**The real descriptor is 297 bytes** [V]. It was captured from the wire during a replug: `logs/usb_resolve_20261116_182613.pcapng`, decoded in `logs/usbdecode_usb_resolve_20261116_182613.txt`. hidapi on Windows rebuilds a 251-byte version from parsed data, and that version **leaves out the Const-only feature reports 0x01 and 0x09** [V].

Over USB each report is sent at its real size: 0x02 = 9 bytes, 0x05 = 37, 0x08 = 3 [V] capture. On Windows, reads return 65 bytes.

Reading a report on request (GET_REPORT) **must ask for the exact size (ID + payload)**. Any other size is stalled (USB status `0xC0000004`, Win32 error 31) [V] `tests/usb_decode.py` on our own capture, plus exact-length retests.

| ID | Direction | Payload | Use | Readable with GET? |
|---|---|---|---|---|
| 0x01 | Feature (Const) | 7 B | **Firmware version.** The descriptor tags it with standard version fields (page 0x06: Software Version ×4 bytes, Major, Minor, Revision). Read: `15 5b 3b fd` / **2.2.0** [V read]. [?] What the 4-byte build ID means. | yes, len 8 [V] |
| 0x02 | Input | 64 × 1 bit (8 B) | Button snapshot (§4) [V] | **yes, len 9** [V] |
| 0x02 | Output | 64 × 1 bit (8 B) | Resolve sends only all-zero, once at start and once at quit [V capture]. All-ones, tried twice, had **no visible effect** [V]. So it's not per-key backlight control as far as can be seen. [?] Purpose. | – |
| 0x03 | Feature | 2 × 8 bit | **Byte 2 (the report's third byte) = white key-backlight brightness, 0–100**: `03 64 00` → backlight dark [V by eye]. **Byte 1: no visible effect**: `03 00 64` looked like full brightness [V]; [?] purpose. Resolve always sends byte 1 = byte 2, with 100 (0x64) as its maximum [V capture]. The coloured status LEDs **ignore 0x03**: still on at `03 00 00` [V]. | USB: no, stalls [V]. **BT: yes**, len 3, read `03 00 00` [V] get_reports_bt |
| 0x04 | Output | 48 × 1 bit (6 B) | **Key status LEDs**, sent over interrupt OUT [V capture]. **One bit per LED, one fixed colour per key** (§4a) [V walk `logs/led_walk_20261116_184710.jsonl`]. | – |
| 0x05 | Input | 3 × 96 bit (36 B) | Balls and rings (§5) [V] | yes, len 37 [V] |
| 0x06 | Input | 16 × 32 bit (64 B) | Knobs (§6) [V] | yes, len 65 [V] |
| 0x07 | Feature | 1 + 4 B | **Byte 1 = connection state:** `00` on BT, `03` on USB once paired, `02` on USB before pairing [V] transport_switch_now/none. **Bytes 2–3, little-endian, track the battery voltage** [A]: on battery they fall about 5.5/min with the level (per-level means 96 % 16514 … 93 % 16408), jump about +330 when charging starts and about −200 when it stops; noise about ±50 between reads [V] `logs/battery_log_20261116_223322.jsonl`. [?] Scale: ÷4 = mV (4.10–4.21 V) is plausible but unverified. Bytes 4–5 always 00 (216 samples) [V]. Never write it blindly. | yes, len 6 [V] |
| 0x08 | Input + Feature | 2 B | **Battery: [charging 0/1, level %].** The input report arrives only when a value changes: `01 63` at plug-in, then `00 63`, then `00 64` [V capture]. Same layout as the Speed Editor's battery report [D]. Just after a switch from USB to BT it read 100 % for about 30 s before a report corrected it to 96 %. The level can wobble ±1 % (94 → 95 → 94). On replug the same report came 4 times in under 1 s [V] battery_log. Idle drain on BT, enabled: about 0.25 %/min [V, one run]. | yes, len 3 (both) [V] |
| 0x09 | Feature (Const) | 32 B | **Serial number**: 32 ASCII hex digits, the same as the USB serial string (redacted in published logs) [V] | yes, len 33 [V] |
| 0x0A | Feature | 1 B | Init / keep-alive `0a 01` (§3). Reading it gives the **enable flag** (`01` streaming, `00` timed out) **and resets the keep-alive timer** (§3) [V]. | yes, len 2 [V] |

Report IDs received as interrupt input: 0x02, 0x05, 0x06 and 0x08 (battery, rare) [V].

## 3. Initialisation and keep-alive

- **Init sequence:** send feature `0a 01`, then feature `03 64 64`. Both are accepted every time; **0 send failures** across every test run [V]. hidapi pads short feature reports to the 33-byte feature length on Windows [V, sends succeed].
- **`0a 01` is required, and is the keep-alive.** After a single init the panel stopped reporting **58.7 s** later (`logs/keepalive_20261116_180432.jsonl`) and **60.0 s** later (`logs/keepalive_get_none_20261116_222533.jsonl`), with a ball still moving [V]. That makes it a fixed timer from the last `0a 01`, not an idle timeout.
- **`03 64 64` is not a keep-alive.** On its own it did not revive a dead stream; `0a 01` revived it immediately (162 reports in 6 s) [V] same log. Sending it once at init is enough [V] smoke_live.
- **Recommended:** resend `0a 01` every 20 s (3× margin). This kept the stream alive for 94 s in the smoke test [V] `logs/smoke_live_20261116_180732.jsonl`.
- **The panel is silent when nothing moves.** Gaps of 4–6 s during pauses are normal and are **not** a failure signal [V] keepalive log. Do not re-init on silence.
- **Init does not cause a state report.** Sending `0a 01` or `03 64 64` produced no 0x02 report, both with nothing held and with a button held [V] `logs/state_probe_20261116_175652.jsonl`, `logs/state_probe_20261116_175916.jsonl`.
- **Resolve does the same:** `0a 01` every **30.00 s** [V capture]. Its full behaviour is in §12.
- **The enable state can be read back:** feature 0x0A reads `01` while enabled and `00` after the timeout [V].
- **Several programs can read at once:** our script read reports while Resolve had the panel open (77 reports, nothing sent) [V] `logs/concurrency_20261116_183041.jsonl`. Both programs receive every input report. Writes from two programs would conflict (brightness, LEDs).
- **Reading feature 0x0A also resets the timer.** One `0a 01`, then only a GET of 0x0A every 5 s: the stream ran the full 100 s with no gap over 3 s. The control run without GETs died at 60.0 s [V] `logs/keepalive_get_get_20261116_222320.jsonl`, `logs/keepalive_get_none_20261116_222533.jsonl`. So a status poll (e.g. `panel.enabled()`) keeps the panel alive as a side effect. Writing `0a 01` stays the recommended keep-alive; it's what Resolve does.
- **At timeout** the enable flag drops to `00` [V] keepalive_get_none, and **the white backlight and the coloured status LEDs all turn off** [V by eye] keepalive_get_none, `logs/quick_checks_led_20261117_221857.jsonl`.
- **Re-enabling does not restore the lights.** After the timeout, `0a 01` alone brought the white backlight back **dim** (not the 100 % set earlier), and the coloured LEDs stayed off. `03 64 64` then brightened the backlight [V by eye, same log]. The LEDs did not come back by themselves, and the closing all-off `04` caused no visible change. So after any re-enable, resend `03` and `04` [V].
  [A] The dim level is the panel's default brightness after enable; its value is unmeasured.
- **The enable state survives a transport switch** (BT → USB → BT): USB streamed at once with no USB-side init, and 0x0A read `01` on both sides [V] `logs/transport_switch_none_20261116_221846.jsonl`. After a timeout, only `0a 01` (or a 0x0A read, above) restores the stream [V].
- **[?] Fresh power-up:** whether a panel that has just been switched on streams before any init is untested.

## 4. Buttons: input report 0x02

- **Layout:** bytes 1–8 hold a 64-bit mask, LSB first. **Bit number = byte_index × 8 + bit**, with byte_index 1–8, so bits run 8–71 [V].
- **Used bits:** exactly 52, **bits 8–59**. Bits 60–71 never set [V] `logs/raw_check_20261115_200959.jsonl`.
- **Each report is a full snapshot** of all buttons, not an event. Apply it as the new state, then compare it with the previous one to get presses and releases [V].
- **Bytes past byte 8 are garbage.** In 111 of 119 reports, bytes 9+ exactly matched the tail of the previous 0x05 report [V] raw_check. The panel sends only 9 bytes over USB [V capture], so the extra bytes are added on the host side (driver or hidapi buffer reuse). Ignore everything past each report's real payload.
- **Button state can be read on request:** `get_input_report(0x02, 9)` (exact length) returns the current snapshot, **held buttons included**: with H_LITE and USER held, it returned {H_LITE, USER}, the same as the stream [V] `logs/smoke_features_20261116_185454.jsonl`. So a driver can rebuild its button state after a (re)init; `panel.py` does. The earlier "rejected" result came from asking for 65 bytes. Neither init report causes a 0x02 to be sent [V].

### Button map [V] `logs/scan_buttons_20261115_201513.jsonl` + recheck `logs/scan_buttons_20261115_203109.jsonl`; labels from the manual p.10 drawing [D]

| Bits | Group | Buttons |
|---|---|---|
| 8–19 | Knob presses, left → right | Y_LIFT, Y_GAMMA, Y_GAIN, CONTRAST, PIVOT, MID_DETAIL, COLOR_BOOST, SHADOWS, HIGHLIGHTS, SATURATION, HUE, LUM_MIX |
| 20–33 | Left 2 × 7 block; even bit = left column, odd = right, top → bottom | 20 AUTO_COLOR / 21 OFFSET, 22 COPY / 23 PASTE, 24 UNDO / 25 REDO, 26 DELETE / 27 RESET, 28 BYPASS / 29 DISABLE, 30 USER / 31 LOOP, 32 SHIFT_UP / 33 SHIFT_DOWN |
| 34–36 | Above left ball | PLAY_STILL, WIPE_STILL, GRAB_STILL |
| 37–40 | Top-middle row | H_LITE, VIEWER, CURSOR, SELECT |
| 41–43 | Above middle ball | RESET_LIFT, RESET_GAMMA, RESET_GAIN |
| 44–46 | Above right ball | ADD_NODE, ADD_WINDOW, ADD_KEYFRM |
| 47–56 | Right 2 × 5 block; odd bit = PREV column, even = NEXT, top → bottom | STILL, KEYFRM, NODE, FRAME, CLIP (e.g. 47 PREV_STILL / 48 NEXT_STILL … 55 PREV_CLIP / 56 NEXT_CLIP) |
| 57–59 | Transport | 57 PLAY_REV (◀), 58 PLAY_FWD (▶), 59 STOP (■, the wide key) |

## 4a. Key LEDs: output report 0x04

Report `04` + 6 bytes, sent as an output report (on the wire: interrupt OUT, 7 bytes, the same as Resolve) [V]. Bit n = byte `1 + n/8`, bit `n%8`.

| Bit | Key | Colour | Bit | Key | Colour |
|---|---|---|---|---|---|
| 0 | BYPASS | red | 6 | PLAY_STILL | green |
| 1 | USER | green | 7 | WIPE_STILL | green |
| 2 | OFFSET | green | 8 | H_LITE | green |
| 3 | DISABLE | red | 9 | VIEWER | green: faint next to the white backlight, clearly green with the backlight off [V by eye] |
| 4 | SHIFT_UP | green | 10 | CURSOR | green |
| 5 | SHIFT_DOWN | green | 11–47 | nothing visible | |

[V] Walk, plus colours from the all-on view.
- **One colour per key:** no key answered to more than one bit, and with every bit on nothing mixed (no amber or yellow). Other keys, transport included, are white-only.
- **Brightness doesn't apply:** these LEDs ignore 0x03 and stay at full when the backlight is 0.
- **The host drives every LED,** including the Shift keys; the panel lights nothing by itself.

⚠ **Never hold AUTO_COLOR + STOP together.** That combination factory-resets the panel and clears Bluetooth pairing [D] manual p.8.

## 5. Balls and rings: input report 0x05

- **Payload:** 36 bytes, read as 9 slots of 4 bytes at offset `1+4k`, k = 0–8 [V].
- **Byte `1+4k` was 0 in every report** (540+ reports across several runs) [V]. The value is therefore decoded as **signed 24-bit little-endian at `2+4k`**. A signed 32-bit read at `1+4k` gives exactly 256× the same number, so the data cannot tell the two layouts apart; s24 is a choice [V for equivalence].
- **Values are relative deltas**, always multiples of 16 (gcd 16 in all 9 slots) [V] raw_check. One step = 16 raw.
- **Timing:** reports arrive every ~4–8 ms while something moves; ring values are 16–64 raw per report [V] `logs/axis_rings_20261116_174515.jsonl`.

| Slot | Control | Sign | Status |
|---|---|---|---|
| 0 / 1 | LEFT ball X / Y | raw +X = top of ball rolled **left**; raw +Y = rolled **toward you** | [V] `logs/axis_balls_20261116_174154.jsonl` |
| 2 | LEFT ring | CW = + | [V] axis_rings |
| 3 / 4 | MIDDLE ball X / Y | same as left | [V] |
| 5 | MIDDLE ring | CW = + | [V] |
| 6 / 7 | RIGHT ball X / Y | same as left | [V] |
| 8 | RIGHT ring | CW = + | [V] |

- **Ball signs are the same on all three balls, and both axes are inverted from screen convention.** Negate both to get right = +X, up = +Y; `panel.py` does this [V].
- **Ring scale:** **4800 raw per revolution** (300 steps of 16) on all three rings. The middle ring measured 4816 / 4640 / 4816 / 4784 over four full turns, slow and fast, and **turning speed makes no difference** (fast/slow = 0.98) [V] `logs/axis_ringturns_20261116_174831.jsonl`. Left and right rings: exactly 9600 raw over 2 turns each [V] `logs/quick_checks_20261117_221355.jsonl`.
- **Cross-axis leak on the balls:** 0.2–7.9 % net and 5–20 % absolute of the main axis, with off-axis per-report peaks of 32–208 raw [V] axis_balls. These figures include hand drift; separating the panel's own leakage would need a jig [?].
  - **No fixed deadzone works**, because the off-axis peaks overlap slow intentional rolls (median 112 raw per report).
  - **Suggested instead:** an axis lock that zeroes the minor axis while it stays under 25 % of the major one (smoothed). This is optional in `panel.py`.
- **Idle jitter:** a hands-off log of 87 s had **no ball, ring or knob deltas at all** [V] `logs/quick_checks_20261117_221355.jsonl` (idle part, stopped early). Two earlier ±16 ring ticks with nobody touching the panel were probably a touch or a vibration [A]; even so, a host should not treat a single ±16 as intent.

## 6. Knobs: input report 0x06

- **Layout:** 16 × signed 32-bit little-endian at `1+4k`. **Slots 0–11 = knobs left → right**, Y_LIFT … LUM_MIX, in the same order as their press bits [V] `logs/axis_knobs_20261116_175340.jsonl`. Slots 12–15 always 0 [V].
- **Values are relative deltas, always multiples of 360** (gcd 360 in all 12 slots) [V] raw_check. 1 count = 360 raw. **CW = +** on all 12 knobs [V].
- **Resolution:** hand-measured turns of Y_LIFT gave 4081 (one turn), 4089 (3 turns, moderate speed), 4100 and 4170 (fast) and 4094 and 4128 (slow) counts per turn [V] axis_knobs, `logs/quick_checks_20261117_221355.jsonl`. **[A] 12-bit encoder, 4096 counts per revolution** (1,474,560 raw). Knobs have no detents [tester].
- **Turning speed makes no difference:** fast turns never come out low; the ±1 % spread is hand placement [V] same log (mean rate ~2100 counts/s fast vs ~500–650 slow).
- **Report size:** knob reports are large and infrequent; up to ~35,000 raw in a single report was seen [V].
- **Readable on request:** `get_input_report(0x06, 65)` returns all zeros when idle [V].

## 7. Reading the panel without losing data

- **Report rate:** while controls move, reports arrive at up to ~80/s or more [V] state_probe C1.
- **Overflow loses movement.** If the reader falls behind, the OS input buffer overflows and reports are dropped. A deliberately slow reader (one read per 30 ms) dropped reports: button transitions that changed more than one bit at once rose from 12 % to 45 % [V] `logs/state_probe_20261116_175916.jsonl`. Dropped 0x05 and 0x06 reports mean lost movement.
- **Overflow did not leave buttons stuck:** 123 presses, 123 releases, nothing held at the end [V]. Because 0x02 is a snapshot, the next report corrects the state. [D/?] This fits Windows hidclass discarding the *oldest* buffered report; the discard order itself was not observed directly.
- **Recommendation:** a dedicated reader loop that never sleeps, `read(65)` with a short timeout.
- **[?] Stuck button (old "bit 22 latch"):** not reproduced, with fast or overflowing readers. If it happens, the next 0x02 report clears it, or the state can be re-read at any time with `get_input_report(0x02, 9)` [V].

## 8. Platform notes

| Platform | Note | Status |
|---|---|---|
| Windows | Every input read returns 65 bytes (ID + padding), including the 37-byte 0x05 report. Feature sends pad to 33 bytes. | [V] |
| C / raw API | Use **65-byte** buffers for reading the input stream (ID + 64); a 64-byte buffer truncates 0x06. For GET_REPORT, use the **exact** report size (table in §2), or the panel stalls. | [V] |
| Linux hidraw | For numbered reports, `read()` returns the report ID in byte 0. The older claim that it doesn't is believed wrong. | [D] kernel hidraw docs, untested |
| macOS | hidapi/IOHID behaviour | [?] untested |
| Bluetooth | BLE HID over GATT, same reports as USB. See §11. | [V] |

## 9. Open questions

1. (closed: VIEWER is green, just faint next to the backlight)
2. **Output 0x02:** what it does. Resolve only sends zeros; all-ones has no visible effect.
3. **`03` byte 1:** no visible effect; what is it for?
4. **Feature 0x07** bytes 2–3: scale of the battery-voltage value (§2). Never write it blindly.
5. (closed: `get_input_report(0x02, 9)` shows held buttons)
6. (closed: no speed dependence on knobs or rings; all rings 4800 raw/rev, §5/§6)
7. (closed: Bluetooth, §11)
8. **Linux/macOS port notes** (§8) need a real test.
9. **Blackmagic vendor GATT service:** what it's for, and whether writes to it act like HID reports.
10. **BT timing:** 15 ms requested interval but notifications ~30 ms apart.
11. **Blue light on the back:** the manual says it flashes while pairing (after pressing the rear Bluetooth button) [D p.6]; the ~5 s light at unplug is assumed to be BT reconnecting (§11.4).
12. **Resolve over Bluetooth:** does it behave as on USB (§12: keep-alive, LEDs)? Not captured.
13. **Fresh power-up:** does the panel stream before any init (§3)?
14. (closed: everything turns off at the timeout and nothing comes back by itself after re-enable, §3)
15. **Resolve LED behaviour** for USER, DISABLE, PLAY_STILL, WIPE_STILL, H_LITE, VIEWER and CURSOR (§12.3).
16. **What triggers Resolve's startup sequence** (§12.1): Resolve starting, or detecting the device.
17. **Backlight level after a bare `0a 01`** (§3): the default value, and whether a fresh enable also starts dim.

Closed 2026-11-16 by the Resolve capture: the 33-byte feature length (= 0x09), what 0x03 and 0x08 are, running alongside Resolve, and that 0x04 drives the key LEDs. Closed by the LED probe: the full 0x04 bit map, one colour per key, and 0x03 byte 2 = backlight.
Closed 2026-11-16 by the Bluetooth work: the BT descriptor (identical), GATT layout, report framing (identical), USB↔BT hand-over, and keep-alive by reading 0x0A.

## 10. Test index

| Script | Purpose | Key logs |
|---|---|---|
| `tests/dump_caps.py` | enumeration, HidP caps, descriptor | `dump_caps_20261115_200228.txt` |
| `tests/raw_check.py` | divisibility, gap bytes, stale tail, report IDs | `raw_check_20261115_200959.jsonl` |
| `tests/scan_buttons.py` | guided button → bit map | `scan_buttons_20261115_201513.jsonl`, `…_203109.jsonl` |
| `tests/axis_test.py` | ball, ring and knob slots, signs, leak, scale | `axis_balls_20261116_174154`, `axis_rings_20261116_174515`, `axis_ringturns_20261116_174831`, `axis_knobs_20261116_175340` |
| `tests/state_probe.py` | GET_REPORT, init → state, overflow / stuck | `state_probe_20261116_175652`, `state_probe_20261116_175916` |
| `tests/keepalive_test.py` | keep-alive necessity and timing | `keepalive_20261116_180432` |
| `tests/smoke_panel.py` | `panel.py` offline replay, `live` run, and `features` (info/battery/state()/LEDs) | `smoke_live_20261116_180732`; `smoke_features_20261116_185454`; `smoke_live_20261116_185621` (one failure: a button held at startup gave an unmatched release. Fixed in `panel.py` and checked offline only; the live rerun was skipped.) |
| `tests/usb_capture.py` + `tests/usb_decode.py` | USBPcap capture of the panel only, and its decoder | `usb_resolve_20261116_182613.pcapng`, `usbdecode_usb_resolve_20261116_182613.txt` |
| `tests/concurrency_check.py` | read-only read alongside Resolve | `concurrency_20261116_183041` |
| `tests/led_probe.py look/look2/walk` | LED patterns, brightness bytes, 0x04 bit walk | `led_look2_20261116_184426`, `led_walk_20261116_184710`, `usb_ledlook_20261116_183826.pcapng` (look: P3/P4 invalid, brightness was being resent) |
| `tests/dump_caps.py` (PANEL_TRANSPORT=bt) | BT enumeration and caps | `dump_caps_bt_20261116_190825.txt` |
| `tests/get_reports.py` | exact-size GET of every report, either transport | `get_reports_bt_20261116_190838` |
| `tests/bt_gatt_decode.py` | BT HCI capture → L2CAP → ATT → GATT table | `btgatt_pairing_20261116_190313.txt` (the raw capture holds pairing keys and stays unpublished) |
| `tests/transport_switch.py --usb-init now/none` | USB ↔ BT hand-over (B4b) | `transport_switch_now_20261116_221546`, `transport_switch_none_20261116_221846`, `usb_b4b1_20261116_221543.pcapng`, `usb_b4b2_20261116_221843.pcapng`; superseded: `transport_switch_20261116_191703` |
| `tests/keepalive_get.py --mode get/none` | does a GET of 0x0A keep the panel alive (K2) | `keepalive_get_get_20261116_222320`, `keepalive_get_none_20261116_222533` |
| `tests/battery_log.py` | 0x07 and battery over 36 min, following USB↔BT switches | `battery_log_20261116_223322` |
| `tests/quick_checks.py [--part turns/idle/led]` | knob speed, ring scale L/R, idle jitter, lights at timeout | `quick_checks_20261117_221355` (turns + 87 s idle; stopped during idle), `quick_checks_led_20261117_221857` |

## 11. Bluetooth

Paired from Windows Settings; Windows asked only for an accept, no PIN [V tester]. Shows as "Micro Color Panel" with its battery level [V].

### 11.1 Link and GATT [V] decoded GATT table, `logs/btgatt_pairing_20261116_190313.txt`
- **Bluetooth Low Energy, HID over GATT** (service 0x1812).
- **The Report Map is the same 297 bytes as the USB descriptor, byte for byte.**
- Each report has its own Report characteristic, with a Report Reference giving the same ID and type as on USB (0x01, 0x03, 0x07, 0x08, 0x09, 0x0A feature; 0x02, 0x05, 0x06, 0x08 input; 0x02, 0x04 output).
- **Other services:**
  - Generic Access: name "Micro Color Panel"; appearance 0x03C1 (keyboard) [D BT assigned numbers].
  - Device Information: manufacturer and model as on USB; serial = the unit serial (as 0x09); PnP ID `02 db1e 0fda 0001` (USB vendor-ID source, VID 0x1EDB, PID 0xDA0F, version 0x0100).
  - Battery service (0x180F, level in %).
  - Scan Parameters (0x1813).
- **Blackmagic vendor service** `1DCE0A9A-D84D-4A9F-878C-B48F0FBE12D3`: 11 characteristics that mirror the HID reports one-to-one (same properties and initial values) [V]. [?] Purpose; probably for hosts that can't open HID over GATT, e.g. iPad. Untested.
- **Connection parameters:** the panel requests a 15 ms interval, slave latency 5 and a 2 s supervision timeout. Windows rejected that L2CAP request, then set the same values itself [V]. Notifications arrive in bursts about 30 ms apart [V]. [?] Why 30 ms rather than 15.

### 11.2 Windows / hidapi [V] `logs/dump_caps_bt_20261116_190825.txt`
- One HID collection. The path contains `{00001812-…}`; hidapi gives **bus_type 2** and **interface_number -1**. Match on bus_type (`PANEL_TRANSPORT=bt` in `panel.py`).
- **Strings use underscores:** `Blackmagic_Design`, `DaVinci_Resolve_Micro_Color_Panel`. **The serial string is the Bluetooth device address (12 hex digits), not the unit serial.** Feature 0x09 still returns the unit serial.
- Caps are identical to USB (65 / 9 / 33); hidapi again rebuilds a 251-byte descriptor.

### 11.3 Reports over Bluetooth
- **Same layouts and scales as USB:** knobs ×360, balls and rings ×16, the 0x05 gap byte always 0, the same button bits [V] `logs/raw_check_20261116_190934.jsonl`. `panel.py` needed only the device selection: info, battery, `state()` with held buttons, and 15 writes, 0 failures [V] `logs/smoke_features_20261116_191442.jsonl`.
- **Init and keep-alive are the same:**
  - one init → the stream ended about 60–65 s later;
  - `03 64 64` alone revived nothing;
  - `0a 01` revived it [V] `logs/keepalive_20261116_191118.jsonl`.
- **GET differences** [V] `logs/get_reports_bt_20261116_190838.jsonl`:
  - 0x03 is readable (stalls on USB);
  - input 0x05 and 0x06 return the **last report sent, not zero**, so never use them to read movement;
  - input 0x02 returns the current button snapshot, as on USB [V smoke_features].
- **Battery 0x08** notifications arrive even while the panel is not enabled [V].
- **Report rate** while gently rolling a ball: about 18/s on BT against about 39/s on USB in the same session [V transport_switch_now]. Movement is not lost; it comes in fewer, larger reports [A].

### 11.4 USB ↔ Bluetooth hand-over [V] `logs/transport_switch_now_20261116_221546.jsonl`, `logs/transport_switch_none_20261116_221846.jsonl`
- **Plugging in USB drops BT at once** (within about 0.2 s), and the panel streams over USB.
- **It stays on USB while plugged in:** 64 s in both runs, with and without `0a 01` sent on USB. No fallback to BT.
- **After unplugging, BT reconnects by itself in 2–5 s**; no re-pairing.
- **The enable flag and its timer are shared** across the switch (§3).
- **A driver must follow the switch:** reopen on the other transport and resend `0a 01`, which is harmless.
- An earlier run (B4, `logs/transport_switch_20261116_191703.jsonl`) seemed to fall back to BT with the cable still in. **Not reproduced.** [A] Caused by that script checking the device list every 0.5 s in its read loop. Each check reads the USB string descriptors; at a 0.3 s interval some were stalled [V] `logs/usb_b4b1_20261116_221543.pcapng`, at 1 s none were [V] `logs/usb_b4b2_20261116_221843.pcapng`. **Check the device list at most once a second, off the read thread.**
- **The blue light on the back** comes on for about 5 s around the moment USB is unplugged, i.e. while the panel moves back to BT [V by eye, battery_log; also seen at unplug in B4 and b4b2]. Not seen while plugged in or on BT (b4b1, K2a, K2b). [A] It signals Bluetooth (re)connecting.

## 12. How DaVinci Resolve drives the panel

Resolve is the reference host. Copying its behaviour is the safest way to drive the panel. Source: one USB session on Windows, with Resolve's own panel support. Evidence: `logs/usb_resolve_20261116_182613.pcapng`, decoded in `logs/usbdecode_usb_resolve_20261116_182613.txt`; the times below are seconds into that capture. Resolve over Bluetooth has not been captured (§9).

### 12.1 Startup [V]
The panel enumerated at 18.5 s. That included reading the 297-byte descriptor and a `SET_IDLE 0` from Windows, and the panel sent one battery report (`08 01 63`). Resolve's first write came at 33.1 s, 14.6 s later. [?] What triggered it is not recorded (Resolve starting up, or its device scan). The sequence then took under 10 ms:

| Time | Resolve sends / reads | Meaning |
|---|---|---|
| 33.09 | feature `03 00 00` | backlight off |
| 33.09 | output `04 00 00 00 00 00 00` | all status LEDs off |
| 33.10 | output `02` + 8 × `00` | output 0x02 cleared (purpose unknown, §9) |
| 33.10 | GET feature 0x01 (len 8) → `01 15 5b 3b fd 02 02 00` | firmware version 2.2.0 |
| 33.10 | feature `0a 01` | enable |
| 33.18 | GET feature 0x08 (len 3) → `08 01 63` | battery: charging, 99 % |
| 39.65 | feature `03 04 04` | the user's saved brightness (4 %) |

It never read 0x07 or 0x09, and never wrote 0x07.

### 12.2 Steady state [V]
- **Keep-alive:** `0a 01` every **30.00 s**, each followed at once by the current brightness `03 b b` (9 pairs, 33.1 s → 273.1 s).
- **Battery:** feature 0x08 is read right after each battery input report (158.44 s and 221.80 s, same timestamp as the report) and twice at startup. No polling between events.
- **Brightness setting:** every change in Resolve's settings goes out at once as `03 b b`, the two bytes always equal. Seen: 0, 4, 8, 12, 38, 100 (`00 04 08 0c 26 64`). Resolve's maximum is 100.
- **Device scan:** string-descriptor reads at 268.1 s and 268.2 s. [A] This is a device-list scan by Resolve or Windows.

### 12.3 Status LEDs [V]
Resolve sends output 0x04 (7 bytes on the wire) only when an LED should change, 0–110 ms after the key event that caused it. The panel never lights a key LED by itself.

| Key | Bit | Resolve behaviour seen |
|---|---|---|
| OFFSET | 2 | toggles on each press (on, off, on, off over 4 presses) |
| BYPASS | 0 | toggles on each press |
| SHIFT_UP | 4 | lit only while held; on at press, off at release |
| SHIFT_DOWN | 5 | lit only while held |

[?] USER, DISABLE, PLAY_STILL, WIPE_STILL, H_LITE, VIEWER and CURSOR were not exercised under Resolve. What each key *does* inside Resolve is described in the product manual [D] and is not repeated here.

### 12.4 Quit [V]
At 290.80 s: feature `03 00 00` (backlight off), then output 0x04 all zero and output 0x02 all zero. **No disable report is sent**: the panel stays enabled until its 60 s timer runs out (§3). A host that wants the panel dark at once should copy this; `panel.py`'s `close()` does.

### 12.5 Summary for host authors
1. On connect: backlight off, LEDs off, output 0x02 zero, read version, `0a 01`, read battery, set brightness.
2. Every 30 s (20 s is safer): `0a 01`, then the current brightness.
3. On a battery input report: optionally read feature 0x08 to confirm.
4. Drive the status LEDs yourself from your app's state: toggle-type for modes, momentary for Shift.
5. On exit: backlight 0, LEDs off, output 0x02 zero; let the timer expire.

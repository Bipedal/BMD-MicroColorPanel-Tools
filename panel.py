"""DaVinci Resolve Micro Color Panel - HID driver for USB (interface 2) and Bluetooth (BLE HID).

Written from scratch 2026-11-16 from verified test results (see tests/ and logs/).

    from panel import Panel
    with Panel() as p:          # transport "auto": USB if plugged in, else Bluetooth
        p.run(print)            # blocking; or p.start(cb) for a background thread

    p.set_leds({"OFFSET", "BYPASS"})       # status LEDs (11 keys, fixed colours)
    p.set_backlight(40)                     # white backlight 0..100
    p.info(), p.battery(), p.enabled(), p.state()

Events (dataclass Event):
    button  name, bit, down (bool) [, synthetic=True when produced by a resync]
    knob    name, index, delta (encoder counts, 4096/rev assumed), raw
    ball    name, dx, dy (steps of 16 raw; +x = right, +y = up), raw_x, raw_y
    ring    name, delta (steps of 16 raw, ~300/rev; + = CW), raw
    battery name "BATTERY", charging (bool), level (0..100)
    link    name "usb"/"bt", up (bool) [, error, down_s]: the link was lost or reopened

Protocol facts used here (all in PANEL_PROTOCOL.md; verified unless marked):
  * Input stream read with length 65; only each report's payload is decoded
    (bytes beyond it are host-side leftovers).
  * 0x02: 64-bit button snapshot, bits 8..59. Applied as a snapshot, never as edges.
  * 0x05: 9 x s24 LE at 2+4k. Ball axes negated here (+x right, +y up).
  * 0x06: 16 x s32 LE at 1+4k, slots 0..11, raw = counts * 360.
  * 0x08 input/feature: battery [charging, level]; input only sent on change.
  * GET_REPORT needs the EXACT size (id + payload) or the panel stalls:
    input 0x02 len 9, feature 0x01 len 8, 0x08 len 3, 0x09 len 33, 0x0a len 2.
  * 0a 01 enables reporting for ~60 s; resent every 20 s (Resolve uses 30 s).
    Feature 0x0a reads back the enable flag.
  * At the ~60 s timeout every light goes off, and a bare 0a 01 brings the backlight
    back dim with the LEDs off: backlight and LEDs are resent after any re-enable.
  * Plugging USB drops Bluetooth at once (reads raise OSError); unplugging brings BT
    back 2-5 s later. With reconnect=True the driver reopens whichever transport
    appears, checking the device list at most once a second (faster polling made USB
    string-descriptor reads stall), then re-enables, restores the lights and re-reads
    the held buttons.
  * 03 a b: b = white backlight 0..100; a has no visible effect; Resolve sends a = b.
  * Output 0x04 (6 bytes): status LEDs, see LEDS. They ignore the backlight level.
  * Never writes feature 0x07 (unknown) or output 0x02 other than zeros.
"""
import logging
import os
import threading
import time
from dataclasses import dataclass, field

import hid

log = logging.getLogger("panel")

VID, PID, IFACE = 0x1EDB, 0xDA0F, 2
READ_LEN = 65
INIT = bytes([0x0A, 0x01])
# Keep-alive: 0a 01 every 20 s. Verified 2026-11-16 (logs/keepalive_20261116_180432.jsonl):
# one init -> stream died 58.7 s later; only 0a 01 revives it. 03 is brightness, not
# a keep-alive, and is sent only when it changes.
KEEPALIVE = {"init": 20.0}
TIMEOUT_GUARD = 55.0   # 0a 01 later than this after the last one = a re-enable (timer ~58.7-60 s)
ENUM_EVERY = 1.0       # minimum seconds between device-list checks while the link is down

# Status LEDs, output 0x04: name -> bit (logs/led_walk_20261116_184710.jsonl).
# Colours are fixed per key: BYPASS/DISABLE red, the rest green (VIEWER looks faint
# next to the white backlight; clearly green with the backlight off).
LEDS = {"BYPASS": 0, "USER": 1, "OFFSET": 2, "DISABLE": 3, "SHIFT_UP": 4,
        "SHIFT_DOWN": 5, "PLAY_STILL": 6, "WIPE_STILL": 7, "H_LITE": 8,
        "VIEWER": 9, "CURSOR": 10}

KNOB_RAW = 360       # raw per encoder count (verified divisibility)
BALL_RAW = 16        # raw per ball/ring step (verified divisibility)

BUTTONS = {
    8: "Y_LIFT", 9: "Y_GAMMA", 10: "Y_GAIN", 11: "CONTRAST",
    12: "PIVOT", 13: "MID_DETAIL", 14: "COLOR_BOOST", 15: "SHADOWS",
    16: "HIGHLIGHTS", 17: "SATURATION", 18: "HUE", 19: "LUM_MIX",
    20: "AUTO_COLOR", 21: "OFFSET", 22: "COPY", 23: "PASTE",
    24: "UNDO", 25: "REDO", 26: "DELETE", 27: "RESET",
    28: "BYPASS", 29: "DISABLE", 30: "USER", 31: "LOOP",
    32: "SHIFT_UP", 33: "SHIFT_DOWN",
    34: "PLAY_STILL", 35: "WIPE_STILL", 36: "GRAB_STILL",
    37: "H_LITE", 38: "VIEWER", 39: "CURSOR", 40: "SELECT",
    41: "RESET_LIFT", 42: "RESET_GAMMA", 43: "RESET_GAIN",
    44: "ADD_NODE", 45: "ADD_WINDOW", 46: "ADD_KEYFRM",
    47: "PREV_STILL", 48: "NEXT_STILL", 49: "PREV_KEYFRM", 50: "NEXT_KEYFRM",
    51: "PREV_NODE", 52: "NEXT_NODE", 53: "PREV_FRAME", 54: "NEXT_FRAME",
    55: "PREV_CLIP", 56: "NEXT_CLIP",
    57: "PLAY_REV", 58: "PLAY_FWD", 59: "STOP",
}
KNOBS = ["Y_LIFT", "Y_GAMMA", "Y_GAIN", "CONTRAST", "PIVOT", "MID_DETAIL",
         "COLOR_BOOST", "SHADOWS", "HIGHLIGHTS", "SATURATION", "HUE", "LUM_MIX"]
BALLS = {"LEFT": (0, 1), "MIDDLE": (3, 4), "RIGHT": (6, 7)}   # (x slot, y slot)
RINGS = {"LEFT": 2, "MIDDLE": 5, "RIGHT": 8}


@dataclass
class Event:
    kind: str
    name: str
    ts: float
    data: dict = field(default_factory=dict)

    def __str__(self):
        return "%-6s %-12s %s" % (self.kind, self.name,
                                  " ".join("%s=%s" % kv for kv in self.data.items()))


def _is_bt(d):
    # BLE HID over GATT: hidapi bus_type 2, path contains the HID service UUID 0x1812
    return d.get("bus_type") == 2 or b"{00001812-" in d["path"].lower()


def find_device(transport="auto"):
    """(hidapi path, "usb"|"bt"). transport: "usb" (interface 2), "bt" (BLE) or "auto"
    (USB if present, else BT; the panel itself drops BT while USB is plugged in)."""
    allp = hid.enumerate(VID, PID)
    usb = [d for d in allp if not _is_bt(d) and
           (d.get("interface_number") == IFACE or b"mi_02" in d["path"].lower())]
    bt = [d for d in allp if _is_bt(d)]
    for tr, devs in (("usb", usb), ("bt", bt)):
        if devs and transport in (tr, "auto"):
            return devs[0]["path"], tr
    raise IOError("Micro Color Panel not found over %s (present: %s)" % (
        transport, ["bt" if _is_bt(d) else "usb" for d in allp] or "none"))


def find_path(transport="usb"):
    """hidapi path of the panel over 'usb', 'bt' or 'auto'."""
    return find_device(transport)[0]


def _s24(b, i):
    v = b[i] | b[i + 1] << 8 | b[i + 2] << 16
    return v - (1 << 24) if v & 0x800000 else v


def _s32(b, i):
    return int.from_bytes(b[i:i + 4], "little", signed=True)


def _bits(b):
    return {pos * 8 + i for pos in range(1, 9) for i in range(8) if b[pos] >> i & 1}


class AxisLock:
    """Suppress the minor ball axis while one axis clearly dominates.

    Smoothed |dx|, |dy| (EMA); the minor axis is zeroed while its EMA is below
    `ratio` x the major's. Measured leak was 5-20% (hand drift included), so 0.25.
    """

    def __init__(self, ratio=0.25, alpha=0.3):
        self.ratio, self.alpha = ratio, alpha
        self.ema = {}

    def __call__(self, name, dx, dy):
        ex, ey = self.ema.get(name, (0.0, 0.0))
        a = self.alpha
        ex, ey = ex + a * (abs(dx) - ex), ey + a * (abs(dy) - ey)
        self.ema[name] = (ex, ey)
        if ey < self.ratio * ex:
            dy = 0
        elif ex < self.ratio * ey:
            dx = 0
        return dx, dy


class Panel:
    def __init__(self, keepalive=KEEPALIVE, axis_lock=None, path=None, open_device=True,
                 backlight=100, transport=None, reconnect=True):
        """axis_lock: None (raw) or a ratio such as 0.25.
        backlight: white backlight 0..100 set at start (Resolve's maximum is 100).
        transport: "auto" (USB if present, else BT), "usb" (HID interface 2) or "bt"
        (BLE HID); default: env PANEL_TRANSPORT, else "auto". The report protocol is
        identical on both.
        reconnect: when the link drops, poll()/run() keep going and reopen the panel
        on the allowed transport(s), emitting "link" events. False: the read error
        is raised, as before.
        open_device=False gives a decoder only (for replaying logs)."""
        self.transport = (transport or os.environ.get("PANEL_TRANSPORT", "auto")).lower()
        if self.transport not in ("auto", "usb", "bt"):
            raise ValueError("transport must be auto, usb or bt, not %r" % self.transport)
        self.reconnect = reconnect
        self.link = None            # transport in use: "usb" / "bt" (None while down)
        self.keepalive = dict(keepalive)
        self.lock = AxisLock(axis_lock) if axis_lock else None
        self.held = set()
        self.leds = set()
        self.backlight = backlight
        self.totals = {}            # (device, axis) -> accumulated value
        self.stats = {"sends_ok": 0, "sends_failed": 0, "writes_ok": 0, "writes_failed": 0,
                      "gets_failed": 0, "reports": 0, "unknown_ids": 0, "inits": 0,
                      "resyncs": 0, "links_lost": 0, "reopens": 0, "restores": 0}
        self._t_sent = {}
        self._io = threading.Lock()  # hidapi is not thread-safe: one call at a time
        self._stop = threading.Event()
        self._thread = None
        self._pending = []          # synthetic events with no callback yet -> next poll()
        self._t_enum = 0.0
        self._t_lost = None
        self.dev = None
        if not open_device:
            return
        if path is None:
            path, self.link = find_device(self.transport)
        else:
            self.link = self.transport if self.transport != "auto" else None
        self.dev = hid.device()
        self.dev.open_path(path)
        # Enable first, then the lights: 0a 01 after a timeout resets the backlight to a
        # dim level (quick_checks_led 2026-11-17), so 03/04 must follow it. (Resolve
        # sends 03 00 00 and LEDs off before enabling, then its brightness after.)
        self.init()
        self.restore()

    # ---------------------------------------------------------------- transport
    def send(self, pkt):
        """Send a feature report. Failures are counted and logged, never hidden."""
        try:
            with self._io:
                if self.dev is None:
                    raise IOError("link down")
                rc = self.dev.send_feature_report(bytes(pkt))
            if rc is None or rc < 0:
                raise IOError("send_feature_report returned %r" % rc)
            self.stats["sends_ok"] += 1
            return True
        except Exception as e:
            self.stats["sends_failed"] += 1
            log.error("feature send %s failed: %r", bytes(pkt).hex(" "), e)
            return False

    def write(self, pkt):
        """Send an output report (interrupt OUT). Failures counted and logged."""
        try:
            with self._io:
                if self.dev is None:
                    raise IOError("link down")
                rc = self.dev.write(bytes(pkt))
            if rc is None or rc <= 0:
                raise IOError("write returned %r" % rc)
            self.stats["writes_ok"] += 1
            return True
        except Exception as e:
            self.stats["writes_failed"] += 1
            log.error("output write %s failed: %r", bytes(pkt).hex(" "), e)
            return False

    def get_feature(self, rid, size):
        """GET_REPORT feature with the EXACT size (id + payload); None on failure."""
        try:
            with self._io:
                if self.dev is None:
                    raise IOError("link down")
                return bytes(self.dev.get_feature_report(rid, size))
        except Exception as e:
            self.stats["gets_failed"] += 1
            log.error("get feature 0x%02x/%d failed: %r", rid, size, e)
            return None

    def get_input(self, rid, size):
        try:
            with self._io:
                if self.dev is None:
                    raise IOError("link down")
                return bytes(self.dev.get_input_report(rid, size))
        except Exception as e:
            self.stats["gets_failed"] += 1
            log.error("get input 0x%02x/%d failed: %r", rid, size, e)
            return None

    # ---------------------------------------------------------------- control
    def init(self, callback=None):
        """Send 0a 01 and resync button state from get_input_report(0x02, 9).
        With a callback, emits synthetic press/release events for any difference
        between the old and the re-read state. If the read fails, held is cleared
        (synthetic releases), and the stream corrects it on the next 0x02."""
        self.stats["inits"] += 1
        ok = self.send(INIT)
        self._t_sent["init"] = time.time()
        self.resync(callback)
        return ok

    def resync(self, callback=None):
        now = self.state()
        if now is None:
            now = set()
        self.stats["resyncs"] += 1
        ts = time.time()
        for bit, down in [(b, False) for b in sorted(self.held - now)] + \
                         [(b, True) for b in sorted(now - self.held)]:
            e = Event("button", BUTTONS.get(bit, "b%02d" % bit), ts,
                      {"bit": bit, "down": down, "synthetic": True})
            if callback:
                callback(e)
            else:
                # e.g. the resync inside __init__: a button already held at start must
                # still produce its press, or its later release arrives unmatched
                self._pending.append(e)
        self.held = now

    def state(self):
        """Currently held button bits, read on request (None if the read fails)."""
        b = self.get_input(0x02, 9)
        return _bits(b) if b and b[0] == 0x02 and len(b) >= 9 else None

    def set_leds(self, names):
        """Light exactly these status LEDs (names from LEDS); others off.
        The wanted set is kept even if the write fails or the link is down, and is
        resent by restore() (after a reconnect or a re-enable)."""
        names = set(names)
        bad = names - set(LEDS)
        if bad:
            raise ValueError("no LED for %s; LEDs exist for %s" % (sorted(bad), sorted(LEDS)))
        self.leds = names
        if self._link_down():
            return False                        # restored on reopen
        v = 0
        for n in names:
            v |= 1 << LEDS[n]
        return self.write(bytes([0x04]) + v.to_bytes(6, "little"))

    def set_backlight(self, level):
        """White key backlight 0..100 (sent as 03 n n, like Resolve). Kept like the
        LEDs when the link is down."""
        level = max(0, min(100, int(level)))
        self.backlight = level
        if self._link_down():
            return False
        return self.send(bytes([0x03, level, level]))

    def restore(self):
        """Resend backlight and LEDs. Needed after any re-enable: at the timeout the
        panel turns every light off and a bare 0a 01 brings only a dim backlight back."""
        self.stats["restores"] += 1
        a = self.set_backlight(self.backlight)
        b = self.set_leds(self.leds)
        return a and b

    def _link_down(self):
        """True while reconnecting (not for a decoder-only Panel, whose sends fail)."""
        return self.dev is None and self.reconnect and self.stats["links_lost"] > 0

    def info(self):
        """{'version': '2.2.0', 'build': '155b3bfd', 'serial': '...'} (None on failure)."""
        v, s = self.get_feature(0x01, 8), self.get_feature(0x09, 33)
        out = {}
        if v and len(v) >= 8:
            out["build"] = v[1:5].hex()
            out["version"] = "%d.%d.%d" % (v[5], v[6], v[7])
        if s:
            out["serial"] = s[1:].split(b"\0")[0].decode("ascii", "replace")
        return out or None

    def battery(self):
        """{'charging': bool, 'level': 0..100} or None."""
        b = self.get_feature(0x08, 3)
        return {"charging": bool(b[1]), "level": b[2]} if b and len(b) >= 3 else None

    def enabled(self):
        """True while the panel is enabled (0a 01 sent less than ~60 s ago)."""
        b = self.get_feature(0x0A, 2)
        return None if not b else bool(b[1])

    def _keepalive(self):
        period = self.keepalive.get("init")
        last = self._t_sent.get("init", 0)
        gap = time.time() - last
        if period and gap >= period:
            self.send(INIT)               # periodic, NOT a recovery path
            self._t_sent["init"] = time.time()
            if last and gap >= TIMEOUT_GUARD:
                # the panel may have timed out (lights off): this 0a 01 re-enabled it
                log.warning("keep-alive %.0f s late: panel may have timed out; "
                            "restoring lights", gap)
                self.restore()

    def read_report(self, timeout_ms=20):
        """One raw report (bytes) or None. Raises if the device is gone."""
        self._keepalive()
        with self._io:
            d = self.dev.read(READ_LEN, timeout_ms)
        if not d:
            return None
        self.stats["reports"] += 1
        return bytes(d)

    # ---------------------------------------------------------------- decoding
    def decode(self, b, ts=None):
        """Raw report -> list of Events. Updates held/totals."""
        ts = ts or time.time()
        rid, ev = b[0], []
        if rid == 0x02:
            now = _bits(b)                      # bytes 1..8 only
            for bit in sorted(now - self.held):
                ev.append(Event("button", BUTTONS.get(bit, "b%02d" % bit), ts,
                                {"bit": bit, "down": True}))
            for bit in sorted(self.held - now):
                ev.append(Event("button", BUTTONS.get(bit, "b%02d" % bit), ts,
                                {"bit": bit, "down": False}))
            self.held = now
        elif rid == 0x05:
            for name, (sx, sy) in BALLS.items():
                rx, ry = _s24(b, 2 + 4 * sx), _s24(b, 2 + 4 * sy)
                if rx or ry:
                    dx, dy = -rx // BALL_RAW, -ry // BALL_RAW   # negate: +x right, +y up
                    if self.lock:
                        dx, dy = self.lock(name, dx, dy)
                    self._acc(name, "x", dx)
                    self._acc(name, "y", dy)
                    if dx or dy:
                        ev.append(Event("ball", name, ts,
                                        {"dx": dx, "dy": dy, "raw_x": rx, "raw_y": ry}))
            for name, s in RINGS.items():
                r = _s24(b, 2 + 4 * s)
                if r:
                    d = r // BALL_RAW
                    self._acc(name, "ring", d)
                    ev.append(Event("ring", name, ts, {"delta": d, "raw": r}))
        elif rid == 0x06:
            for k, name in enumerate(KNOBS):
                r = _s32(b, 1 + 4 * k)
                if r:
                    d = r // KNOB_RAW
                    self._acc(name, "knob", d)
                    ev.append(Event("knob", name, ts, {"index": k, "delta": d, "raw": r}))
        elif rid == 0x08:
            ev.append(Event("battery", "BATTERY", ts,
                            {"charging": bool(b[1]), "level": b[2]}))
        else:
            self.stats["unknown_ids"] += 1
            log.warning("unexpected report id 0x%02x: %s", rid, b[:16].hex(" "))
        return ev

    def _acc(self, dev, axis, v):
        if v:
            k = (dev, axis)                     # keyed per device AND axis
            self.totals[k] = self.totals.get(k, 0) + v

    # ---------------------------------------------------------------- loops
    def poll(self, timeout_ms=20):
        ev, self._pending = self._pending, []
        if self._link_down():
            if not self._stop.is_set():
                ev += self._try_reopen(timeout_ms)
            return ev
        try:
            b = self.read_report(timeout_ms)
        except OSError as e:
            if not self.reconnect:
                raise
            return ev + self._lost(e)
        return ev + (self.decode(b) if b else [])

    # ---------------------------------------------------------------- reconnect
    def _lost(self, err):
        """A read failed: the link is gone (USB unplugged, or BT dropped because USB
        was plugged in). Close, release held buttons, start looking for the panel."""
        tr, ts = self.link, time.time()
        log.warning("link %s lost: %r; reconnecting", tr, err)
        self.stats["links_lost"] += 1
        with self._io:
            dev, self.dev = self.dev, None
            try:
                dev.close()
            except Exception as e:      # a dead handle may refuse to close; logged
                log.warning("close after link loss failed: %r", e)
        self.link, self._t_lost, self._t_enum = None, ts, 0.0
        ev = [Event("link", tr or "?", ts, {"up": False, "error": repr(err)})]
        for bit in sorted(self.held):   # nothing can stay held on a dead link
            ev.append(Event("button", BUTTONS.get(bit, "b%02d" % bit), ts,
                            {"bit": bit, "down": False, "synthetic": True}))
        self.held = set()
        return ev

    def _try_reopen(self, timeout_ms):
        """One reopen attempt at most every ENUM_EVERY s; otherwise just waits."""
        now = time.time()
        if now - self._t_enum < ENUM_EVERY:
            time.sleep(timeout_ms / 1000.0)
            return []
        self._t_enum = now
        try:
            path, tr = find_device(self.transport)
        except IOError:
            time.sleep(timeout_ms / 1000.0)
            return []
        dev = hid.device()
        try:
            dev.open_path(path)
        except Exception as e:
            log.warning("reopen over %s failed: %r; retrying", tr, e)
            return []
        with self._io:
            self.dev, self.link = dev, tr
        self.stats["reopens"] += 1
        ev = []
        self.init(ev.append)            # 0a 01 + re-read held buttons (synthetic presses)
        self.restore()
        down = round(time.time() - self._t_lost, 1) if self._t_lost else None
        log.warning("link up over %s after %s s", tr, down)
        return [Event("link", tr, time.time(), {"up": True, "down_s": down})] + ev

    def run(self, callback):
        """Blocking read loop; never sleeps, so the OS input buffer cannot overflow
        (dropped 0x05/0x06 reports lose movement)."""
        while not self._stop.is_set():
            for e in self.poll(20):
                callback(e)

    def start(self, callback):
        self._thread = threading.Thread(target=self.run, args=(callback,), daemon=True)
        self._thread.start()

    def stop(self):
        self._stop.set()
        if self._thread:
            self._thread.join(1.0)

    def close(self, dark=True):
        """Like Resolve's quit: LEDs off and (dark=True) backlight 0, so a panel left
        on battery does not keep lighting. No disable report exists; the panel stops
        reporting ~60 s after the last 0a 01."""
        self.stop()             # _stop also ends reconnect attempts
        if self.dev:
            try:
                self.set_leds(())
                if dark:
                    self.set_backlight(0)
            finally:
                self.dev.close()
                self.dev = None

    def __enter__(self):
        return self

    def __exit__(self, *a):
        self.close()


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    with Panel(axis_lock=0.25) as p:
        print("panel up over %s; Ctrl+C to stop" % p.link)
        try:
            p.run(print)
        except KeyboardInterrupt:
            pass
        print("stats:", p.stats)

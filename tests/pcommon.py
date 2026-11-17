"""Shared helpers for the panel tests: open interface 2, init/keep-alive, logging.

Every feature-report send is counted and logged; failures are never swallowed.
"""
import json
import os
import time

import hid

VID, PID = 0x1EDB, 0xDA0F
READ_LEN = 65                    # HidP_GetCaps InputReportByteLength (verified, dump_caps)
INIT = bytes([0x0A, 0x01])       # enable / keep-alive (spec §3)
LIGHT = bytes([0x03, 0x64, 0x64])  # backlight brightness (spec §2, 0x03)
# Keep-alive: 0a 01 must be resent within ~60 s (verified, keepalive_test). 03 xx xx is
# backlight brightness, NOT a keep-alive: it is sent once at init and never resent,
# so tests that change brightness are not overridden (bug fixed 2026-11-16).
LIGHT_EVERY = None
INIT_EVERY = 15.0

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOGDIR = os.path.join(ROOT, "logs")


def log_path(name, ext="jsonl"):
    os.makedirs(LOGDIR, exist_ok=True)
    return os.path.join(LOGDIR, time.strftime(name + "_%Y%m%d_%H%M%S." + ext))


def hexs(b):
    return " ".join("%02x" % x for x in b)


class JsonLog:
    def __init__(self, path):
        self.path = path
        self.f = open(path, "w", encoding="utf-8", buffering=1)  # line-buffered: survives kills

    def write(self, **ev):
        ev.setdefault("ts", round(time.time(), 4))
        self.f.write(json.dumps(ev) + "\n")

    def close(self):
        self.f.close()


def find_iface2():
    """Kept for older scripts: honours PANEL_TRANSPORT like find_panel()."""
    return find_panel()


TRANSPORT = os.environ.get("PANEL_TRANSPORT", "usb").lower()   # usb | bt


def is_bt(d):
    return d.get("bus_type") == 2 or b"{00001812-" in d["path"].lower()


def find_panel(transport=None):
    """usb: HID interface 2 (MI_02). bt: the BLE HID collection (hidapi bus_type 2)."""
    transport = (transport or TRANSPORT).lower()
    allp = hid.enumerate(VID, PID)
    if transport == "bt":
        devs = [d for d in allp if is_bt(d)]
    else:
        devs = [d for d in allp if not is_bt(d) and
                (d.get("interface_number") == 2 or b"mi_02" in d["path"].lower())]
    if not devs:
        raise SystemExit("Panel not found over %s (VID %04x PID %04x); present: %s" % (
            transport, VID, PID, [("bt" if is_bt(d) else "usb") for d in allp]))
    return devs[0]


class Panel:
    """Thin transport. keepalive=False lets a test control sends itself."""

    def __init__(self, log=None, keepalive=True, init=True):
        self.log = log
        self.keepalive = keepalive
        self.sends_ok = 0
        self.sends_failed = 0
        self.dev = hid.device()
        self.dev.open_path(find_iface2()["path"])
        now = time.time()
        self.t_light = self.t_init = now
        if init:
            self.send(INIT, "init")
            self.send(LIGHT, "init")

    def send(self, pkt, why=""):
        pkt = bytes(pkt)
        try:
            rc = self.dev.send_feature_report(pkt)
            ok = rc is not None and rc >= 0
            err = None if ok else "rc=%r" % rc
        except Exception as e:  # counted and logged, not hidden
            ok, rc, err = False, None, repr(e)
        if ok:
            self.sends_ok += 1
        else:
            self.sends_failed += 1
            print("!! feature send %s failed: %s" % (hexs(pkt), err))
        if self.log:
            self.log.write(t="send", why=why, data=hexs(pkt), ok=ok, rc=rc, err=err)
        return ok

    def tick(self):
        if not self.keepalive:
            return
        now = time.time()
        if LIGHT_EVERY and now - self.t_light >= LIGHT_EVERY:
            self.send(LIGHT, "keepalive")
            self.t_light = now
        if now - self.t_init >= INIT_EVERY:
            self.send(INIT, "keepalive")
            self.t_init = now

    def read(self, timeout_ms=50):
        """Returns bytes (as delivered by hidapi) or None on timeout."""
        self.tick()
        d = self.dev.read(READ_LEN, timeout_ms)
        return bytes(d) if d else None

    def close(self):
        self.dev.close()


def button_bits(b):
    """0x02 report -> set of pressed bit numbers (bit = byte_pos*8 + bit, byte_pos 1..8)."""
    return {pos * 8 + i for pos in range(1, 9) for i in range(8) if b[pos] >> i & 1}


class TapGate:
    """Detects the same button pressed `n` times within `window` seconds.

    Feed every report; returns the bit number once the gesture completes.
    Keeps its own held-state, independent of any test logic.
    """

    def __init__(self, n=5, window=3.0):
        self.n, self.window = n, window
        self.held = set()
        self.taps = {}

    def feed(self, b):
        if not b or b[0] != 0x02:
            return None
        now_held = button_bits(b)
        t = time.time()
        for bit in now_held - self.held:
            q = [x for x in self.taps.get(bit, []) if t - x <= self.window] + [t]
            self.taps[bit] = q
            if len(q) >= self.n:
                self.taps[bit] = []
                self.held = now_held
                return bit
        self.held = now_held
        return None


def wait_enter(msg, prompt=True):
    print("\n>>> " + msg)
    if prompt:
        input("    Press Enter when ready... ")

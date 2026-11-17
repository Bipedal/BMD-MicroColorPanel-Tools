"""Task 6 smoke test for panel.py.

Usage: python tests/smoke_panel.py offline     # replay verified logs, no device
       python tests/smoke_panel.py live        # move everything, end with USER x5
       python tests/smoke_panel.py features    # info/battery, held-button state(), LEDs
       python tests/smoke_panel.py reconnect   # unplug/replug USB; driver follows (cap 180 s)

offline: maps in panel.py == tests/panel_map.py; replaying the task-4 captures gives
         ball RIGHT -> +dx, UP -> +dy, ring CW -> +delta, knob CW -> +delta, and the
         knob order K0..K11.
live:    prints events, checks press/release balance, unknown IDs, send failures.
Log: logs/smoke_<mode>_<ts>.jsonl
"""
import glob
import json
import os
import re
import sys
import time
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
import panel  # noqa: E402
import panel_map  # noqa: E402
from pcommon import LOGDIR, JsonLog, log_path  # noqa: E402

fails = []


def check(cond, msg):
    print("   %s  %s" % ("ok  " if cond else "FAIL", msg))
    if not cond:
        fails.append(msg)


def latest(pattern):
    c = [f for f in glob.glob(os.path.join(LOGDIR, pattern)) if "ABORTED" not in f]
    return max(c) if c else None


def replay_steps(path):
    """-> {(dev, dir): [events]} using a fresh offline decoder."""
    p = panel.Panel(open_device=False)
    out, step = defaultdict(list), None
    for line in open(path, encoding="utf-8"):
        e = json.loads(line)
        if e.get("t") == "step":
            step = (e["dev"], e["dir"])
        elif e.get("t") == "rx" and step and e["rid"] in (5, 6):
            b = bytes.fromhex(e["data"].replace(" ", ""))
            b = b + bytes(65 - len(b))
            out[step] += p.decode(b, e["ts"])
    return out


def offline():
    print("== maps")
    check(panel.BUTTONS == panel_map.BUTTONS, "BUTTONS identical to tests/panel_map.py")
    check(panel.KNOBS == panel_map.KNOBS, "KNOBS identical")
    check(panel.BALLS == panel_map.BALL_SLOTS, "ball slots identical")
    check(panel.RINGS == panel_map.RING_SLOTS, "ring slots identical")

    f = latest("axis_balls_*.jsonl")
    print("== balls replay: %s" % f)
    st = replay_steps(f)
    for ball in panel.BALLS:
        for d, key, sign in (("UP", "dy", 1), ("DOWN", "dy", -1),
                             ("RIGHT", "dx", 1), ("LEFT", "dx", -1)):
            s = sum(e.data[key] for e in st[(ball, d)] if e.kind == "ball" and e.name == ball)
            check(s * sign > 0, "%s ball %-5s -> %s sum %+d" % (ball, d, key, s))

    f = latest("axis_rings_*.jsonl")
    print("== rings replay: %s" % f)
    st = replay_steps(f)
    for ring in panel.RINGS:
        for d, sign in (("CW", 1), ("CCW", -1)):
            s = sum(e.data["delta"] for e in st[(ring, d)] if e.kind == "ring" and e.name == ring)
            check(s * sign > 0, "%s ring %-3s -> delta sum %+d" % (ring, d, s))

    f = latest("axis_knobs_*.jsonl")
    print("== knobs replay: %s" % f)
    st = replay_steps(f)
    ev = [e for e in st[("ALL", "ORDER")] if e.kind == "knob"]
    first = []
    for e in ev:
        if e.name not in first:
            first.append(e.name)
    check(first == panel.KNOBS, "knob order %s" % " ".join(first))
    net = defaultdict(int)
    for e in ev:
        net[e.name] += e.data["delta"]
    # net per knob, not per report: the capture has a small real back-turn on Y_GAIN
    check(all(v > 0 for v in net.values()) and len(net) == 12,
          "every knob net positive while turning CW (min %d)" % min(net.values()))
    turn = sum(e.data["delta"] for e in st[("Y_LIFT", "TURN")] if e.kind == "knob")
    check(3700 < turn < 4500, "Y_LIFT full turn = %d counts (expect ~4096)" % turn)

    print("== accumulator keyed by (device, axis)")
    p = panel.Panel(open_device=False)
    b = bytearray(65)
    b[0] = 5
    for slot, v in ((0, -160), (1, 320)):       # left ball raw x=-160, raw y=+320
        v &= 0xFFFFFF
        b[2 + 4 * slot], b[3 + 4 * slot], b[4 + 4 * slot] = v & 255, v >> 8 & 255, v >> 16
    p.decode(bytes(b))
    check(p.totals == {("LEFT", "x"): 10, ("LEFT", "y"): -20},
          "separate x/y totals %s" % p.totals)

    print("== init resyncs held state from get_input_report(0x02, 9)")
    p.send = lambda pkt: True
    snap = bytearray(9)
    snap[0] = 2
    snap[4] = 0x20                            # bit 37 H_LITE held
    p.get_input = lambda rid, n: bytes(snap)
    p.held = {22}                             # COPY believed held, actually released
    ev = []
    p.init(ev.append)
    got = sorted((e.name, e.data["down"]) for e in ev)
    check(p.held == {37} and got == [("COPY", False), ("H_LITE", True)]
          and all(e.data.get("synthetic") for e in ev), "resync events %s" % got)
    p.get_input = lambda rid, n: None         # read fails -> clear, synthetic release
    ev = []
    p.init(ev.append)
    check(not p.held and [(e.name, e.data["down"]) for e in ev] == [("H_LITE", False)],
          "failed read -> held cleared with synthetic release")

    print("== button held at startup: press queued for the first poll (live bug 2026-11-16)")
    q = panel.Panel(open_device=False)
    q.send = lambda pkt: True
    q.get_input = lambda rid, n: bytes(snap)  # H_LITE held at start
    q.init()                                  # no callback, as inside __init__
    q.read_report = lambda timeout_ms=20: bytes([0x02]) + bytes(64)   # then released
    ev = q.poll()
    got = [(e.name, e.data["down"]) for e in ev]
    check(got == [("H_LITE", True), ("H_LITE", False)], "press then release: %s" % got)

    print("== LED packing (output 0x04)")
    sent = []
    p.write = lambda pkt: sent.append(bytes(pkt)) or True
    p.set_leds({"BYPASS", "CURSOR"})          # bits 0 and 10
    check(sent[-1] == bytes([0x04, 0x01, 0x04, 0, 0, 0, 0]), "BYPASS+CURSOR -> %s" % sent[-1].hex(" "))
    p.set_leds(())
    check(sent[-1] == bytes([0x04] + [0] * 6), "all off -> %s" % sent[-1].hex(" "))
    try:
        p.set_leds({"STOP"})
        check(False, "unknown LED name rejected")
    except ValueError:
        check(True, "unknown LED name rejected")
    fs = []
    p.send = lambda pkt: fs.append(bytes(pkt)) or True
    p.set_backlight(140)
    check(fs[-1] == bytes([0x03, 100, 100]), "backlight clamps to 100 -> %s" % fs[-1].hex(" "))

    print("== battery input 0x08")
    ev = p.decode(bytes([0x08, 0x01, 0x63]) + bytes(62))
    check(len(ev) == 1 and ev[0].kind == "battery" and ev[0].data == {"charging": True, "level": 99},
          "08 01 63 -> %s" % [(e.kind, e.data) for e in ev])

    print("== stale tail ignored")
    p = panel.Panel(open_device=False)
    b = bytearray(65)
    b[0] = 2
    b[3] = 0x40            # bit 30 USER
    b[20:30] = b"\xff" * 10  # garbage beyond payload
    ev = p.decode(bytes(b))
    check([e.name for e in ev] == ["USER"], "only USER decoded: %s" % [e.name for e in ev])
    reconnect_sim()


class FakeTime:
    def __init__(self):
        self.t = 1000.0

    def time(self):
        return self.t

    def sleep(self, s):
        self.t += s


class FakeHid:
    """Panel present over self.where ("usb" / "bt" / None). A handle opened on one
    transport raises OSError on read once the panel has left it (as hidapi does)."""

    def __init__(self, clock):
        self.clock, self.where = clock, "usb"
        self.enums, self.sent, self.held_bits = [], [], set()
        self.queue = []

    def enumerate(self, vid, pid):
        self.enums.append(self.clock.t)
        if self.where == "usb":
            return [{"path": b"\\\\?\\hid#vid_1edb&pid_da0f&mi_02#x", "interface_number": 2,
                     "bus_type": 1}]
        if self.where == "bt":
            return [{"path": b"\\\\?\\hid#{00001812-0000-1000-8000-00805f9b34fb}#x",
                     "interface_number": -1, "bus_type": 2}]
        return []

    def device(self):
        fh = self

        class D:
            def open_path(self, p):
                self.tr = "bt" if b"{00001812-" in p else "usb"
                if fh.where != self.tr:
                    raise OSError("open failed")

            def _alive(self):
                if fh.where != self.tr:
                    raise OSError("read error")

            def send_feature_report(self, b):
                self._alive()
                fh.sent.append((self.tr, bytes(b).hex(" ")))
                return len(b)

            def write(self, b):
                self._alive()
                fh.sent.append((self.tr, bytes(b).hex(" ")))
                return len(b)

            def get_input_report(self, rid, n):
                self._alive()
                b = bytearray(9)
                b[0] = 2
                for bit in fh.held_bits:
                    b[bit // 8] |= 1 << bit % 8
                return list(b)

            def get_feature_report(self, rid, n):
                self._alive()
                return [rid] + [0] * (n - 1)

            def read(self, n, to):
                self._alive()
                if fh.queue:
                    return list(fh.queue.pop(0))
                fh.clock.t += to / 1000.0
                return []

            def close(self):
                pass
        return D()


def reconnect_sim():
    print("== reconnect: USB unplug -> BT, BT -> USB plug; lights restored (simulated)")
    real_hid, real_time = panel.hid, panel.time
    clk = FakeTime()
    fh = FakeHid(clk)
    panel.hid, panel.time = fh, clk
    try:
        p = panel.Panel(transport="auto")
        check(p.link == "usb" and fh.sent == [("usb", "0a 01"), ("usb", "03 64 64"),
                                              ("usb", "04 00 00 00 00 00 00")],
              "auto opens USB; order 0a 01, 03, 04: %s" % fh.sent)
        p.set_leds({"OFFSET"})
        p.set_backlight(40)
        usr = bytearray(9)
        usr[0], usr[3] = 2, 0x40                  # USER held
        fh.queue.append(bytes(usr))
        ev = p.poll()
        check([(e.name, e.data["down"]) for e in ev] == [("USER", True)], "USER pressed")
        # unplug USB: BT appears 3 s later
        fh.where, t_unplug, fh.enums = None, clk.t, []
        fh.held_bits = {30}                       # still held when BT is back
        events = []
        while clk.t - t_unplug < 3.0:
            events += p.poll()
        p.set_leds({"OFFSET", "BYPASS"})          # changed while the link is down
        fh.where, fh.sent = "bt", []
        while clk.t - t_unplug < 6.0:
            events += p.poll()
        got = [(e.kind, e.name, e.data.get("up", e.data.get("down"))) for e in events]
        check(got == [("link", "usb", False), ("button", "USER", False),
                      ("link", "bt", True), ("button", "USER", True)],
              "events: link down, USER released, link up over bt, USER pressed again: %s" % got)
        gaps = [b - a for a, b in zip(fh.enums, fh.enums[1:])]
        check(fh.enums and min(gaps) >= 0.999 and len(fh.enums) <= 7,
              "device list checked at most 1/s (%d checks in 6 s)" % len(fh.enums))
        check(fh.sent[:3] == [("bt", "0a 01"), ("bt", "03 28 28"), ("bt", "04 05 00 00 00 00 00")],
              "BT reopen: 0a 01, backlight 40, LEDs OFFSET+BYPASS: %s" % fh.sent[:3])
        check(p.link == "bt" and p.stats["reopens"] == 1 and p.stats["links_lost"] == 1,
              "link bt, 1 loss, 1 reopen")
        # plug USB in: BT drops at once
        fh.where, fh.sent, fh.held_bits = "usb", [], set()
        t_plug, events = clk.t, []
        while clk.t - t_plug < 2.5:
            events += p.poll()
        got = [(e.kind, e.name, e.data.get("up", e.data.get("down"))) for e in events]
        check(got == [("link", "bt", False), ("button", "USER", False), ("link", "usb", True)]
              and p.link == "usb", "BT -> USB: %s" % got)
        check(fh.sent[:3] == [("usb", "0a 01"), ("usb", "03 28 28"),
                              ("usb", "04 05 00 00 00 00 00")], "USB reopen restores lights")
        # a late keep-alive (stalled host) restores the lights; a normal one does not
        fh.sent = []
        p._t_sent["init"] = clk.t - 21
        p.poll()
        check(fh.sent == [("usb", "0a 01")], "on-time keep-alive: 0a 01 only")
        fh.sent = []
        p._t_sent["init"] = clk.t - 56
        p.poll()
        check(fh.sent == [("usb", "0a 01"), ("usb", "03 28 28"), ("usb", "04 05 00 00 00 00 00")],
              "keep-alive 56 s late: 0a 01 then lights: %s" % fh.sent)
        # fixed transport waits for its own link
        q = panel.Panel(transport="usb")
        fh.where, fh.sent = "bt", []
        t1 = clk.t
        while clk.t - t1 < 3:
            q.poll()
        check(q.dev is None and not fh.sent, "transport=usb ignores BT while unplugged")
        # reconnect=False raises as before
        fh.where = "usb"
        r = panel.Panel(transport="usb", reconnect=False)
        fh.where = None
        try:
            r.poll()
            check(False, "reconnect=False raises on link loss")
        except OSError:
            check(True, "reconnect=False raises on link loss")
        p._stop.set()
        fh.where, fh.enums = None, []
        p._lost(OSError("x"))
        p.poll()
        check(not fh.enums, "no reconnect attempts after stop/close")
        check(p.stats["sends_failed"] == 0 and p.stats["writes_failed"] == 0,
              "no failed sends/writes counted while down")
    finally:
        panel.hid, panel.time = real_hid, real_time


def features():
    """Live checks of the new panel.py features. Steps:
    F1 info/battery/enabled (automatic)
    F2 hold H_LITE and, while holding it, press USER 5x -> state() must contain H_LITE
    F3 LED chase (each of the 11 LEDs alone, 0.5 s), all on 2 s, then all on with the
       backlight at 0 for 4 s (is VIEWER clearly green?), backlight back to 100
    F4 close(): LEDs off and backlight 0 -> panel should go dark
    """
    from pcommon import TapGate
    path = log_path("smoke_features")
    log = JsonLog(path)
    p = panel.Panel()
    try:
        print("== F1 info")
        info, bat, en = p.info(), p.battery(), p.enabled()
        print("   info %s, battery %s, enabled %s" % (info, bat, en))
        log.write(t="info", info=info, battery=bat, enabled=en)
        # firmware 2.2.0 on the test unit; the serial is 32 ASCII hex digits (spec §2)
        check(info and re.fullmatch(r"\d+\.\d+\.\d+", info.get("version", "")) and
              re.fullmatch(r"[0-9A-F]{32}", info.get("serial", "")), "version/serial read")
        check(bat is not None and 0 <= bat["level"] <= 100, "battery read")
        check(en is True, "enabled flag reads True after init")

        print("== F2 hold H_LITE, press USER x5 while holding", flush=True)
        gate, t0 = TapGate(5, 3.0), time.time()
        fired = False
        while time.time() - t0 < 120:
            b = p.read_report(30)
            if b:
                p.decode(b)
                if gate.feed(b) == 30:
                    fired = True
                    break
        st = p.state()
        names = sorted(panel.BUTTONS.get(x, x) for x in (st or ()))
        log.write(t="state", fired=fired, state=names, stream=sorted(p.held))
        print("   state() = %s; stream held = %s" % (names, sorted(panel.BUTTONS.get(x) for x in p.held)))
        check(fired and st is not None and 37 in st, "state() shows H_LITE while it is held")

        print("== F3 LED chase", flush=True)
        time.sleep(1.0)
        for name in panel.LEDS:
            p.set_leds({name})
            time.sleep(0.5)
        p.set_leds(panel.LEDS)
        time.sleep(2.0)
        p.set_backlight(0)
        time.sleep(4.0)
        p.set_backlight(100)
        p.set_leds(())
        time.sleep(1.0)
    finally:
        print("== F4 close(): panel should go dark", flush=True)
        p.close()
    check(p.stats["sends_failed"] == 0 and p.stats["writes_failed"] == 0,
          "sends ok=%d failed=%d, writes ok=%d failed=%d, gets failed=%d"
          % (p.stats["sends_ok"], p.stats["sends_failed"], p.stats["writes_ok"],
             p.stats["writes_failed"], p.stats["gets_failed"]))
    log.write(t="summary", stats=p.stats, fails=fails)
    log.close()
    print("   log: %s" % path)


def live():
    path = log_path("smoke_live")
    log = JsonLog(path)
    cnt, downs, ups = Counter(), Counter(), Counter()
    taps = []
    print(">>> Move everything: buttons, knobs, balls, rings, for at least 90 s. "
          "End with USER x5.", flush=True)
    with panel.Panel(axis_lock=0.25) as p:
        t0 = last_ev = time.time()
        while time.time() - t0 < 300:
            for e in p.poll(20):
                last_ev = e.ts
                cnt[e.kind] += 1
                log.write(t="ev", kind=e.kind, name=e.name, **e.data)
                if e.kind == "button":
                    (downs if e.data["down"] else ups)[e.name] += 1
                    if e.name == "USER" and e.data["down"]:
                        taps = [t for t in taps if e.ts - t < 3] + [e.ts]
            if len(taps) >= 5:
                break
        t_q = time.time()                     # let the last releases arrive
        while time.time() - t_q < 1.0:
            for e in p.poll(20):
                if e.kind == "button":
                    (downs if e.data["down"] else ups)[e.name] += 1
        print("== live summary (%.0f s)" % (time.time() - t0))
        print("   events: %s" % dict(cnt))
        print("   totals: %s" % {"%s.%s" % k: v for k, v in sorted(p.totals.items())})
        check(cnt["button"] > 0 and cnt["knob"] > 0 and cnt["ball"] > 0 and cnt["ring"] > 0,
              "all four event kinds seen")
        check(downs == ups, "every press has a release")
        check(not p.held, "nothing held at end: %s" % sorted(p.held))
        check(p.stats["unknown_ids"] == 0, "no unknown report IDs")
        check(last_ev - t0 > 65, "events still arriving %.0f s after start (>65 s proves "
              "the 20 s keep-alive; one init alone dies at ~60 s)" % (last_ev - t0))
        check(p.stats["sends_failed"] == 0, "feature sends ok=%d failed=%d"
              % (p.stats["sends_ok"], p.stats["sends_failed"]))
        log.write(t="summary", stats=p.stats, counts=dict(cnt))
    log.close()
    print("   log: %s" % path)


def reconnect_live():
    """Start on USB with OFFSET+BYPASS lit and the backlight at 40.
    R1 unplug USB; wait until the lights are back (BT); press USER once.
    R2 plug USB back in; wait 3 s; press USER once.  End: USER x5."""
    path = log_path("smoke_reconnect")
    log = JsonLog(path)
    links, taps, users = [], [], []
    p = panel.Panel(transport="auto")
    p.set_leds({"OFFSET", "BYPASS"})
    p.set_backlight(40)
    print(">>> on %s. R1 unplug USB, wait for the lights, press USER once. "
          "R2 plug USB in, wait 3 s, press USER once. End with USER x5." % p.link, flush=True)
    t0 = time.time()
    try:
        while time.time() - t0 < 180:
            for e in p.poll(20):
                rel = round(e.ts - t0, 2)
                if e.kind == "link":
                    links.append((rel, e.name, e.data["up"]))
                    print("  %6.1f s  link %s %s %s" % (rel, e.name, "UP" if e.data["up"] else "DOWN",
                                                       e.data.get("down_s", "")), flush=True)
                    log.write(t="link", rel=rel, **{"tr": e.name}, **e.data)
                elif e.kind == "button":
                    log.write(t="button", rel=rel, name=e.name, link=p.link, **e.data)
                    if e.name == "USER" and e.data["down"] and not e.data.get("synthetic"):
                        users.append((rel, p.link))
                        taps = [t for t in taps if e.ts - t < 3] + [e.ts]
            if len(taps) >= 5:
                break
    finally:
        p.close()
    seq = [(tr, up) for _, tr, up in links]
    print("== reconnect summary (%.0f s)" % (time.time() - t0))
    print("   links: %s" % links)
    print("   USER presses (time, link): %s" % users[:-5])
    check(seq[:4] == [("usb", False), ("bt", True), ("bt", False), ("usb", True)],
          "usb lost -> bt up -> bt lost -> usb up")
    check(any(l == "bt" for _, l in users) and any(l == "usb" for _, l in users),
          "USER seen over bt and over usb after the switches")
    check(p.stats["sends_failed"] == 0 and p.stats["writes_failed"] == 0,
          "sends ok=%d failed=%d, writes ok=%d failed=%d, reopens %d, restores %d" % (
              p.stats["sends_ok"], p.stats["sends_failed"], p.stats["writes_ok"],
              p.stats["writes_failed"], p.stats["reopens"], p.stats["restores"]))
    log.write(t="summary", links=links, users=users, stats=p.stats, fails=fails)
    log.close()
    print("   log: %s" % path)


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "offline"
    {"offline": offline, "live": live, "features": features, "reconnect": reconnect_live}[mode]()
    print("\n%s (%d failures)" % ("PASS" if not fails else "FAIL", len(fails)))
    sys.exit(1 if fails else 0)

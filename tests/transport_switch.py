"""USB <-> Bluetooth hand-over (B4b). Start with the panel connected over Bluetooth and
USB UNPLUGGED. Keep rolling a ball gently on and off the whole time.

Usage: python tests/transport_switch.py --usb-init now|none [--cap 240]
       python tests/transport_switch.py --offline        (simulated devices, no panel)

Timeline (you): roll ~15 s on BT -> plug USB, roll 60 s (note when the blue light
                changes) -> unplug USB, roll 20 s -> USER x5 (ends; either transport)
Script:
  * device list polled every 1.0 s in a BACKGROUND thread (0.3 s caused ~10 string-descriptor STALLs per minute, b4b1) (B4 stalled the reads)
  * BT: opened + 0a 01 keep-alive every 15 s while present
  * USB: --usb-init now  -> 0a 01 at open + keep-alive every 15 s (like Resolve)
         --usb-init none -> never sends anything on USB
  * every 5 s: 0x0a + 0x07 read on each open link (GET, exact size)
  * every printed line carries the time since start
Writes only 0a 01. Log: logs/transport_switch_<mode>_<ts>.jsonl
"""
import argparse
import json
import queue
import sys
import threading
import time
from collections import Counter

from pcommon import PID, VID, JsonLog, TapGate, hexs, is_bt, log_path

T0 = time.time()


def say(msg):
    print("%6.1f s  %s" % (time.time() - T0, msg), flush=True)


class Link:
    def __init__(self, hidmod, name, path, log, init):
        self.name, self.log, self.do_init = name, log, init
        self.dev = hidmod.device()
        self.dev.open_path(path)
        self.t_init = 0
        self.t_status = time.time()
        self.alive = True
        self.n = Counter()
        self.first_rx = None
        log.write(t="open", tr=name, init=init)
        say("[%s] opened (%s)" % (name, "init" if init else "NO init"))
        self.status("open")
        if init:
            self.init()

    def get(self, rid, n):
        try:
            return bytes(self.dev.get_feature_report(rid, n))
        except Exception as e:
            self.log.write(t="get_err", tr=self.name, rid=rid, err=repr(e))
            return None

    def status(self, why):
        en, f7 = self.get(0x0A, 2), self.get(0x07, 6)
        self.log.write(t="status", tr=self.name, why=why,
                       enable=hexs(en) if en else None, f07=hexs(f7) if f7 else None)
        say("[%s] %s: enable=%s  0x07=%s  rx so far %s" % (
            self.name, why, hexs(en) if en else "?", hexs(f7) if f7 else "?", dict(self.n)))
        self.t_status = time.time()

    def init(self):
        try:
            self.dev.send_feature_report(bytes([0x0A, 0x01]))
            self.log.write(t="send", tr=self.name, data="0a 01", ok=True)
        except Exception as e:
            self.log.write(t="send", tr=self.name, data="0a 01", ok=False, err=repr(e))
            say("[%s] !! 0a 01 failed: %r" % (self.name, e))
        self.t_init = time.time()

    def read(self):
        now = time.time()
        if self.do_init and now - self.t_init > 15:
            self.init()
        if now - self.t_status > 5:
            self.status("periodic")
        try:
            d = self.dev.read(65, 5)
        except Exception as e:
            self.log.write(t="read_err", tr=self.name, err=repr(e))
            say("[%s] read error -> closed: %r" % (self.name, e))
            self.close()
            return None
        if d:
            b = bytes(d)
            self.n[b[0]] += 1
            if self.first_rx is None:
                self.first_rx = time.time()
                say("[%s] first report (0x%02x)" % (self.name, b[0]))
            self.log.write(t="rx", tr=self.name, rid=b[0], data=hexs(b[:9]))
            return b
        return None

    def close(self):
        if self.alive:
            self.alive = False
            try:
                self.dev.close()
            except Exception as e:
                self.log.write(t="close_err", tr=self.name, err=repr(e))
            self.log.write(t="close", tr=self.name, rx=dict(self.n))
            say("[%s] closed; rx %s" % (self.name, dict(self.n)))


def present(hidmod):
    out = {}
    for d in hidmod.enumerate(VID, PID):
        if is_bt(d):
            out["bt"] = d["path"]
        elif d.get("interface_number") == 2 or b"mi_02" in d["path"].lower():
            out["usb"] = d["path"]
    return out


class Watcher(threading.Thread):
    """Polls the device list off the read thread; posts (t, 'appeared'|'gone', tr, path)."""

    def __init__(self, hidmod, log, period=1.0):
        super().__init__(daemon=True)
        self.hidmod, self.log, self.period = hidmod, log, period
        self.q = queue.Queue()
        self.stop = threading.Event()
        self.seen = {}

    def run(self):
        while not self.stop.is_set():
            t = time.time()
            try:
                now = present(self.hidmod)
            except Exception as e:
                self.log.write(t="enum_err", err=repr(e))
                now = self.seen
            dt = time.time() - t
            if dt > 1.0:
                self.log.write(t="enum_slow", secs=round(dt, 2))
                self.q.put((time.time(), "note", "enum", "enumerate took %.1f s" % dt))
            for tr in sorted(set(now) - set(self.seen)):
                self.q.put((t, "appeared", tr, now[tr]))
            for tr in sorted(set(self.seen) - set(now)):
                self.q.put((t, "gone", tr, None))
            self.seen = dict(now)
            self.stop.wait(self.period)


def run(hidmod, usb_init, cap, log, gate_clock=None):
    links, gate = {}, TapGate(5, 3.0)
    timeline = []

    def ev(msg):
        timeline.append("%6.1f s  %s" % (time.time() - T0, msg))
        say(msg)
        log.write(t="event", msg=msg)

    first = present(hidmod)
    if "bt" not in first or "usb" in first:
        ev("!! expected Bluetooth only at start; present: %s" % sorted(first))
    w = Watcher(hidmod, log)
    w.start()
    say(">>> roll ~15 s, plug USB, roll 60 s, unplug USB, roll 20 s, USER x5")
    done = False
    try:
        while time.time() - T0 < cap and not done:
            while True:
                try:
                    t, kind, tr, arg = w.q.get_nowait()
                except queue.Empty:
                    break
                if kind == "note":
                    ev(arg)
                elif kind == "appeared":
                    ev("%s appeared" % tr.upper())
                    if tr not in links or not links[tr].alive:
                        try:
                            links[tr] = Link(hidmod, tr, arg, log,
                                             init=(tr == "bt" or usb_init == "now"))
                        except Exception as e:
                            ev("%s open failed: %r" % (tr.upper(), e))
                else:
                    ev("%s disappeared" % tr.upper())
                    if tr in links:
                        links[tr].close()
            idle = True
            for tr, ln in list(links.items()):
                if not ln.alive:
                    continue
                b = ln.read()
                if b:
                    idle = False
                    if gate.feed(b) == 30:
                        ev("USER x5 on %s" % tr.upper())
                        done = True
            if idle and not any(l.alive for l in links.values()):
                time.sleep(0.01)   # nothing open: don't spin
    except KeyboardInterrupt:
        ev("interrupted")
    finally:
        w.stop.set()
        for ln in links.values():
            ln.close()
    if not done:
        ev("time cap reached")
    return timeline


def summary(path, timeline):
    print("\n== timeline")
    for s in timeline:
        print("   " + s)
    rows = [json.loads(l) for l in open(path, encoding="utf-8")]
    rx = [r for r in rows if r["t"] == "rx"]
    win = Counter((int((r["ts"] - T0) // 5) * 5, r["tr"]) for r in rx)
    print("\n== reports per 5 s window (usb / bt)")
    for wn in sorted({k[0] for k in win}):
        print("   %4d s: usb %4d   bt %4d" % (wn, win.get((wn, "usb"), 0), win.get((wn, "bt"), 0)))
    print("\n== status reads")
    for r in rows:
        if r["t"] == "status":
            print("   %6.1f s [%s] %-8s enable=%s 0x07=%s" % (
                r["ts"] - T0, r["tr"], r["why"], r["enable"], r["f07"]))
    bad = [r for r in rows if r["t"] == "send" and not r["ok"]]
    print("\nfailed sends: %d   get errors: %d   slow enumerations: %d" % (
        len(bad), sum(r["t"] == "get_err" for r in rows), sum(r["t"] == "enum_slow" for r in rows)))
    print("log: %s" % path)


# ---------------------------------------------------------------- offline simulation
class FakeHid:
    """Script: BT 0-2 s; USB plugged at 2 s (BT drops 2.1 s); USB data until 4 s;
    BT back at 4.5 s with USB still listed until 5 s; USER x5 on BT at 6 s.
    Enumeration stalls 1.5 s once at 3 s (must not stall reads)."""

    def __init__(self):
        self.t0 = time.time()
        self.sent = []
        self.stalled = False

    def now(self):
        return time.time() - self.t0

    def enumerate(self, vid, pid):
        t = self.now()
        if 3.0 < t and not self.stalled:
            self.stalled = True
            time.sleep(1.5)
            t = self.now()
        out = []
        if t < 2.1 or t > 4.5:
            out.append({"bus_type": 2, "path": b"bt-path", "interface_number": -1})
        if 2.0 <= t < 5.0:
            out.append({"bus_type": 1, "path": b"usb-mi_02", "interface_number": 2})
        return out

    def device(self):
        hidm = self

        class Dev:
            def open_path(self, p):
                self.tr = "bt" if p == b"bt-path" else "usb"
                self.closed = False

            def get_feature_report(self, rid, n):
                return [rid] + [1] * (n - 1)

            def send_feature_report(self, b):
                hidm.sent.append((round(hidm.now(), 1), self.tr, bytes(b).hex()))

            def read(self, n, to):
                t = hidm.now()
                time.sleep(to / 1000)
                if self.tr == "usb" and not 2.0 <= t < 4.0:
                    return []
                if self.tr == "usb" and 2.0 <= t < 4.0:
                    return [0x05] + [0] * 36
                if self.tr == "bt" and not (t < 2.1 or t > 4.5):
                    raise OSError("bt link gone")
                if self.tr == "bt" and 6.0 <= t < 7.0:   # 5 taps of USER (bit 30)
                    k = int((t - 6.0) / 0.1)
                    on = k % 2 == 0
                    return [0x02, 0, 0, (1 << 6) if on else 0, 0, 0, 0, 0, 0]
                return [0x05] + [0] * 36

            def close(self):
                self.closed = True
        return Dev()


def offline():
    path = log_path("transport_switch_OFFLINE")
    log = JsonLog(path)
    fh = FakeHid()
    tl = run(fh, "now", 12, log)
    log.close()
    summary(path, tl)
    msgs = " | ".join(tl)
    checks = {
        "USB opened": "USB appeared" in msgs,
        "BT gone noticed": "BT disappeared" in msgs,
        "BT back": msgs.count("BT appeared") >= 1 and "USER x5 on BT" in msgs,
        "slow enum noticed": "enumerate took" in msgs,
        "USB init sent": any(tr == "usb" and h == "0a01" for _, tr, h in fh.sent),
        "ended by gesture": "USER x5 on BT" in msgs,
    }
    rows = [json.loads(l) for l in open(path, encoding="utf-8")]
    usb_rx = [r["ts"] - T0 for r in rows if r["t"] == "rx" and r["tr"] == "usb"]
    # reads kept flowing during the 1.5 s enumeration stall (3.0-4.5 s, USB data until 4 s)
    checks["reads not stalled"] = any(3.2 < t < 3.9 for t in usb_rx)
    for k, v in checks.items():
        print("  %-20s %s" % (k, "PASS" if v else "FAIL"))
    return 0 if all(checks.values()) else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--usb-init", choices=("now", "none"))
    ap.add_argument("--cap", type=float, default=240)
    ap.add_argument("--offline", action="store_true")
    a = ap.parse_args()
    if a.offline:
        return offline()
    if not a.usb_init:
        ap.error("--usb-init now|none is required")
    import hid
    path = log_path("transport_switch_%s" % a.usb_init)
    log = JsonLog(path)
    log.write(t="config", usb_init=a.usb_init, cap=a.cap, t0=T0)
    tl = run(hid, a.usb_init, a.cap, log)
    log.close()
    summary(path, tl)
    return 0


if __name__ == "__main__":
    sys.exit(main())

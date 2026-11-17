"""Session 1: panel-only quick checks. USB, Resolve closed. One run, three phases.

Usage: python tests/quick_checks.py [--part all|turns|idle|led] [--cap 900]
       python tests/quick_checks.py --offline

Phase T (turns), each step ended by USER pressed 5x within 3 s:
  K1  Y LIFT: from your mark, FAST CW about 3 turns, then SLOWLY CCW back to the mark
  K2  Y LIFT: from the mark, SLOWLY CW about 3 turns, then FAST CCW back to the mark
  K3  Y LIFT: exactly 3 full turns CW, moderate speed, stop on the mark
  R1  LEFT ring: exactly 2 full turns CW, stop on the mark
  R2  RIGHT ring: exactly 2 full turns CW, stop on the mark
  -> K1/K2: |CCW| / CW should be 1.00 if counts don't depend on speed (spec §9 #6).
     K3: counts per turn (spec §6, 4096 assumed). R1/R2: raw per turn (spec §5, ~4800).
Phase I (idle): starts right after the last USER x5. HANDS OFF for 120 s; every
  non-zero ball/ring/knob delta is logged (spec §5 idle ring jitter). Keep-alive runs.
Phase L (LEDs at timeout, spec §9 #14). Nothing to press, just watch:
   0 s  0a 01, 03 64 64, all 11 coloured LEDs ON (the cue that idle is over); then silence
  ~60 s  the stream times out; the backlight should go off. Do the coloured LEDs?
   72 s  0a 01 only. Does anything come back on?
   84 s  03 64 64 only (backlight 100). Does anything come back on?
   96 s  coloured LEDs OFF (04 zeros). Run ends.
Log: logs/quick_checks_<ts>.jsonl
"""
import argparse
import sys
import time
from collections import defaultdict

from pcommon import JsonLog, TapGate, hexs, log_path

END_BIT = 30                                  # USER
KA = bytes([0x0A, 0x01])
LIGHT = bytes([0x03, 0x64, 0x64])
LEDS_ALL = bytes([0x04, 0xFF, 0x07, 0, 0, 0, 0])   # bits 0-10 (spec §4a)
LEDS_OFF = bytes([0x04, 0, 0, 0, 0, 0, 0])
KNOB_RAW = 360                                # raw per knob count (spec §6)

# (id, slot key, prompt, kind)   slot keys: B<k> = 0x05 slot k, K<k> = 0x06 slot k
STEPS = [
    ("K1", "K0", "Y LIFT: from your mark, FAST CW ~3 turns, then SLOWLY CCW back to the mark", "back"),
    ("K2", "K0", "Y LIFT: from the mark, SLOWLY CW ~3 turns, then FAST CCW back to the mark", "back"),
    ("K3", "K0", "Y LIFT: exactly 3 full turns CW, moderate speed, stop on the mark", "turns3"),
    ("R1", "B2", "LEFT ring: exactly 2 full turns CW, stop on the mark", "turns2"),
    ("R2", "B8", "RIGHT ring: exactly 2 full turns CW, stop on the mark", "turns2"),
]
IDLE_S, IDLE_SKIP = 120.0, 3.0
LED_PLAN = [(0.0, "all on", [KA, LIGHT, LEDS_ALL]), (72.0, "0a 01 only", [KA]),
            (84.0, "03 64 64 only", [LIGHT]), (96.0, "LEDs off, end", [LEDS_OFF])]


def s24(b, i):
    v = b[i] | b[i + 1] << 8 | b[i + 2] << 16
    return v - (1 << 24) if v & 0x800000 else v


def deltas(b):
    """report -> {slot key: raw delta}, non-zero only."""
    out = {}
    if b[0] == 0x05 and len(b) >= 37:
        for k in range(9):
            v = s24(b, 2 + 4 * k)
            if v:
                out["B%d" % k] = v
    elif b[0] == 0x06 and len(b) >= 49:
        for k in range(12):
            v = int.from_bytes(b[1 + 4 * k:5 + 4 * k], "little", signed=True)
            if v:
                out["K%d" % k] = v
    return out


class Acc:
    """Per slot, per sign: sum, report count, first/last time (for a rate)."""

    def __init__(self):
        self.d = defaultdict(lambda: {"sum": 0, "n": 0, "t0": None, "t1": None})

    def add(self, key, v, t):
        a = self.d[(key, "+" if v > 0 else "-")]
        a["sum"] += v
        a["n"] += 1
        a["t0"] = t if a["t0"] is None else a["t0"]
        a["t1"] = t

    def get(self, key, sign):
        return self.d.get((key, sign))

    def rate(self, key, sign):
        a = self.get(key, sign)
        if not a or a["t1"] == a["t0"]:
            return None
        return abs(a["sum"]) / (a["t1"] - a["t0"])

    def others(self, main):
        """other slots with |sum| over 2 % of the main slot's absolute total."""
        tot = sum(abs(a["sum"]) for (k, _), a in self.d.items() if k == main) or 1
        o = defaultdict(int)
        for (k, _), a in self.d.items():
            if k != main:
                o[k] += abs(a["sum"])
        return {k: v for k, v in o.items() if v > 0.02 * tot}


def evaluate(sid, key, kind, acc):
    pos, neg = acc.get(key, "+"), acc.get(key, "-")
    p = pos["sum"] if pos else 0
    n = -neg["sum"] if neg else 0
    unit = KNOB_RAW if key[0] == "K" else 1
    r = dict(t="step_result", step=sid, slot=key, cw_raw=p, ccw_raw=n,
             cw_rate=acc.rate(key, "+"), ccw_rate=acc.rate(key, "-"),
             other_slots=acc.others(key))
    if kind == "back":
        r["ccw_over_cw"] = round(n / p, 4) if p else None
        r["cw_counts"], r["ccw_counts"] = p / unit, n / unit
    else:
        turns = 3 if kind == "turns3" else 2
        r["per_turn"] = (p - n) / unit / turns
        r["unit"] = "counts" if unit > 1 else "raw"
    return r


def run(dev, log, cap=900.0, clock=time.time, say=print, parts=("turns", "idle", "led")):
    t0 = clock()
    nfail = [0]

    def rel():
        return round(clock() - t0, 2)

    def send(pkt, why):
        try:
            if pkt[0] == 0x04:
                rc = dev.write(pkt)
            else:
                rc = dev.send_feature_report(pkt)
            ok = rc is not None and rc >= 0
            err = None if ok else "rc=%r" % rc
        except Exception as e:
            ok, err = False, repr(e)
        if not ok:
            nfail[0] += 1
            say("!! send %s failed: %s" % (hexs(pkt), err))
        log.write(t="send", why=why, data=hexs(pkt), ok=ok, err=err, rel=rel())

    def read():
        d = dev.read(65, 20)
        if not d:
            return None
        b = bytes(d)
        log.write(t="rx", data=hexs(b[:49]), rel=rel())
        return b

    results = []
    send(KA, "init")
    send(LIGHT, "init")
    t_ka = clock()
    gate = TapGate(5, 3.0)
    ended = "done"
    # ---- phase T
    for sid, key, text, kind in (STEPS if "turns" in parts else []):
        say("%7.1f s  step %s: %s   (USER x5 when done)" % (clock() - t0, sid, text))
        log.write(t="step", step=sid, text=text, rel=rel())
        acc = Acc()
        while True:
            if clock() - t0 > cap:
                ended = "cap"
                break
            if clock() - t_ka >= 15:
                send(KA, "keepalive")
                t_ka = clock()
            b = read()
            if not b:
                continue
            t = clock()
            for k, v in deltas(b).items():
                acc.add(k, v, t)
            if gate.feed(b) == END_BIT:
                break
        if ended == "cap":
            break
        r = evaluate(sid, key, kind, acc)
        log.write(rel=rel(), **r)
        results.append(r)
    # ---- phase I
    idle = defaultdict(list)
    if ended != "cap" and "idle" in parts:
        say("%7.1f s  idle: hands off for %d s" % (clock() - t0, IDLE_S))
        log.write(t="phase", phase="idle", rel=rel())
        ti = clock()
        while clock() - ti < IDLE_S:
            if clock() - t_ka >= 15:
                send(KA, "keepalive")
                t_ka = clock()
            b = read()
            if b and clock() - ti >= IDLE_SKIP:
                for k, v in deltas(b).items():
                    idle[k].append(v)
        log.write(t="idle_result", seconds=IDLE_S - IDLE_SKIP,
                  slots={k: v for k, v in idle.items()}, rel=rel())
    # ---- phase L
    led_rx = []
    if ended != "cap" and "led" in parts:
        say("%7.1f s  LED phase: watch the panel" % (clock() - t0))
        log.write(t="phase", phase="led", rel=rel())
        tl = clock()
        for at, what, pkts in LED_PLAN:
            while clock() - tl < at:
                b = read()
                if b:
                    led_rx.append(round(clock() - tl, 2))
            say("%7.1f s  LED phase +%.0f s: %s" % (clock() - t0, at, what))
            for p in pkts:
                send(p, "led:" + what)
        try:
            fl = hexs(bytes(dev.get_feature_report(0x0A, 2)))
        except Exception as e:
            fl = "err %r" % e
        log.write(t="led_result", rx_times=led_rx, flag_at_end=fl, rel=rel())
    else:
        fl = None   # phase skipped or cap hit
    summary = dict(t="result", ended=ended, steps=results,
                   idle={k: v for k, v in idle.items()}, led_rx=led_rx,
                   flag_at_end=fl, sends_failed=nfail[0], end=rel())
    log.write(**summary)
    return summary


def report(s, say=print):
    say("\n== result (ended: %s, %.0f s, failed sends: %d)" % (
        s["ended"], s["end"], s["sends_failed"]))
    for r in s["steps"]:
        rate = lambda x: "-" if x is None else "%.0f/s" % (x / (KNOB_RAW if r["slot"][0] == "K" else 1))
        if "ccw_over_cw" in r:
            say("   %s %s  CW %.0f counts @ %s   CCW %.0f counts @ %s   CCW/CW = %s" % (
                r["step"], r["slot"], r["cw_counts"], rate(r["cw_rate"]), r["ccw_counts"],
                rate(r["ccw_rate"]), r["ccw_over_cw"]))
        else:
            say("   %s %s  %.1f %s per turn  (CW %d, CCW %d raw)" % (
                r["step"], r["slot"], r["per_turn"], r["unit"], r["cw_raw"], r["ccw_raw"]))
        if r["other_slots"]:
            say("      other slots moved: %s" % r["other_slots"])
    say("   idle: %s" % ({k: v for k, v in s["idle"].items()} or "no deltas at all"))
    say("   LED phase: reports received at %s s; 0x0A at end: %s" % (
        s["led_rx"] or "none", s["flag_at_end"]))


# ---------------------------------------------------------------- offline simulation
class FakeDev:
    """Scripted panel: events are (time, report); clock advances 20 ms per empty read.
    Stream (and the LED-phase silence) honour a 60 s timer from the last 0a 01."""

    def __init__(self):
        self.t = 0.0
        self.q = []
        self.sent = []
        self.alive_until = 0.0

    def clock(self):
        return self.t

    def send_feature_report(self, b):
        self.sent.append((round(self.t, 2), bytes(b)))
        if b[0] == 0x0A:
            self.alive_until = self.t + 60
        return len(b)

    def write(self, b):
        self.sent.append((round(self.t, 2), bytes(b)))
        return len(b)

    def get_feature_report(self, rid, n):
        return [0x0A, 1 if self.t < self.alive_until else 0]

    def read(self, n, to):
        if self.q and self.q[0][0] <= self.t:
            return list(self.q.pop(0)[1])
        self.t += to / 1000.0
        return []


def r05(slot, v):
    b = bytearray(37)
    b[0] = 0x05
    b[2 + 4 * slot:5 + 4 * slot] = (v & 0xFFFFFF).to_bytes(3, "little")
    return bytes(b)


def r06(slot, counts):
    b = bytearray(65)
    b[0] = 0x06
    b[1 + 4 * slot:5 + 4 * slot] = (counts * KNOB_RAW).to_bytes(4, "little", signed=True)
    return bytes(b)


def r02(held):
    b = bytearray(9)
    b[0] = 0x02
    if held:
        b[3] |= 1 << 6          # bit 30 = byte 3, bit 6
    return bytes(b)


def script(fd, slip):
    """slip: fraction of counts lost when turning fast (0 = speed independent)."""
    t = 1.0
    ev = []

    def stream(mk, total, step, dt):
        nonlocal t
        sgn = 1 if total > 0 else -1
        left = abs(total)
        while left > 0:
            v = min(step, left)
            ev.append((t, mk(v * sgn)))
            left -= v
            t += dt

    def taps():
        nonlocal t
        t += 0.5
        for _ in range(5):
            ev.append((t, r02(True)))
            ev.append((t + 0.1, r02(False)))
            t += 0.3
        t += 0.5

    knob = lambda v: r06(0, v)
    fast = int(3 * 4096 * (1 - slip))
    # K1: fast CW (may lose counts), slow CCW back exactly 3 turns
    stream(knob, fast, 40, 0.01)
    stream(knob, -3 * 4096, 4, 0.01)
    taps()
    # K2: slow CW 3 turns, fast CCW back
    stream(knob, 3 * 4096, 4, 0.01)
    stream(knob, -fast, 40, 0.01)
    taps()
    # K3: 3 turns at 4081 counts per turn (as hand-measured)
    stream(knob, 3 * 4081, 10, 0.01)
    taps()
    # R1/R2: 2 turns of 4800 raw; R1 with a little ball leak in slot 0
    stream(lambda v: r05(2, v), 9600, 32, 0.006)
    ev.append((t, r05(0, 400)))
    taps()
    stream(lambda v: r05(8, v), 9600, 32, 0.006)
    taps()
    # idle: one ring tick of +16 at 30 s, -16 at 80 s; one inside the skip window
    ev.append((t + 1.0, r05(5, 16)))
    ev.append((t + 30.0, r05(5, 16)))
    ev.append((t + 80.0, r05(2, -16)))
    fd.q = sorted(ev, key=lambda e: e[0])


def offline():
    log = JsonLog(log_path("quick_checks_OFFLINE"))
    ok = True

    def chk(c, msg):
        nonlocal ok
        ok &= bool(c)
        print("  %s  %s" % ("PASS" if c else "FAIL", msg))

    for slip in (0.0, 0.10):
        fd = FakeDev()
        script(fd, slip)
        s = run(fd, log, clock=fd.clock, say=lambda m: None)
        st = {r["step"]: r for r in s["steps"]}
        print("-- sim slip=%.2f" % slip)
        report(s, say=lambda m: print("     " + m))
        exp = round(1 / (1 - slip), 2)
        chk(s["ended"] == "done" and len(st) == 5, "all 5 steps ended by USER x5")
        chk(abs(st["K1"]["ccw_over_cw"] - exp) < 0.01, "K1 CCW/CW = %.2f" % exp)
        chk(abs(st["K2"]["ccw_over_cw"] - (1 - slip)) < 0.01, "K2 CCW/CW = %.2f" % (1 - slip))
        chk(st["K1"]["cw_rate"] > 5 * st["K1"]["ccw_rate"], "K1 fast half really faster")
        chk(abs(st["K3"]["per_turn"] - 4081) < 1, "K3 4081 counts/turn")
        chk(abs(st["R1"]["per_turn"] - 4800) < 1 and abs(st["R2"]["per_turn"] - 4800) < 1,
            "R1/R2 4800 raw/turn")
        chk(st["R1"]["other_slots"] == {"B0": 400} and not st["R2"]["other_slots"],
            "leak into another slot reported")
        chk(s["idle"] == {"B5": [16], "B2": [-16]}, "idle ticks caught, skip window honoured")
        led = [(round(t - fd.sent[0][0], 0), p) for t, p in fd.sent]
        led_start = [t for t, p in fd.sent if p == LEDS_ALL][0]
        after = [(round(t - led_start), hexs(p)) for t, p in fd.sent if t >= led_start]
        chk(after == [(0, "0a 01"), (0, "03 64 64"), (0, "04 ff 07 00 00 00 00"), (72, "0a 01"), (84, "03 64 64"),
                      (96, "04 00 00 00 00 00 00")],
            "LED phase sends only the plan, no keep-alive: %s" % after)
        ka_before = [t for t, p in fd.sent if p == KA and t < led_start]
        gaps = [b - a for a, b in zip(ka_before, ka_before[1:])]
        chk(max(gaps) <= 15.1, "keep-alive every <=15 s before the LED phase")
        chk(s["flag_at_end"] == "0a 01" and s["sends_failed"] == 0, "flag 01 at end, 0 failed")
    fd = FakeDev()
    s = run(fd, log, clock=fd.clock, say=lambda m: None, parts=("led",))
    sent = [(round(t), hexs(p)) for t, p in fd.sent]
    chk(sent == [(0, "0a 01"), (0, "03 64 64"), (0, "0a 01"), (0, "03 64 64"),
                 (0, "04 ff 07 00 00 00 00"), (72, "0a 01"), (84, "03 64 64"),
                 (96, "04 00 00 00 00 00 00")] and not s["steps"],
        "--part led: init, then exactly the LED plan: %s" % sent)
    log.close()
    print("offline:", "ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--cap", type=float, default=900)
    ap.add_argument("--offline", action="store_true")
    ap.add_argument("--part", choices=("all", "turns", "idle", "led"), default="all")
    a = ap.parse_args()
    if a.offline:
        return offline()
    import hid
    from pcommon import find_panel
    parts = ("turns", "idle", "led") if a.part == "all" else (a.part,)
    path = log_path("quick_checks" + ("" if a.part == "all" else "_" + a.part))
    log = JsonLog(path)
    dev = hid.device()
    dev.open_path(find_panel("usb")["path"])
    t0 = time.time()
    try:
        s = run(dev, log, cap=a.cap, say=lambda m: print(m, flush=True), parts=parts)
    finally:
        dev.close()
        log.close()
    report(s)
    print("log: %s" % path)
    return 0


if __name__ == "__main__":
    sys.exit(main())

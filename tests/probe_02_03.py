"""Session 6: what do output 0x02 and byte 1 of feature 03 do? (spec §9 #2, #3). USB.

Usage: python tests/probe_02_03.py            # Resolve closed; ~80 s, just watch
       python tests/probe_02_03.py --offline

Timeline (s). A "flash" = all 11 coloured LEDs on for 1 s, then off: the cue for a new part.
   0      init (0a 01, 03 64 64), flash, then backlight 0 (03 00 00): the panel goes dark
   2-26   PART 1A  output 0x02, one byte at a time = ff for 2 s, then all zero for 1 s
                   (bytes 1..8 = the same bit positions as the button bits 8..71)
  26-45   PART 1B  output 0x02, a single bit walking 0..63, 0.3 s each
          -> anything at all lighting or changing on the dark panel?
  46      flash
  47-61   PART 2A  backlight byte 2 = 0; byte 1 = 0, 1, 10, 25, 50, 75, 100 (2 s each)
          -> any light on the dark panel?
  61-77   PART 2B  backlight byte 2 = 50 (half) for 2 s; then byte 1 swept as above
          -> does the half brightness change at all?
  77      restore: 03 64 64, output 0x02 zero, LEDs off; end
Values stay within Resolve's own range (0-100) for feature 03. Feature 0x07 is never
written. 0a 01 is resent every 15 s. Every input report is counted (does 0x02 output
provoke any?). Log: logs/probe_02_03_<ts>.jsonl
"""
import argparse
import sys
import time
from collections import Counter

from pcommon import JsonLog, hexs, log_path

KA = bytes([0x0A, 0x01])
ALL_LEDS = bytes([0x04, 0xFF, 0x07, 0, 0, 0, 0])
NO_LEDS = bytes([0x04]) + bytes(6)
OUT02_ZERO = bytes([0x02]) + bytes(8)
SWEEP = (0, 1, 10, 25, 50, 75, 100)


def plan():
    """[(t, packet, kind, label)] kind: 'f' feature, 'o' output."""
    p = [(0.0, KA, "f", "init"), (0.0, bytes([3, 100, 100]), "f", "init"),
         (0.0, ALL_LEDS, "o", "flash"), (1.0, NO_LEDS, "o", "flash end"),
         (1.0, bytes([3, 0, 0]), "f", "dark")]
    t = 2.0
    for k in range(8):                                   # PART 1A
        b = bytearray(OUT02_ZERO)
        b[1 + k] = 0xFF
        p.append((t, bytes(b), "o", "1A byte %d = ff" % (k + 1)))
        p.append((t + 2.0, OUT02_ZERO, "o", "1A zero"))
        t += 3.0
    for bit in range(64):                                # PART 1B
        b = bytearray(OUT02_ZERO)
        b[1 + bit // 8] = 1 << bit % 8
        p.append((round(t, 2), bytes(b), "o", "1B bit %d" % bit))
        t += 0.3
    p.append((round(t, 2), OUT02_ZERO, "o", "1B end"))
    p.append((46.0, ALL_LEDS, "o", "flash"))
    p.append((47.0, NO_LEDS, "o", "flash end"))
    t = 47.0
    for a in SWEEP:                                      # PART 2A
        p.append((t, bytes([3, a, 0]), "f", "2A byte1=%d" % a))
        t += 2.0
    p.append((61.0, bytes([3, 50, 50]), "f", "2B baseline 50"))
    t = 63.0
    for a in SWEEP:                                      # PART 2B
        p.append((t, bytes([3, a, 50]), "f", "2B byte1=%d" % a))
        t += 2.0
    p += [(77.0, bytes([3, 100, 100]), "f", "restore"), (77.0, OUT02_ZERO, "o", "restore"),
          (77.0, NO_LEDS, "o", "restore")]
    return sorted(p, key=lambda x: x[0])


def run(dev, log, clock=time.time, say=print):
    t0 = clock()
    rel = lambda: round(clock() - t0, 2)
    rx = Counter()
    rx_by_part = Counter()
    fails = 0
    t_ka = clock()
    part = "init"
    for at, pkt, kind, label in plan():
        while clock() - t0 < at:
            if clock() - t_ka >= 15:
                dev.send_feature_report(KA)
                log.write(t="send", data=hexs(KA), why="keepalive", rel=rel())
                t_ka = clock()
            d = dev.read(65, 10)
            if d:
                b = bytes(d)
                rx[b[0]] += 1
                rx_by_part[part] += 1
                log.write(t="rx", rid=b[0], data=hexs(b[:16]), rel=rel(), part=part)
        part = label.split()[0]
        try:
            rc = dev.send_feature_report(pkt) if kind == "f" else dev.write(pkt)
            ok = rc is not None and rc >= 0
        except Exception as e:
            ok, rc = False, repr(e)
        if not ok:
            fails += 1
            say("!! %s failed: %s (%r)" % (label, hexs(pkt), rc))
        if pkt[0] == KA[0]:
            t_ka = clock()
        log.write(t="send", data=hexs(pkt), why=label, ok=ok, rel=rel())
        if label in ("flash", "dark", "restore") or label.endswith("byte 1 = ff") or label.startswith("2B baseline"):
            say("%6.1f s  %s" % (clock() - t0, label))
    res = dict(t="result", failed=fails, rx=dict(rx), rx_by_part=dict(rx_by_part), end=rel())
    log.write(**res)
    return res


class FakeDev:
    def __init__(self):
        self.t, self.sent = 0.0, []

    def clock(self):
        return self.t

    def send_feature_report(self, b):
        self.sent.append((round(self.t, 2), "f", bytes(b)))
        return len(b)

    def write(self, b):
        self.sent.append((round(self.t, 2), "o", bytes(b)))
        return len(b)

    def read(self, n, to):
        self.t += to / 1000.0
        return []


def offline():
    log = JsonLog(log_path("probe_02_03_OFFLINE"))
    fd = FakeDev()
    r = run(fd, log, clock=fd.clock, say=lambda m: None)
    log.close()
    ok = True

    def chk(c, m):
        nonlocal ok
        ok &= bool(c)
        print("  %s  %s" % ("PASS" if c else "FAIL", m))
    feats = [p for _, k, p in fd.sent if k == "f"]
    outs = [p for _, k, p in fd.sent if k == "o"]
    chk(all(p[0] in (0x0A, 0x03) for p in feats), "features: only 0a and 03")
    chk(all(p[0] != 0x07 for p in feats), "0x07 never written")
    chk(all(p[0] == 3 and len(p) == 3 and max(p[1:]) <= 100 for p in feats if p[0] == 3),
        "03 values within 0-100")
    chk(all(len(p) == 9 for p in outs if p[0] == 2) and all(len(p) == 7 for p in outs if p[0] == 4),
        "output lengths 02=9, 04=7")
    ones = [p for p in outs if p[0] == 2 and bin(int.from_bytes(p[1:], "little")).count("1") == 1]
    chk(len(ones) == 64 and len({p for p in ones}) == 64, "64 distinct single-bit 0x02 patterns")
    ka = [t for t, k, p in fd.sent if p == KA]
    chk(max(b - a for a, b in zip(ka, ka[1:])) <= 15.1 and fd.sent[-1][0] >= 77, "keep-alive <= 15 s, ends at 77 s")
    last = {p[0]: p for _, _, p in fd.sent}
    chk(last[3] == bytes([3, 100, 100]) and last[2] == OUT02_ZERO and last[4] == NO_LEDS,
        "restored at end")
    chk(r["failed"] == 0, "no failed sends")
    print("offline:", "ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--offline", action="store_true")
    a = ap.parse_args()
    if a.offline:
        return offline()
    import hid
    from pcommon import find_panel
    path = log_path("probe_02_03")
    log = JsonLog(path)
    dev = hid.device()
    dev.open_path(find_panel("usb")["path"])
    try:
        r = run(dev, log, say=lambda m: print(m, flush=True))
    finally:
        dev.close()
        log.close()
    print("\n== result: failed sends %d, input reports %s, by part %s" % (
        r["failed"], r["rx"] or "none", r["rx_by_part"] or "none"))
    print("log: %s" % path)
    return 0


if __name__ == "__main__":
    sys.exit(main())

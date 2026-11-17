"""Task 2: capture free movement and check raw-format assumptions.

Usage:  python tests/raw_check.py [--secs 180]
Capture starts immediately and ends when the SAME button is pressed 5 times
within 3 s (or after --secs, a safety cap).

Checks:
  * report IDs and lengths seen (flags anything not in the descriptor)
  * per-ID byte map: which byte offsets were ever nonzero (padding must stay 0)
  * 0x06 knobs: 16 x s32 at 1+4k -> nonzero slots, divisibility by 360, gcd
  * 0x05 balls/rings: 9 x 4-byte groups at 1+4k. Is the low byte (1+4k) ever
    nonzero? (zero always => 24-bit at 2+4k and 32-bit at 1+4k differ only by x256)
    s24 at 2+4k: divisibility by 16, gcd
  * 0x02 buttons: bits seen
  * 0x08 input: every distinct value
Log: logs/raw_check_<ts>.jsonl (every report, every send)
"""
import argparse
import math
import sys
import time
from collections import Counter, defaultdict

from pcommon import JsonLog, Panel, TapGate, hexs, log_path

# payload bytes per ID from the descriptor (dump_caps, verified); +1 for the ID byte
PAYLOAD = {0x02: 8, 0x05: 36, 0x06: 64, 0x08: 2}


def s24(b, i):
    v = b[i] | b[i + 1] << 8 | b[i + 2] << 16
    return v - (1 << 24) if v & 0x800000 else v


def s32(b, i):
    return int.from_bytes(b[i:i + 4], "little", signed=True)


def gcd_all(vals):
    g = 0
    for v in vals:
        g = math.gcd(g, abs(v))
    return g


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--secs", type=float, default=180)
    a = ap.parse_args()

    path = log_path("raw_check")
    log = JsonLog(path)
    p = Panel(log=log)
    gate = TapGate(5, 3.0)
    end_bit = None
    print(">>> Capturing now. Turn EVERY knob both ways, roll ALL THREE balls in all "
          "directions, turn ALL THREE rings both ways, press a few buttons.\n"
          ">>> To finish: press any ONE button 5 times quickly (cap %d s)." % a.secs,
          flush=True)

    ids = Counter()
    lens = defaultdict(Counter)
    nonzero = defaultdict(Counter)       # rid -> byte offset -> count nonzero
    knob = defaultdict(list)             # slot -> raw values
    ball = defaultdict(list)             # slot -> s24 values
    lowbyte = Counter()                  # slot -> count of nonzero byte 1+4k in 0x05
    lowbyte_vals = defaultdict(Counter)
    bits_seen = Counter()
    r08 = Counter()
    gaps, last_rx = [], None

    t0 = time.time()
    try:
        while time.time() - t0 < a.secs:
            b = p.read(50)
            if b is None:
                continue
            now = time.time()
            if last_rx is not None and now - last_rx > 1.0:
                gaps.append(round(now - last_rx, 2))
            last_rx = now
            rid = b[0]
            ids[rid] += 1
            lens[rid][len(b)] += 1
            log.write(t="rx", rid=rid, len=len(b), data=hexs(b))
            end_bit = gate.feed(b)
            for i, x in enumerate(b[1:], 1):
                if x:
                    nonzero[rid][i] += 1
            if rid == 0x06:
                for k in range(16):
                    v = s32(b, 1 + 4 * k)
                    if v:
                        knob[k].append(v)
            elif rid == 0x05:
                for k in range(9):
                    if b[1 + 4 * k]:
                        lowbyte[k] += 1
                        lowbyte_vals[k][b[1 + 4 * k]] += 1
                    v = s24(b, 2 + 4 * k)
                    if v:
                        ball[k].append(v)
            elif rid == 0x02:
                for i in range(64):
                    if b[1 + i // 8] >> (i % 8) & 1:
                        bits_seen[i + 8] += 1   # bit number = byte_pos*8 + bit
            elif rid == 0x08:
                r08[hexs(b[1:3])] += 1
            if end_bit is not None:
                break
    except KeyboardInterrupt:
        print("    interrupted")
    finally:
        p.close()

    out = []
    pr = out.append
    pr("== raw_check summary (%.0f s, ended by %s)" % (time.time() - t0,
       "5x bit %d" % end_bit if end_bit is not None else "time cap / interrupt"))
    pr("reports by ID: " + ", ".join("0x%02x=%d" % kv for kv in sorted(ids.items())))
    unexpected = [r for r in ids if r not in PAYLOAD]
    pr("UNEXPECTED report IDs: %s" % ([hex(r) for r in unexpected] or "none"))
    for rid in sorted(lens):
        pr("  0x%02x lengths returned: %s" % (rid, dict(lens[rid])))
    pr("byte offsets ever nonzero (offset 0 = ID):")
    for rid in sorted(nonzero):
        offs = sorted(nonzero[rid])
        lim = PAYLOAD.get(rid, 64)
        beyond = [o for o in offs if o > lim]
        pr("  0x%02x: %s" % (rid, _ranges(offs)))
        pr("        beyond payload (>%d): %s" % (lim, beyond or "none"))

    pr("0x06 knobs (s32 @ 1+4k):")
    for k in range(16):
        v = knob.get(k)
        if not v:
            pr("  slot %2d: no data%s" % (k, "   <-- expected 12 knobs" if k < 12 else ""))
            continue
        bad = [x for x in v if x % 360]
        pr("  slot %2d: n=%4d min=%7d max=%7d gcd=%d  not%%360=%d %s"
           % (k, len(v), min(v), max(v), gcd_all(v), len(bad), bad[:5] if bad else ""))
    pr("0x05 balls/rings (4-byte groups @ 1+4k, s24 @ 2+4k):")
    for k in range(9):
        v = ball.get(k)
        lb = "lowbyte nonzero %d%s" % (lowbyte[k], (" vals " + str(dict(lowbyte_vals[k].most_common(5))))
                                      if lowbyte[k] else "")
        if not v:
            pr("  slot %d: no data; %s" % (k, lb))
            continue
        bad = [x for x in v if x % 16]
        pr("  slot %d: n=%4d min=%7d max=%7d gcd=%d not%%16=%d %s; %s"
           % (k, len(v), min(v), max(v), gcd_all(v), len(bad), bad[:5] if bad else "", lb))
    pr("0x02 button bits seen: %s" % (sorted(bits_seen) or "none"))
    pr("0x08 input values: %s" % (dict(r08) or "none"))
    pr("rx gaps > 1 s: %d %s" % (len(gaps), gaps[:10]))
    pr("feature sends ok=%d failed=%d" % (p.sends_ok, p.sends_failed))
    pr("log: %s" % path)
    for line in out:
        print(line)
        log.write(t="summary", line=line)
    log.close()
    return 0


def _ranges(offs):
    if not offs:
        return "none"
    r, s, prev = [], offs[0], offs[0]
    for o in offs[1:] + [None]:
        if o is None or o != prev + 1:
            r.append(str(s) if s == prev else "%d-%d" % (s, prev))
            if o is not None:
                s = o
        if o is not None:
            prev = o
    return ",".join(r)


if __name__ == "__main__":
    sys.exit(main())

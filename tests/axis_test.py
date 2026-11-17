"""Task 4: axis identification, signs, leakage. Run one part at a time.

Usage: python tests/axis_test.py --part balls|rings|knobs [--cap 570]

Every step ends with the USER button pressed 5 times quickly (bit 30).
All 0x05 (9 x s24 @ 2+4k) and 0x06 (16 x s32 @ 1+4k) slots are summed per step;
nothing is assumed about which slot is which.

Log: logs/axis_<part>_<ts>.jsonl (every report, step markers, summary)
"""
import argparse
import statistics
import sys
import time
from collections import defaultdict

from pcommon import JsonLog, Panel, TapGate, button_bits, hexs, log_path

END_BIT = 30  # USER ("reserved for future use" per the manual)

BALLS = ("LEFT", "MIDDLE", "RIGHT")
DIRS = {  # wording used in the on-screen prompts
    "UP": "roll the top of the ball AWAY from you",
    "DOWN": "roll the top of the ball TOWARD you",
    "LEFT": "roll the top of the ball to the LEFT",
    "RIGHT": "roll the top of the ball to the RIGHT",
}
PARTS = {
    "balls": [("ball", b, d, "%s ball: %s, slowly, ~3 s" % (b, DIRS[d]))
              for b in BALLS for d in DIRS],
    "rings": [("ring", b, d, "%s ring: turn %s slowly, ~3 s" % (b, d))
              for b in BALLS for d in ("CW", "CCW")] +
             [("ring", b, "TURN", "%s ring: exactly ONE full turn CW (use a mark)" % b)
              for b in BALLS],
    # speed check: same ring, exactly one full turn each, slow vs fast
    "ringturns": [("ring", "MIDDLE", "TURN_%s%d" % (sp, i),
                   "MIDDLE ring: exactly ONE full turn CW, %s" % txt)
                  for i in (1, 2) for sp, txt in (("SLOW", "SLOWLY (~4 s)"),
                                                  ("FAST", "FAST (~1 s)"))],
    "knobs": [("knobs", "ALL", "ORDER", "each knob in turn, LEFT -> RIGHT (Y LIFT ... LUM MIX): "
               "turn it CW a little, let go, then the next"),
              ("knob", "Y_LIFT", "TURN", "Y LIFT knob: exactly ONE full turn CW (use a mark)")],
}


def s24(b, i):
    v = b[i] | b[i + 1] << 8 | b[i + 2] << 16
    return v - (1 << 24) if v & 0x800000 else v


def s32(b, i):
    return int.from_bytes(b[i:i + 4], "little", signed=True)


class Step:
    def __init__(self, kind, dev, d, text):
        self.kind, self.dev, self.dir, self.text = kind, dev, d, text
        self.vals = defaultdict(list)    # "B5" / "K6" + slot -> [(ts, raw)]

    def add(self, key, ts, v):
        self.vals[key].append((ts, v))

    def sum(self, key):
        return sum(v for _, v in self.vals.get(key, []))

    def abssum(self, key):
        return sum(abs(v) for _, v in self.vals.get(key, []))

    def ranked(self):
        return sorted(self.vals, key=lambda k: -self.abssum(k))


def run_steps(p, log, steps, cap):
    t0 = time.time()
    done = []
    for i, st in enumerate(steps, 1):
        print("\n>>> STEP %d/%d: %s\n    then USER x5." % (i, len(steps), st.text), flush=True)
        log.write(t="step", n=i, kind=st.kind, dev=st.dev, dir=st.dir)
        gate = TapGate(5, 3.0)
        while True:
            if time.time() - t0 > cap:
                print("!! time cap reached")
                return done
            b = p.read(50)
            if not b:
                continue
            ts = time.time()
            rid = b[0]
            if rid == 0x05:
                log.write(t="rx", rid=rid, data=hexs(b[:37]))
                for k in range(9):
                    v = s24(b, 2 + 4 * k)
                    if v:
                        st.add("B%d" % k, ts, v)
            elif rid == 0x06:
                log.write(t="rx", rid=rid, data=hexs(b))
                for k in range(16):
                    v = s32(b, 1 + 4 * k)
                    if v:
                        st.add("K%d" % k, ts, v)
            elif rid == 0x02:
                log.write(t="rx", rid=rid, data=hexs(b[:9]))
                fired = gate.feed(b)
                if fired is not None and fired != END_BIT:
                    print("    (5x bit %d ignored; end key is USER)" % fired)
                elif fired == END_BIT:
                    break
            else:
                log.write(t="rx", rid=rid, data=hexs(b))
        # settle: absorb extra USER taps
        t_q = time.time()
        while time.time() - t_q < 1.5:
            b = p.read(50)
            if b and b[0] == 0x02 and END_BIT in button_bits(b):
                t_q = time.time()
        moved = ", ".join("%s=%+d" % (k, st.sum(k)) for k in st.ranked()[:4]) or "nothing"
        print("    step done: %s" % moved, flush=True)
        done.append(st)
    return done


def pct(a, b):
    return 100.0 * abs(a) / abs(b) if b else float("nan")


def analyse_balls(steps, out):
    by = {(s.dev, s.dir): s for s in steps}
    for ball in BALLS:
        out.append("-- %s ball" % ball)
        axes = {}
        for axis, (pos, neg) in (("Y", ("UP", "DOWN")), ("X", ("RIGHT", "LEFT"))):
            sp, sn = by.get((ball, pos)), by.get((ball, neg))
            if not sp or not sn:
                out.append("   %s: missing steps" % axis)
                continue
            kp, kn = (sp.ranked() or [None])[0], (sn.ranked() or [None])[0]
            if kp is None or kn is None:
                out.append("   %s: AMBIGUOUS, no movement in %s or %s; retest" % (axis, pos, neg))
                continue
            vp, vn = sp.sum(kp), sn.sum(kn)
            if kp != kn:
                out.append("   %s: AMBIGUOUS, %s moved %s but %s moved %s; retest"
                           % (axis, pos, kp, neg, kn))
                continue
            if (vp > 0) == (vn > 0):
                out.append("   %s: AMBIGUOUS, %s and %s have the same sign (%+d, %+d); retest"
                           % (axis, pos, neg, vp, vn))
                continue
            axes[axis] = kp
            out.append("   %s = slot %s: %s sum %+d, %s sum %+d  ->  %s is %s"
                       % (axis, kp, pos, vp, neg, vn, pos, "POSITIVE" if vp > 0 else "NEGATIVE"))
        if len(axes) == 2 and axes["X"] == axes["Y"]:
            out.append("   AMBIGUOUS: X and Y resolved to the same slot; retest")
            continue
        # leakage: off-axis slot during on-axis steps
        for axis, other in (("Y", "X"), ("X", "Y")):
            if axis not in axes or other not in axes:
                continue
            main, off = axes[axis], axes[other]
            dirs = ("UP", "DOWN") if axis == "Y" else ("LEFT", "RIGHT")
            for d in dirs:
                s = by[(ball, d)]
                offv = [abs(v) for _, v in s.vals.get(off, [])]
                mainv = [abs(v) for _, v in s.vals.get(main, [])]
                p95 = sorted(offv)[int(0.95 * (len(offv) - 1))] if offv else 0
                out.append("   leak %-5s into %s: net %.1f%%, abs %.1f%% of main; off-axis "
                           "per-report p95=%d raw (main median %d)"
                           % (d, other, pct(s.sum(off), s.sum(main)),
                              pct(s.abssum(off), s.abssum(main)), p95,
                              statistics.median(mainv) if mainv else 0))
        # anything else that moved
        mine = set(axes.values())
        for d in DIRS:
            s = by.get((ball, d))
            if not s:
                continue
            ref = max((s.abssum(k) for k in mine), default=0)
            for k in s.ranked():
                if k not in mine and ref and s.abssum(k) > 0.05 * ref:
                    out.append("   NOTE %s step: %s also moved (abs %d = %.0f%% of main)"
                               % (d, k, s.abssum(k), pct(s.abssum(k), ref)))


def analyse_rings(steps, out):
    by = {(s.dev, s.dir): s for s in steps}
    for ring in BALLS:
        cw, ccw, turn = by.get((ring, "CW")), by.get((ring, "CCW")), by.get((ring, "TURN"))
        out.append("-- %s ring" % ring)
        if not cw or not ccw:
            out.append("   missing steps")
            continue
        kc, kcc = (cw.ranked() or [None])[0], (ccw.ranked() or [None])[0]
        if kc is None or kc != kcc or (cw.sum(kc) > 0) == (ccw.sum(kcc) > 0):
            out.append("   AMBIGUOUS: CW top %s %+d, CCW top %s %+d; retest"
                       % (kc, cw.sum(kc) if kc else 0, kcc, ccw.sum(kcc) if kcc else 0))
            continue
        out.append("   slot %s: CW sum %+d, CCW sum %+d  ->  CW is %s"
                   % (kc, cw.sum(kc), ccw.sum(kc), "POSITIVE" if cw.sum(kc) > 0 else "NEGATIVE"))
        for name, s in (("CW", cw), ("CCW", ccw)):
            for k in s.ranked()[1:]:
                if s.abssum(k) > 0.05 * s.abssum(kc):
                    out.append("   NOTE %s: %s also moved (abs %d = %.0f%%)"
                               % (name, k, s.abssum(k), pct(s.abssum(k), s.abssum(kc))))
        if turn:
            out.append("   one full turn CW: slot %s sum %+d raw (= %+d per quarter turn, "
                       "hand-measured, approx.)" % (kc, turn.sum(kc), turn.sum(kc) // 4))


def analyse_ringturns(steps, out):
    rows = []
    for s in steps:
        v = s.vals.get("B5", [])
        if not v:
            out.append("   %s: no B5 data" % s.dir)
            continue
        dur = (v[-1][0] - v[0][0]) or 1e-9
        rows.append((s.dir, s.sum("B5"), dur))
        out.append("   %-12s sum %+6d  over %.2f s" % (s.dir, s.sum("B5"), dur))
        for k in s.ranked():
            if k != "B5" and s.abssum(k) > 0.05 * s.abssum("B5"):
                out.append("      NOTE: %s also moved (abs %d)" % (k, s.abssum(k)))
    slow = [r[1] for r in rows if "SLOW" in r[0]]
    fast = [r[1] for r in rows if "FAST" in r[0]]
    if slow and fast:
        ms, mf = sum(slow) / len(slow), sum(fast) / len(fast)
        out.append("   mean slow %+.0f, mean fast %+.0f, fast/slow = %.2f" % (ms, mf, mf / ms))
        spread = max(slow + fast) / min(slow + fast)
        out.append("   max/min over all 4 = %.2f  (hand error should be < ~1.15)" % spread)


def analyse_knobs(steps, out):
    order = next((s for s in steps if s.dir == "ORDER"), None)
    if order:
        ks = [k for k in order.vals if k.startswith("K")]
        tm = {k: statistics.median(t for t, _ in order.vals[k]) for k in ks}
        seq = sorted(ks, key=lambda k: tm[k])
        out.append("-- knob order (by median time of movement):")
        out.append("   " + " ".join(seq))
        exp = ["K%d" % i for i in range(12)]
        out.append("   matches K0..K11 left -> right: %s" % ("YES" if seq == exp else "NO, check above"))
        neg = [k for k in seq if order.sum(k) < 0]
        out.append("   CW sign: %s" % ("all POSITIVE" if not neg else
                                       "NEGATIVE or mixed in %s" % neg))
        for k in seq:
            out.append("   %s sum %+d over %d reports" % (k, order.sum(k), len(order.vals[k])))
        others = [k for k in order.vals if not k.startswith("K")]
        if others:
            out.append("   NOTE: ball/ring slots moved during knob step: %s" % others)
    turn = next((s for s in steps if s.dir == "TURN"), None)
    if turn and turn.ranked():
        k = turn.ranked()[0]
        out.append("-- Y LIFT one full turn CW: top slot %s sum %+d raw (= %+.1f units of 360)"
                   % (k, turn.sum(k), turn.sum(k) / 360))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--part", required=True, choices=sorted(PARTS))
    ap.add_argument("--cap", type=float, default=570)
    a = ap.parse_args()

    path = log_path("axis_%s" % a.part)
    log = JsonLog(path)
    p = Panel(log=log)
    steps = [Step(*x) for x in PARTS[a.part]]
    try:
        done = run_steps(p, log, steps, a.cap)
    except KeyboardInterrupt:
        print("!! interrupted")
        done = [s for s in steps if s.vals]
    finally:
        p.close()

    out = ["", "== axis_test %s summary (%d/%d steps)" % (a.part, len(done), len(steps))]
    {"balls": analyse_balls, "rings": analyse_rings, "ringturns": analyse_ringturns,
     "knobs": analyse_knobs}[a.part](done, out)
    out.append("feature sends ok=%d failed=%d" % (p.sends_ok, p.sends_failed))
    out.append("log: %s" % path)
    for line in out:
        print(line)
        log.write(t="summary", line=line)
    log.close()


if __name__ == "__main__":
    sys.exit(main())

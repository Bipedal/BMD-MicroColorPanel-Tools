"""Task 5: state probes and stuck-button reproduction.

Usage: python tests/state_probe.py [--slow-ms 30] [--phases A,B,C1,C2]

Verified 2026-11-16: the panel rejects GET_REPORT (Win32 error 31) for every ID except
input 0x06, so the get_input/get_feature probes are expected to fail.

Phases
  A  (hands off, automatic)
     A1 get_feature_report on 0x03 0x07 0x08 0x0a, get_input_report on 0x02 0x05 0x06 0x08
     A2 send 0a 01 / 03 64 64 with nothing held: does any report come back?
  B  (hold H/LITE ~6 s, then release)
     while held: get_input_report(0x02), send 0a 01, send 03 64 64 -> does the held
     bit show up? after release: same probes, bit should be gone.
  C1 (normal fast reading) spin knobs/rings fast + tap buttons; ends after 3 s of
     silence (hands off). USER x5 then starts C2.
  C2 (deliberately slow reading) same motions, same ending; the script reads only
     every --slow-ms to overflow the OS input buffer, then checks for a stuck bit and
     whether get_input_report / init recover the true state.

Never asks for AUTO COLOR + STOP (factory reset). Sends NO output (LED) reports.
Log: logs/state_probe_<ts>.jsonl
"""
import argparse
import sys
import time

from pcommon import INIT, LIGHT, JsonLog, Panel, TapGate, button_bits, hexs, log_path
from panel_map import BUTTONS

HOLD_BIT = 37   # H/LITE
END_BIT = 30    # USER
FEATURE_IDS = (0x03, 0x07, 0x08, 0x0A)
INPUT_IDS = (0x02, 0x05, 0x06, 0x08)


class Probe:
    def __init__(self, p, log):
        self.p, self.log = p, log
        self.read_fail = 0
        self.out = []

    def say(self, s):
        print(s, flush=True)
        self.out.append(s)

    # ---- low level
    def get_feature(self, rid, n=33):
        try:
            d = self.p.dev.get_feature_report(rid, n)
            r = bytes(d)
            self.log.write(t="get_feature", rid=rid, n=n, data=hexs(r))
            return r, None
        except Exception as e:
            self.read_fail += 1
            self.log.write(t="get_feature", rid=rid, n=n, err=repr(e))
            return None, repr(e)

    def get_input(self, rid, n=65):
        try:
            d = self.p.dev.get_input_report(rid, n)
            r = bytes(d)
            self.log.write(t="get_input", rid=rid, n=n, data=hexs(r))
            return r, None
        except Exception as e:
            self.read_fail += 1
            self.log.write(t="get_input", rid=rid, n=n, err=repr(e))
            return None, repr(e)

    def listen(self, ms):
        """Collect all reports for ms milliseconds (no keep-alive sends)."""
        got, end = [], time.time() + ms / 1000
        ka, self.p.keepalive = self.p.keepalive, False
        try:
            while time.time() < end:
                b = self.p.read(10)
                if b:
                    got.append(b)
                    self.log.write(t="rx", rid=b[0], data=hexs(b))
        finally:
            self.p.keepalive = ka
        return got

    def send_and_listen(self, pkt, ms=500):
        self.listen(100)  # drain
        ok = self.p.send(pkt, "probe")
        got = self.listen(ms)
        r02 = [b for b in got if b[0] == 0x02]
        return ok, got, r02


def fmt_bits(bits):
    return "{%s}" % ", ".join("%d %s" % (b, BUTTONS.get(b, "?")) for b in sorted(bits)) if bits else "{}"


def phase_a(pr):
    pr.say("\n== A1 feature/input report reads (nothing held)")
    for rid in FEATURE_IDS:
        r, e = pr.get_feature(rid)
        pr.say("   get_feature 0x%02x: %s" % (rid, e or ("%d bytes: %s" % (len(r), hexs(r)))))
    for rid in INPUT_IDS:
        r, e = pr.get_input(rid)
        if e:
            pr.say("   get_input   0x%02x: %s" % (rid, e))
        else:
            nz = [i for i, x in enumerate(r[1:], 1) if x]
            pr.say("   get_input   0x%02x: %d bytes, id byte 0x%02x, nonzero offsets %s"
                   % (rid, len(r), r[0], nz or "none"))
    pr.say("\n== A2 init/light with nothing held (3 tries each, 500 ms listen)")
    for name, pkt in (("0a 01", INIT), ("03 64 64", LIGHT)):
        for i in range(3):
            ok, got, r02 = pr.send_and_listen(pkt)
            pr.say("   send %-8s ok=%s -> %d reports %s; 0x02: %s"
                   % (name, ok, len(got), sorted({b[0] for b in got}),
                      [fmt_bits(button_bits(b)) for b in r02] or "none"))


def phase_b(pr, cap=60):
    pr.say("\n== B hold H/LITE")
    print(">>> Press and HOLD H/LITE for ~6 s, then release.", flush=True)
    held, t0 = set(), time.time()
    while HOLD_BIT not in held:
        if time.time() - t0 > cap:
            pr.say("   timeout waiting for H/LITE hold")
            return
        b = pr.p.read(50)
        if b and b[0] == 0x02:
            held = button_bits(b)
            pr.log.write(t="rx", rid=2, data=hexs(b[:9]))
    pr.say("   stream shows held: %s" % fmt_bits(held))
    time.sleep(0.5)
    results = []
    for label in ("while held", "after release"):
        if label == "after release":
            while HOLD_BIT in held:
                if time.time() - t0 > cap:
                    pr.say("   timeout waiting for release")
                    return
                b = pr.p.read(50)
                if b and b[0] == 0x02:
                    held = button_bits(b)
            time.sleep(0.5)
            pr.say("   released; stream shows held: %s" % fmt_bits(held))
        r, e = pr.get_input(0x02)
        gi = e or fmt_bits(button_bits(r))
        pr.say("   [%s] get_input_report(0x02): %s" % (label, gi))
        for name, pkt in (("0a 01", INIT), ("03 64 64", LIGHT)):
            ok, got, r02 = pr.send_and_listen(pkt)
            pr.say("   [%s] send %-8s -> 0x02 reports: %s"
                   % (label, name, [fmt_bits(button_bits(b)) for b in r02] or "none"))
            results.append((label, name, r02))


def phase_c(pr, slow_ms, quiet=3.0, min_secs=10, cap=180):
    """Runs until the panel has been silent for `quiet` s (after at least min_secs),
    so the stuck check happens with hands off. slow_ms>0 sleeps between reads to
    overflow the OS input buffer."""
    mode = ("C2 slow reader (%d ms between reads)" % slow_ms) if slow_ms else "C1 normal reader"
    pr.say("\n== %s" % mode)
    print(">>> Spin knobs and rings FAST with one hand while tapping buttons with the "
          "other (never AUTO COLOR or STOP), then hands OFF.", flush=True)
    held, presses, releases, n = set(), 0, 0, {}
    t0 = last = time.time()
    while True:
        now_t = time.time()
        if now_t - t0 > cap:
            pr.say("   time cap")
            break
        if now_t - t0 > min_secs and now_t - last > quiet:
            break
        if slow_ms:
            time.sleep(slow_ms / 1000)
        b = pr.p.read(1 if slow_ms else 50)
        if not b:
            continue
        last = time.time()
        n[b[0]] = n.get(b[0], 0) + 1
        pr.log.write(t="rx", rid=b[0], data=hexs(b[:9] if b[0] == 2 else b))
        if b[0] == 0x02:
            now = button_bits(b)
            presses += len(now - held)
            releases += len(held - now)
            held = now
    pr.say("   ran %.0f s; reports read: %s" % (time.time() - t0,
           {("0x%02x" % k): v for k, v in sorted(n.items())}))
    pr.say("   press edges %d, release edges %d" % (presses, releases))
    pr.say("   STUCK per report stream (hands off): %s" % fmt_bits(held))
    r, e = pr.get_input(0x02)
    pr.say("   get_input_report(0x02): %s" % (e or fmt_bits(button_bits(r))))
    ok, got, r02 = pr.send_and_listen(INIT)
    pr.say("   send 0a 01 -> 0x02 reports: %s" % ([fmt_bits(button_bits(b)) for b in r02] or "none"))
    return held


def wait_user5(pr, msg, cap=120):
    print(">>> " + msg, flush=True)
    gate, t0 = TapGate(5, 3.0), time.time()
    while time.time() - t0 < cap:
        b = pr.p.read(50)
        if b and gate.feed(b) == END_BIT:
            t_q = time.time()          # settle: absorb extra taps
            while time.time() - t_q < 1.5:
                b = pr.p.read(50)
                if b and b[0] == 0x02 and END_BIT in button_bits(b):
                    t_q = time.time()
            return True
    pr.say("   timeout waiting for USER x5")
    return False


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--slow-ms", type=int, default=30)
    ap.add_argument("--phases", default="A,B,C1,C2")
    a = ap.parse_args()
    phases = a.phases.upper().split(",")

    path = log_path("state_probe")
    log = JsonLog(path)
    p = Panel(log=log)
    pr = Probe(p, log)
    try:
        if "A" in phases:
            print(">>> Phase A: hands off the panel for ~10 s.", flush=True)
            phase_a(pr)
        if "B" in phases:
            phase_b(pr)
        if "C1" in phases:
            phase_c(pr, 0)
        if "C2" in phases and wait_user5(pr, "Press USER x5 to start C2."):
            phase_c(pr, a.slow_ms)
    except KeyboardInterrupt:
        pr.say("!! interrupted")
    finally:
        p.close()
    pr.say("\nfeature sends ok=%d failed=%d; report reads failed=%d"
           % (p.sends_ok, p.sends_failed, pr.read_fail))
    pr.say("log: %s" % path)
    for line in pr.out:
        log.write(t="summary", line=line)
    log.close()


if __name__ == "__main__":
    sys.exit(main())

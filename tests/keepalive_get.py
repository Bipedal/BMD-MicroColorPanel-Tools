"""K2: does a GET of feature 0x0A count as a keep-alive? USB only.

Usage: python tests/keepalive_get.py --mode get|none [--cap 100]
       python tests/keepalive_get.py --offline

  * opens USB, sends 0a 01 ONCE (+ 03 64 64 brightness), never again
  * --mode get : GET feature 0x0A every 5 s
    --mode none: no GETs until the end (control; repeats keepalive_test)
  * you roll a ball gently and continuously; the script records when reports stop
  * ends at the time cap, or USER x5 if the stream is still alive
  * at the end: GET 0x0A once, to see the flag after the stream died
Background: B4b variant 2 kept streaming 66 s after the last 0a 01 while 0x0A was
read every 5 s; keepalive_test (no GETs) died ~60 s after an init.
Log: logs/keepalive_get_<mode>_<ts>.jsonl
"""
import argparse
import json
import sys
import time

from pcommon import JsonLog, TapGate, hexs, log_path


def run(dev, mode, cap, log, clock=time.time):
    t0 = clock()
    gate = TapGate(5, 3.0)
    dev.send_feature_report(bytes([0x0A, 0x01]))
    dev.send_feature_report(bytes([0x03, 0x64, 0x64]))
    log.write(t="send", data="0a 01 + 03 64 64", rel=0.0)
    print("   0.0 s  0a 01 sent (once). roll continuously now", flush=True)
    t_get, last_rx, n, gaps, ended = clock(), None, 0, [], "cap"
    while clock() - t0 < cap:
        now = clock()
        if mode == "get" and now - t_get >= 5:
            t_get = now
            try:
                r = bytes(dev.get_feature_report(0x0A, 2))
                log.write(t="get", rid=10, data=hexs(r), rel=round(now - t0, 2))
            except Exception as e:
                log.write(t="get_err", err=repr(e), rel=round(now - t0, 2))
                print("  %5.1f s  !! GET 0x0a failed: %r" % (now - t0, e), flush=True)
        d = dev.read(65, 20)
        if not d:
            continue
        b = bytes(d)
        now = clock()
        if last_rx is not None and now - last_rx > 3:
            gaps.append((round(last_rx - t0, 1), round(now - t0, 1)))
            print("  %5.1f s  stream resumed after %.1f s silence" % (now - t0, now - last_rx),
                  flush=True)
        last_rx = now
        n += 1
        log.write(t="rx", rid=b[0], rel=round(now - t0, 3))
        if n % 200 == 0:
            print("  %5.1f s  %d reports" % (now - t0, n), flush=True)
        if gate.feed(b) == 30:
            ended = "USER x5"
            break
    end = clock() - t0
    try:
        fl = hexs(bytes(dev.get_feature_report(0x0A, 2)))
    except Exception as e:
        fl = "err %r" % e
    last = None if last_rx is None else round(last_rx - t0, 1)
    res = dict(t="result", mode=mode, ended=ended, end=round(end, 1), reports=n,
               last_rx=last, gaps=gaps, flag_at_end=fl,
               silent_at_end=None if last is None else round(end - last, 1))
    log.write(**res)
    return res


class FakeDev:
    """Stream dies 60 s after the last 0a 01, or after the last GET if gets_count."""

    def __init__(self, gets_count):
        self.t = 0.0
        self.alive_until = 0
        self.gets_count = gets_count

    def clock(self):
        return self.t

    def send_feature_report(self, b):
        if b[0] == 0x0A:
            self.alive_until = self.t + 60

    def get_feature_report(self, rid, n):
        if self.gets_count:
            self.alive_until = max(self.alive_until, self.t + 60)
        return [0x0A, 1 if self.t < self.alive_until else 0]

    def read(self, n, to):
        self.t += 0.05
        return [0x05] + [0] * 36 if self.t < self.alive_until else []


def offline():
    log = JsonLog(log_path("keepalive_get_OFFLINE"))
    ok = True
    for mode, counts, expect in (("none", False, "dies"), ("get", False, "dies"),
                                 ("get", True, "lives")):
        fd = FakeDev(counts)
        r = run(fd, mode, 100, log, clock=fd.clock)
        got = "dies" if r["silent_at_end"] > 10 else "lives"
        print("  sim mode=%s gets_count=%s -> last_rx %s, %s (expect %s) %s" % (
            mode, counts, r["last_rx"], got, expect, "PASS" if got == expect else "FAIL"))
        ok &= got == expect
    log.close()
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=("get", "none"))
    ap.add_argument("--cap", type=float, default=100)
    ap.add_argument("--offline", action="store_true")
    a = ap.parse_args()
    if a.offline:
        return offline()
    if not a.mode:
        ap.error("--mode get|none is required")
    import hid
    from pcommon import find_panel
    path = log_path("keepalive_get_%s" % a.mode)
    log = JsonLog(path)
    dev = hid.device()
    dev.open_path(find_panel("usb")["path"])
    try:
        r = run(dev, a.mode, a.cap, log)
    finally:
        dev.close()
        log.close()
    print("\n== result (%s)" % a.mode)
    print("   ended by %s at %.1f s, %d reports" % (r["ended"], r["end"], r["reports"]))
    print("   last report at %s s after the single 0a 01; silent for %s s at the end" % (
        r["last_rx"], r["silent_at_end"]))
    print("   silences > 3 s: %s" % (r["gaps"] or "none"))
    print("   enable flag at end: %s" % r["flag_at_end"])
    print("log: %s" % path)
    return 0


if __name__ == "__main__":
    sys.exit(main())

"""Can we read interface 2 while DaVinci Resolve has the panel open?

Usage: python tests/concurrency_check.py [--secs 12]
READ-ONLY: opens the device and reads; sends NOTHING (no init, no keep-alive),
so Resolve's state is untouched. Log: logs/concurrency_<ts>.jsonl
"""
import argparse
import subprocess
import sys
import time
from collections import Counter

import hid

from pcommon import JsonLog, find_iface2, hexs, log_path


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--secs", type=float, default=12)
    a = ap.parse_args()
    path = log_path("concurrency")
    log = JsonLog(path)
    tl = subprocess.run(["tasklist", "/FI", "IMAGENAME eq Resolve.exe"],
                        capture_output=True, text=True).stdout
    resolve = "Resolve.exe" in tl
    print("Resolve running: %s" % resolve)
    log.write(t="env", resolve_running=resolve)
    dev = hid.device()
    try:
        dev.open_path(find_iface2()["path"])
    except Exception as e:
        print("OPEN FAILED (exclusive access?): %r" % e)
        log.write(t="open", ok=False, err=repr(e))
        log.close()
        return 1
    log.write(t="open", ok=True)
    print(">>> opened read-only; roll a ball now (%d s)" % a.secs, flush=True)
    ids, errs, t0 = Counter(), 0, time.time()
    try:
        while time.time() - t0 < a.secs:
            try:
                d = dev.read(65, 50)
            except Exception as e:
                errs += 1
                log.write(t="read_err", err=repr(e))
                if errs > 5:
                    break
                continue
            if d:
                b = bytes(d)
                ids[b[0]] += 1
                log.write(t="rx", rid=b[0], data=hexs(b[:37]))
    finally:
        dev.close()
    print("reports read: %s; read errors: %d" % ({"0x%02x" % k: v for k, v in ids.items()}, errs))
    print("VERDICT: %s" % ("shared read WORKS alongside Resolve" if ids and resolve else
                           "no reports (did the ball move? is Resolve's keep-alive running?)"
                           if resolve else "Resolve not running: test not meaningful"))
    print("log: %s" % path)
    log.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())

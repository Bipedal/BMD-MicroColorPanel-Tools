"""Long hands-off log of feature 0x07 and battery (0x08), following USB <-> BT switches.

Usage: python tests/battery_log.py [--minutes 36] [--every 10]
       python tests/battery_log.py --offline

Every --every seconds, on whichever transport is present (USB preferred):
  GET feature 0x07 (6), 0x08 (3), 0x0A (2). Battery input reports (0x08) are logged too.
One 0a 01 at each (re)open; the 0x0A reads keep the panel enabled (spec §3).
Nothing else is written. Test: are 0x07 bytes 2-3 (LE) battery voltage x4?
  -> should fall slowly on battery, jump up on charge, and track the 0x08 level.
Ends at the time cap or USER x5. Log: logs/battery_log_<ts>.jsonl
"""
import argparse
import json
import sys
import time

from pcommon import PID, VID, JsonLog, TapGate, hexs, is_bt, log_path


def pick(hidmod):
    usb = bt = None
    for d in hidmod.enumerate(VID, PID):
        if is_bt(d):
            bt = d["path"]
        elif d.get("interface_number") == 2 or b"mi_02" in d["path"].lower():
            usb = d["path"]
    return ("usb", usb) if usb else (("bt", bt) if bt else (None, None))


def decode07(r):
    """r = 6 bytes incl. ID -> (state byte, u16 LE of bytes 2-3, mV if x4)."""
    v = r[2] | r[3] << 8
    return r[1], v, v / 4.0


def run(hidmod, minutes, every, log, clock=time.time, sleep=time.sleep):
    t0 = clock()
    cap = minutes * 60
    dev, tr, gate = None, None, TapGate(5, 3.0)
    t_tick, t_enum, rows = -1e9, -1e9, []
    end = "cap"
    while clock() - t0 < cap:
        now = clock()
        if now - t_enum >= 1.0:                       # follow transport switches
            t_enum = now
            ntr, path = pick(hidmod)
            if ntr != tr:
                if dev:
                    try:
                        dev.close()
                    except Exception as e:
                        log.write(t="close_err", err=repr(e))
                dev, tr = None, ntr
                log.write(t="transport", tr=tr, rel=round(now - t0, 1))
                print("%7.1f s  transport -> %s" % (now - t0, tr), flush=True)
                if path:
                    try:
                        dev = hidmod.device()
                        dev.open_path(path)
                        dev.send_feature_report(bytes([0x0A, 0x01]))
                        t_tick = -1e9
                    except Exception as e:
                        log.write(t="open_err", tr=tr, err=repr(e))
                        print("   !! open %s failed: %r" % (tr, e), flush=True)
                        dev = None
        if dev is None:
            sleep(0.2)
            continue
        if now - t_tick >= every:
            t_tick = now
            try:
                f7 = bytes(dev.get_feature_report(0x07, 6))
                f8 = bytes(dev.get_feature_report(0x08, 3))
                fa = bytes(dev.get_feature_report(0x0A, 2))
            except Exception as e:
                log.write(t="get_err", tr=tr, err=repr(e), rel=round(now - t0, 1))
                print("%7.1f s  !! GET failed on %s: %r" % (now - t0, tr, e), flush=True)
                t_enum = -1e9                          # re-check the transport now
                continue
            st, v, mv = decode07(f7)
            row = dict(t="sample", tr=tr, rel=round(now - t0, 1), f07=hexs(f7), f08=hexs(f8),
                       f0a=hexs(fa), state=st, raw=v, mv4=mv, charging=f8[1], level=f8[2])
            log.write(**row)
            rows.append(row)
            if len(rows) % 6 == 1:
                print("%7.1f s  %-3s 0x07=%s  raw=%5d (/4 = %6.1f mV)  batt chg=%d %3d%%" % (
                    now - t0, tr, hexs(f7), v, mv, f8[1], f8[2]), flush=True)
        try:
            d = dev.read(65, 50)
        except Exception as e:
            log.write(t="read_err", tr=tr, err=repr(e), rel=round(clock() - t0, 1))
            t_enum = -1e9
            continue
        if d:
            b = bytes(d)
            if b[0] == 0x08:
                log.write(t="batt_event", tr=tr, data=hexs(b[:3]), rel=round(clock() - t0, 1))
                print("%7.1f s  battery event %s" % (clock() - t0, hexs(b[:3])), flush=True)
            if gate.feed(b) == 30:
                end = "USER x5"
                break
    if dev:
        dev.close()
    return rows, end


def summarize(rows):
    print("\n== per transport / charging state")
    groups = {}
    for r in rows:
        groups.setdefault((r["tr"], r["charging"]), []).append(r)
    for (tr, chg), rs in groups.items():
        raws = [r["raw"] for r in rs]
        print("   %-3s chg=%d: %3d samples, %5.1f..%5.1f min, raw %d..%d (mean %.0f), "
              "level %d..%d %%, state byte %s" % (
                  tr, chg, len(rs), rs[0]["rel"] / 60, rs[-1]["rel"] / 60, min(raws), max(raws),
                  sum(raws) / len(raws), min(r["level"] for r in rs),
                  max(r["level"] for r in rs), sorted({r["state"] for r in rs})))
    # trend per contiguous segment: mean of first vs last 5 samples
    print("\n== trend per segment (mean of first 5 vs last 5 samples)")
    seg = []
    for r in rows + [None]:
        if seg and (r is None or (r["tr"], r["charging"]) != (seg[0]["tr"], seg[0]["charging"])):
            a = [x["raw"] for x in seg[:5]]
            b = [x["raw"] for x in seg[-5:]]
            print("   %-3s chg=%d %5.1f-%5.1f min: raw %.0f -> %.0f (%+.0f), level %d -> %d" % (
                seg[0]["tr"], seg[0]["charging"], seg[0]["rel"] / 60, seg[-1]["rel"] / 60,
                sum(a) / len(a), sum(b) / len(b), sum(b) / len(b) - sum(a) / len(a),
                seg[0]["level"], seg[-1]["level"]))
            seg = []
        if r is not None:
            seg.append(r)


class FakeHid:
    """USB 0-60 s (charging, raw ~0x4170), BT 60-180 s (raw falls), USB again 180+."""

    def __init__(self):
        self.t = 0.0

    def clock(self):
        return self.t

    def sleep(self, s):
        self.t += s

    def enumerate(self, v, p):
        if 60 <= self.t < 180:
            return [{"bus_type": 2, "path": b"bt", "interface_number": -1}]
        return [{"bus_type": 1, "path": b"usb-mi_02", "interface_number": 2}]

    def device(self):
        fh = self

        class D:
            def open_path(self, p):
                self.tr = "bt" if p == b"bt" else "usb"

            def send_feature_report(self, b):
                pass

            def get_feature_report(self, rid, n):
                usb = self.tr == "usb"
                if rid == 7:
                    v = 0x4170 if usb else int(0x40c0 - (fh.t - 60) * 0.5)
                    return [7, 3 if usb else 0, v & 255, v >> 8, 0, 0]
                if rid == 8:
                    return [8, 1 if usb else 0, 97]
                return [10, 1]

            def read(self, n, to):
                fh.t += to / 1000
                return []

            def close(self):
                pass
        return D()


def offline():
    log = JsonLog(log_path("battery_log_OFFLINE"))
    fh = FakeHid()
    rows, end = run(fh, 4, 10, log, clock=fh.clock, sleep=fh.sleep)
    log.close()
    summarize(rows)
    trs = [r["tr"] for r in rows]
    bt = [r["raw"] for r in rows if r["tr"] == "bt"]
    checks = {
        "decode07 LE": decode07(bytes([7, 3, 0x77, 0x41, 0, 0]))[1] == 0x4177,
        "switches followed": trs[0] == "usb" and "bt" in trs and trs[-1] == "usb",
        "~10 s spacing": 20 <= len(rows) <= 26,
        "bt trend falls": bt[0] > bt[-1],
        "ended by cap": end == "cap",
    }
    for k, v in checks.items():
        print("  %-18s %s" % (k, "PASS" if v else "FAIL"))
    return 0 if all(checks.values()) else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--minutes", type=float, default=36)
    ap.add_argument("--every", type=float, default=10)
    ap.add_argument("--offline", action="store_true")
    a = ap.parse_args()
    if a.offline:
        return offline()
    import hid
    path = log_path("battery_log")
    log = JsonLog(path)
    log.write(t="config", minutes=a.minutes, every=a.every)
    try:
        rows, end = run(hid, a.minutes, a.every, log)
    except KeyboardInterrupt:
        rows, end = [json.loads(l) for l in open(path, encoding="utf-8")
                     if '"sample"' in l], "interrupted"
    log.write(t="end", why=end)
    log.close()
    print("\nended by %s, %d samples" % (end, len(rows)))
    summarize(rows)
    print("log: %s" % path)
    return 0


if __name__ == "__main__":
    sys.exit(main())

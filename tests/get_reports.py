"""Read every report on request (GET_REPORT) with its exact size, on either transport.

Usage: [PANEL_TRANSPORT=bt] python tests/get_reports.py
Sends only 0a 01 (to check the enable flag before/after). Hands off the panel.
Log: logs/get_reports_<transport>_<ts>.jsonl
"""
import sys
import time

import hid

from pcommon import TRANSPORT, JsonLog, find_panel, hexs, log_path

# id -> exact size (id + payload), from the 297-byte descriptor
FEATURES = {0x01: 8, 0x03: 3, 0x07: 6, 0x08: 3, 0x09: 33, 0x0A: 2}
INPUTS = {0x02: 9, 0x05: 37, 0x06: 65, 0x08: 3}


def main():
    path = log_path("get_reports_%s" % TRANSPORT)
    log = JsonLog(path)
    d = hid.device()
    d.open_path(find_panel()["path"])
    fails = 0

    def get(kind, rid, n):
        nonlocal fails
        try:
            r = bytes(d.get_feature_report(rid, n) if kind == "feature" else d.get_input_report(rid, n))
            log.write(t="get", kind=kind, rid=rid, n=n, data=hexs(r))
            return r, None
        except Exception as e:
            fails += 1
            log.write(t="get", kind=kind, rid=rid, n=n, err=repr(e))
            return None, repr(e)

    print("transport: %s" % TRANSPORT)
    en0, _ = get("feature", 0x0A, 2)
    print("enable flag before init: %s" % (en0.hex(" ") if en0 else "read failed"))
    try:
        d.send_feature_report(bytes([0x0A, 0x01]))
        log.write(t="send", data="0a 01", ok=True)
    except Exception as e:
        log.write(t="send", data="0a 01", ok=False, err=repr(e))
        print("!! send 0a 01 failed: %r" % e)
    time.sleep(0.3)
    en1, _ = get("feature", 0x0A, 2)
    print("enable flag after 0a 01: %s" % (en1.hex(" ") if en1 else "read failed"))
    for kind, table in (("feature", FEATURES), ("input", INPUTS)):
        for rid, n in table.items():
            r, e = get(kind, rid, n)
            txt = e if e else r.hex(" ")
            if r and rid == 0x09:
                txt += "   (ascii %r)" % r[1:].split(b"\0")[0].decode("ascii", "replace")
            print("  %-7s 0x%02x len %2d: %s" % (kind, rid, n, txt))
    d.close()
    print("read failures: %d\nlog: %s" % (fails, path))
    log.close()


if __name__ == "__main__":
    sys.exit(main())

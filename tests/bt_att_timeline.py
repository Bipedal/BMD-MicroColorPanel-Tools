"""What does a host send to the panel over Bluetooth, and when? (ATT timeline)

Usage: python tests/bt_att_timeline.py <capture.pcapng> [--adapter 14] [--handle auto]
                                       [--marker LOOP]
       python tests/bt_att_timeline.py --offline

Input: a USBPcap capture of the USB Bluetooth adapter (tests/usb_capture.py --bt). Such
captures hold link keys, so they are always named *_PRIVATE_* and never published; this
decoder prints only what is safe:
  * LE connection events (handle, interval, latency, timeout; never addresses or keys)
  * for the panel's connection only: ATT writes / reads / notifications, mapped to HID
    report IDs with the GATT table from logs/btgatt_pairing_20261116_190313.txt
    (handles are stable while the panel stays bonded). Output 0x04 gets LED names,
    input 0x02 button names; 0x05/0x06 movement is collapsed into runs.
  * notification timing (spec §9 #10): gaps between notifications while moving.
The panel's connection is found automatically: the LE handle whose notifications match
the panel's report handles and lengths. SMP (cid 6) and other connections are skipped.
Log: logs/bttimeline_<capture name without _PRIVATE>.txt
"""
import argparse
import os
import struct
import sys
from collections import Counter, defaultdict
from statistics import median

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from panel_map import BUTTONS  # noqa: E402
from pcommon import LOGDIR  # noqa: E402
from usb_decode import leds04, parse_usbpcap, read_pcapng  # noqa: E402

# value handle -> (report id, type, payload length without ID)   [V] btgatt_pairing
REPORTS = {0x0019: (0x01, "feature", 7), 0x001C: (0x02, "input", 8),
           0x0020: (0x02, "output", 8), 0x0023: (0x03, "feature", 2),
           0x0026: (0x04, "output", 6), 0x0029: (0x05, "input", 36),
           0x002D: (0x06, "input", 64), 0x0031: (0x07, "feature", 5),
           0x0034: (0x08, "feature", 2), 0x0037: (0x08, "input", 2),
           0x003B: (0x09, "feature", 32), 0x003E: (0x0A, "feature", 1)}
CCCD = {0x001E: 0x02, 0x002B: 0x05, 0x002F: 0x06, 0x0039: 0x08}
ATT = {0x01: "Error", 0x0A: "Read Req", 0x0B: "Read Rsp", 0x12: "Write Req",
       0x13: "Write Rsp", 0x52: "Write Cmd", 0x1B: "Notify", 0x1D: "Indicate"}


def frames(path, adapter):
    """Yield ('acl', t, handle, dir, cid, pdu) and ('evt', t, code, params)."""
    frag = {}
    t0 = None
    for ts, raw in read_pcapng(path):
        try:
            p = parse_usbpcap(raw)
        except struct.error:
            continue
        if p["dev"] != adapter or not p["data"]:
            continue
        t0 = ts if t0 is None else t0
        t = ts - t0
        d = bytes(p["data"])
        if p["xfer"] == 1 and p["ep"] & 0x80 and p["completion"] and len(d) >= 2:
            yield ("evt", t, d[0], d[2:2 + d[1]])
            continue
        if p["xfer"] != 3:
            continue
        ep_in = bool(p["ep"] & 0x80)
        if ep_in != p["completion"] or len(d) < 4:
            continue
        hf, ln = struct.unpack_from("<HH", d, 0)
        h, pb = hf & 0x0FFF, (hf >> 12) & 3
        dirn = "dev->host" if ep_in else "host->dev"
        body = d[4:4 + ln]
        key = (h, dirn)
        if pb in (0, 2):
            if len(body) < 4:
                continue
            frag[key] = [struct.unpack_from("<H", body, 0)[0], bytearray(body)]
        elif key in frag:
            frag[key][1] += body
        else:
            continue
        need, buf = frag[key]
        if len(buf) >= need + 4:
            yield ("acl", t, h, dirn, struct.unpack_from("<H", buf, 2)[0], bytes(buf[4:4 + need]))
            del frag[key]


def le_event(code, prm):
    """HCI event -> text (no addresses), or None if not interesting."""
    if code == 0x05 and len(prm) >= 4:
        return "Disconnection: handle 0x%04x reason 0x%02x" % (
            struct.unpack_from("<H", prm, 1)[0] & 0xFFF, prm[3])
    if code != 0x3E or not prm:
        return None
    sub = prm[0]
    if sub in (0x01, 0x0A) and len(prm) >= 19:
        off = 12 if sub == 0x01 else 24          # after status, handle, role, type, addr(, RPAs)
        h = struct.unpack_from("<H", prm, 2)[0] & 0xFFF
        iv, lat, to = struct.unpack_from("<HHH", prm, off)
        return "LE Connection Complete: status 0x%02x handle 0x%04x interval %.2f ms latency %d timeout %d ms" % (
            prm[1], h, iv * 1.25, lat, to * 10)
    if sub == 0x03 and len(prm) >= 10:
        h, iv, lat, to = struct.unpack_from("<HHHH", prm, 2)
        return "LE Connection Update Complete: status 0x%02x handle 0x%04x interval %.2f ms latency %d timeout %d ms" % (
            prm[1], h & 0xFFF, iv * 1.25, lat, to * 10)
    if sub == 0x06 and len(prm) >= 11:
        h, imin, imax, lat, to = struct.unpack_from("<HHHHH", prm, 1)
        return "LE Remote Conn Param Request: handle 0x%04x interval %.2f-%.2f ms latency %d timeout %d ms" % (
            h & 0xFFF, imin * 1.25, imax * 1.25, lat, to * 10)
    return None


def pick_handle(items):
    score = Counter()
    for it in items:
        if it[0] == "acl" and it[4] == 4 and len(it[5]) >= 3 and it[5][0] == 0x1B:
            h = struct.unpack_from("<H", it[5], 1)[0]
            r = REPORTS.get(h)
            if r and r[1] == "input" and len(it[5]) - 3 == r[2]:
                score[it[2]] += 1
    if not score:      # no input traffic (e.g. discovery only): any ATT op on report handles
        for it in items:
            if it[0] == "acl" and it[4] == 4 and len(it[5]) >= 3 and it[5][0] in (0x0A, 0x12, 0x52):
                if struct.unpack_from("<H", it[5], 1)[0] in REPORTS:
                    score[it[2]] += 1
    return score.most_common(1)[0][0] if score else None, dict(score)


def bits(payload8):
    return {pos * 8 + i for pos in range(1, 9) for i in range(8)
            if payload8[pos - 1] >> i & 1}


def analyse(items, handle, marker_bit, pr):
    names = {v: k for k, v in BUTTONS.items()}
    lines, held, taps, pending = [], set(), [], {}
    notif_t, move_runs = [], []
    counts = Counter()
    run = None                                    # [rid, n, t_first, t_last]

    def flush():
        nonlocal run
        if run:
            lines.append((run[2], "   input 0x%02x movement x%d (until %.2f)" % (run[0], run[1], run[3])))
            run = None

    for it in items:
        if it[0] == "evt":
            txt = le_event(it[2], it[3])
            if txt:
                flush()
                lines.append((it[1], "HCI " + txt))
            continue
        _, t, h, dirn, cid, pdu = it
        if h != handle or cid != 4 or not pdu:
            continue
        op = pdu[0]
        counts[ATT.get(op, "op 0x%02x" % op)] += 1
        ah = struct.unpack_from("<H", pdu, 1)[0] if len(pdu) >= 3 else None
        r = REPORTS.get(ah)
        if op in (0x12, 0x52) and ah is not None:
            v = pdu[3:]
            flush()
            if r:
                rid, typ, _ = r
                full = bytes([rid]) + v
                txt = "SET %s 0x%02x: %s" % (typ, rid, full.hex(" "))
                if rid == 0x04 and typ == "output" and len(full) >= 7:
                    txt += "   LEDs: " + leds04(full)
            elif ah in CCCD:
                txt = "CCCD for input 0x%02x = %s" % (CCCD[ah], v.hex(" "))
            else:
                txt = "write handle 0x%04x: %s" % (ah, v.hex(" "))
            lines.append((t, txt + ("" if op == 0x12 else "  (no rsp)")))
        elif op == 0x0A and ah is not None:
            pending[dirn] = (t, ah)
        elif op == 0x0B and "host->dev" in pending:
            t1, rh = pending.pop("host->dev")
            r = REPORTS.get(rh)
            flush()
            lab = ("GET %s 0x%02x" % (r[1], r[0])) if r else ("read handle 0x%04x" % rh)
            lines.append((t1, "%s -> %s%s" % (lab, (bytes([r[0]]) if r else b"").hex(), " " + pdu[1:].hex(" "))))
        elif op == 0x01 and len(pdu) >= 5:
            flush()
            lines.append((t, "ATT Error for op 0x%02x handle 0x%04x code 0x%02x" % (
                pdu[1], struct.unpack_from("<H", pdu, 2)[0], pdu[4])))
        elif op == 0x1B and r and r[1] == "input":
            rid, v = r[0], pdu[3:]
            notif_t.append((t, rid))
            if rid == 0x02 and len(v) >= 8:
                now = bits(v)
                flush()
                for b in sorted(now - held):
                    if b == marker_bit:
                        taps = [x for x in taps if t - x <= 3] + [t]
                        if len(taps) >= 5:
                            lines.append((t, "==== %s x5 marker ====" % BUTTONS[b]))
                            taps = []
                        continue
                    lines.append((t, "   [press %s]" % BUTTONS.get(b, b)))
                for b in sorted(held - now):
                    if b != marker_bit:
                        lines.append((t, "   [release %s]" % BUTTONS.get(b, b)))
                held = now
            elif rid in (0x05, 0x06):
                if run and run[0] == rid and t - run[3] < 1.0:
                    run[1] += 1
                    run[3] = t
                else:
                    flush()
                    run = [rid, 1, t, t]
            else:
                flush()
                lines.append((t, "   input 0x%02x: %s" % (rid, v.hex(" "))))
    flush()
    lines.sort(key=lambda x: x[0])
    pr("ATT ops on the panel connection: %s" % dict(counts))
    # cadence of host writes
    for lab in ("SET feature 0x0a", "SET feature 0x03", "SET output 0x04", "GET feature 0x08"):
        ts = [t for t, x in lines if x.startswith(lab)]
        iv = [b - a for a, b in zip(ts, ts[1:])]
        pr("   %-18s %3d x%s" % (lab, len(ts), (", median interval %.2f s" % median(iv)) if iv else ""))
    # notification timing
    mv = [t for t, rid in notif_t if rid in (0x05, 0x06)]
    gaps = [b - a for a, b in zip(mv, mv[1:]) if b - a < 0.2]
    if gaps:
        ms = [g * 1000 for g in gaps]
        same = sum(1 for g in ms if g < 3)
        bins = Counter(int(g // 7.5) for g in ms if g >= 3)
        pr("notification timing while moving: %d gaps < 200 ms; %d within 3 ms (same "
           "connection event); median of the rest %.1f ms" % (
               len(ms), same, median([g for g in ms if g >= 3]) if len(ms) > same else 0))
        pr("   gaps >= 3 ms in 7.5 ms bins: %s" % ", ".join(
            "%.1f-%.1f ms: %d" % (k * 7.5, k * 7.5 + 7.5, n) for k, n in sorted(bins.items())[:10]))
    pr("\n== timeline (s)")
    for t, x in lines:
        pr("   %8.2f  %s" % (t, x))
    return lines, gaps


# ------------------------------------------------------------- offline self-test
def offline():
    """Synthetic frames: two connections (a mouse-like one and the panel)."""
    def att(t, h, dirn, pdu):
        return ("acl", t, h, dirn, 4, pdu)

    def notify(t, h, ah, v):
        return att(t, h, "dev->host", bytes([0x1B]) + struct.pack("<H", ah) + v)

    P, M = 0x0E05, 0x0E01
    items = [("evt", 0.0, 0x3E, bytes([0x0A, 0x00]) + struct.pack("<H", P) + bytes(20) +
              struct.pack("<HHH", 12, 5, 200) + b"\x00"),
             ("evt", 0.5, 0x3E, bytes([0x03, 0x00]) + struct.pack("<HHHH", P, 24, 0, 200))]
    items += [notify(0.1 + i * 0.01, M, 0x001C, bytes(5)) for i in range(10)]   # wrong length
    items += [att(1.0, P, "host->dev", bytes([0x12]) + struct.pack("<H", 0x0023) + b"\x00\x00"),
              att(1.01, P, "host->dev", bytes([0x52]) + struct.pack("<H", 0x0026) + bytes(6)),
              att(1.02, P, "host->dev", bytes([0x0A]) + struct.pack("<H", 0x0019)),
              att(1.03, P, "dev->host", bytes([0x0B, 0x15, 0x5B, 0x3B, 0xFD, 2, 2, 0])),
              att(1.04, P, "host->dev", bytes([0x12]) + struct.pack("<H", 0x003E) + b"\x01"),
              att(31.04, P, "host->dev", bytes([0x12]) + struct.pack("<H", 0x003E) + b"\x01"),
              att(61.04, P, "host->dev", bytes([0x12]) + struct.pack("<H", 0x003E) + b"\x01")]
    off = bytearray(8)
    on = bytearray(8)
    on[21 // 8 - 1] |= 1 << (21 % 8)                 # OFFSET bit 21 -> payload byte 1
    t = 2.0
    for v in (on, off):
        items.append(notify(t, P, 0x001C, bytes(v)))
        t += 0.2
    items.append(att(2.05, P, "host->dev", bytes([0x52]) + struct.pack("<H", 0x0026) + bytes([4, 0, 0, 0, 0, 0])))
    loop = bytearray(8)
    loop[31 // 8 - 1] |= 1 << (31 % 8)
    for k in range(5):
        items.append(notify(3.0 + k * 0.3, P, 0x001C, bytes(loop)))
        items.append(notify(3.1 + k * 0.3, P, 0x001C, bytes(8)))
    for k in range(40):                              # movement: pairs 30 ms apart
        base = 5.0 + (k // 2) * 0.030 + (k % 2) * 0.001
        items.append(notify(base, P, 0x0029, bytes(36)))
    items.sort(key=lambda x: x[1])
    out = []
    h, sc = pick_handle(items)
    lines, gaps = analyse(items, h, 31, out.append)
    txt = "\n".join(out)
    ok = True

    def chk(c, m):
        nonlocal ok
        ok &= bool(c)
        print("  %s  %s" % ("PASS" if c else "FAIL", m))
    chk(h == P, "panel handle picked by matching notifications: 0x%04x %s" % (h, sc))
    chk("interval 15.00 ms latency 5 timeout 2000 ms" in txt, "connection complete parsed")
    chk("Update Complete: status 0x00 handle 0x0e05 interval 30.00 ms" in txt, "update parsed")
    chk("SET feature 0x03: 03 00 00" in txt and "SET output 0x04: 04 00 00 00 00 00 00   LEDs: none" in txt,
        "writes mapped to report IDs")
    chk("GET feature 0x01 -> 01 15 5b 3b fd 02 02 00" in txt, "read paired with response")
    chk(any(l.split() == ["SET", "feature", "0x0a", "3", "x,", "median", "interval", "30.00", "s"] for l in out), "keep-alive cadence")
    chk("[press OFFSET]" in txt and "LEDs: OFFSET" in txt and "[press LOOP]" not in txt
        and "LOOP x5 marker" in txt, "buttons, LED names, marker hidden")
    chk("input 0x05 movement x40" in txt, "movement collapsed")
    chk("20 within 3 ms" in txt and "median of the rest 29.0 ms" in txt,
        "timing: bursts of 2 per 30 ms event")
    print(txt if not ok else "")
    print("offline:", "ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pcap", nargs="?")
    ap.add_argument("--adapter", type=int, default=14)
    ap.add_argument("--handle", default="auto")
    ap.add_argument("--marker", default="LOOP")
    ap.add_argument("--offline", action="store_true")
    a = ap.parse_args()
    if a.offline:
        return offline()
    if not a.pcap:
        ap.error("pcap required")
    names = {v: k for k, v in BUTTONS.items()}
    items = list(frames(a.pcap, a.adapter))
    out = []
    pr = out.append
    pr("== %s, adapter %d" % (os.path.basename(a.pcap), a.adapter))
    h, sc = pick_handle(items)
    if a.handle != "auto":
        h = int(a.handle, 0)
    pr("LE handles with panel-shaped notifications: %s; using %s" % (
        {("0x%04x" % k): v for k, v in sc.items()}, "0x%04x" % h if h is not None else "none"))
    if h is None:
        print("\n".join(out))
        print("no panel connection found")
        return 1
    analyse(items, h, names[a.marker], pr)
    base = os.path.splitext(os.path.basename(a.pcap))[0].replace("_PRIVATE", "")
    path = os.path.join(LOGDIR, "bttimeline_%s.txt" % base)
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print("\n".join(out[:40]))
    print("...\nlog: %s" % path)
    return 0


if __name__ == "__main__":
    sys.exit(main())

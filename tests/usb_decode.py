"""Decode a USBPcap pcapng of the panel: what does the host send, and when?

Usage: python tests/usb_decode.py logs/usb_<name>_<ts>.pcapng [--addr N]

Reads pcapng directly (no tshark needed). Pairs control SETUP/completion by IRP id.
Reports:
  * raw HID report descriptor if the capture contains enumeration (replug)
  * every host->device HID request (SET_REPORT feature/output, SET_IDLE, ...),
    grouped, with distinct payloads, counts and typical interval
  * GET_REPORT requests with status (STALL etc.) and responses
  * IN interrupt reports per ID with wire lengths; all 0x08 values
  * a timeline split at USER x5 markers (0x02 bit 30), with repeats collapsed;
    output 0x02 payloads also shown as "LED bit n = button bit n" (HYPOTHESIS)
Log: logs/usbdecode_<pcapname>.txt
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

USER_BIT = 30
STATUS = {0: "OK", 0xC0000004: "STALL", 0xC0000011: "XACT_ERR", 0xC0010000: "CANCELED"}
HID_REQ = {0x01: "GET_REPORT", 0x02: "GET_IDLE", 0x03: "GET_PROTOCOL",
           0x09: "SET_REPORT", 0x0A: "SET_IDLE", 0x0B: "SET_PROTOCOL"}
RTYPE = {1: "input", 2: "output", 3: "feature"}


# ------------------------------------------------------------------ pcapng
def read_pcapng(path):
    """Yield (ts_seconds, packet_bytes)."""
    data = open(path, "rb").read()
    i, end, tsres = 0, "<", {}
    ifaces = []
    while i + 12 <= len(data):
        btype = struct.unpack_from(end + "I", data, i)[0]
        if btype == 0x0A0D0D0A:                     # section header: detect endianness
            bom = data[i + 8:i + 12]
            end = "<" if bom == b"\x4d\x3c\x2b\x1a" else ">"
            ifaces = []
        blen = struct.unpack_from(end + "I", data, i + 4)[0]
        body = data[i + 8:i + blen - 4]
        if btype == 1:                              # interface description
            res = 1e-6
            j = 8
            while j + 4 <= len(body):
                code, ln = struct.unpack_from(end + "HH", body, j)
                if code == 0:
                    break
                if code == 9:
                    v = body[j + 4]
                    res = 2 ** -(v & 0x7F) if v & 0x80 else 10 ** -v
                j += 4 + ((ln + 3) & ~3)
            ifaces.append(res)
        elif btype == 6:                            # enhanced packet
            iid, th, tl, cap, _ = struct.unpack_from(end + "IIIII", body, 0)
            res = ifaces[iid] if iid < len(ifaces) else 1e-6
            yield ((th << 32) | tl) * res, body[20:20 + cap]
        i += blen


def parse_usbpcap(pkt):
    hl, irp, status, func, info, bus, dev, ep, xfer, dlen = struct.unpack_from(
        "<HQIHBHHBBI", pkt, 0)
    p = {"irp": irp, "status": status, "func": func, "completion": bool(info & 1),
         "bus": bus, "dev": dev, "ep": ep, "xfer": xfer, "dlen": dlen}
    if xfer == 2:
        p["stage"] = pkt[27]
    p["data"] = pkt[hl:hl + dlen]
    return p


# ------------------------------------------------------------------ decode
def decode(path, addr=None):
    pkts = []
    for ts, raw in read_pcapng(path):
        try:
            p = parse_usbpcap(raw)
        except struct.error:
            continue
        if addr is not None and p["dev"] != addr:
            continue
        p["ts"] = ts
        pkts.append(p)
    if not pkts:
        return None
    t0 = pkts[0]["ts"]
    for p in pkts:
        p["t"] = p["ts"] - t0

    pending, ctrl, ins, outs = {}, [], [], []
    for p in pkts:
        if p["xfer"] == 2:                          # control
            if not p["completion"] and p.get("stage") == 0 and len(p["data"]) >= 8:
                bm, br, wv, wi, wl = struct.unpack_from("<BBHHH", p["data"], 0)
                pending[p["irp"]] = {"t": p["t"], "bm": bm, "br": br, "wv": wv, "wi": wi,
                                     "wl": wl, "out": bytes(p["data"][8:]), "dev": p["dev"]}
            elif p["completion"] and p["irp"] in pending:
                c = pending.pop(p["irp"])
                c["status"] = p["status"]
                c["resp"] = bytes(p["data"])
                ctrl.append(c)
        elif p["xfer"] == 1:                        # interrupt
            if p["ep"] & 0x80 and p["completion"] and p["data"]:
                ins.append((p["t"], bytes(p["data"])))
            elif not p["ep"] & 0x80 and not p["completion"] and p["data"]:
                outs.append((p["t"], bytes(p["data"])))
    for c in pending.values():                      # never completed
        c["status"], c["resp"] = None, b""
        ctrl.append(c)
    ctrl.sort(key=lambda c: c["t"])
    return {"pkts": pkts, "ctrl": ctrl, "ins": ins, "outs": outs,
            "devs": Counter(p["dev"] for p in pkts)}


def describe(c):
    """-> (group key, text) for a control transfer."""
    bm, br, wv = c["bm"], c["br"], c["wv"]
    kind = (bm >> 5) & 3
    st = "pending" if c["status"] is None else STATUS.get(c["status"], "0x%08x" % c["status"])
    if kind == 1:                                   # class
        name = HID_REQ.get(br, "CLASS_0x%02x" % br)
        if br in (0x01, 0x09):
            rt, rid = wv >> 8, wv & 0xFF
            key = (name, RTYPE.get(rt, rt), rid)
            if br == 0x09:
                return key, "%s %s 0x%02x: %s [%s]" % (name, RTYPE.get(rt, rt), rid,
                                                       c["out"].hex(" "), st)
            return key, "%s %s 0x%02x len %d -> [%s] %s" % (name, RTYPE.get(rt, rt), rid,
                                                            c["wl"], st, c["resp"].hex(" "))
        return (name, wv), "%s wValue 0x%04x [%s]" % (name, wv, st)
    if kind == 0 and br == 6:
        dt = wv >> 8
        return ("GET_DESCRIPTOR", dt), "GET_DESCRIPTOR type 0x%02x idx %d len %d [%s] %d bytes" % (
            dt, wv & 0xFF, c["wl"], st, len(c["resp"]))
    return (("STD" if kind == 0 else "VENDOR"), bm, br), \
        "bm=0x%02x req=0x%02x wValue=0x%04x wIndex=0x%04x out=%s [%s] resp=%s" % (
            bm, br, wv, c["wi"], c["out"].hex(" "), st, c["resp"].hex(" "))


def led_names(payload):
    """HYPOTHESIS: output 0x02 bit n lights the button with input bit n."""
    bits = [pos * 8 + i for pos in range(1, min(9, len(payload)))
            for i in range(8) if payload[pos] >> i & 1]
    return ",".join(BUTTONS.get(b, "b%d" % b) for b in bits) or "none"


def markers(ins):
    """Times at which USER was pressed 5 times within 3 s."""
    held, taps, out = set(), [], []
    for t, b in ins:
        if b[0] != 0x02 or len(b) < 9:
            continue
        now = {pos * 8 + i for pos in range(1, 9) for i in range(8) if b[pos] >> i & 1}
        if USER_BIT in now - held:
            taps = [x for x in taps if t - x <= 3] + [t]
            if len(taps) >= 5:
                out.append(t)
                taps = []
        held = now
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pcap")
    ap.add_argument("--addr", type=int, default=None)
    a = ap.parse_args()
    d = decode(a.pcap, a.addr)
    lines = []
    pr = lines.append
    if not d:
        print("no packets")
        return 1
    pr("== %s" % a.pcap)
    pr("packets %d, duration %.1f s, device addresses %s"
       % (len(d["pkts"]), d["pkts"][-1]["t"], dict(d["devs"])))

    # descriptors
    for c in d["ctrl"]:
        if c["bm"] == 0x81 and c["br"] == 6 and c["wv"] >> 8 == 0x22 and c["resp"]:
            pr("\n== RAW REPORT DESCRIPTOR (%d bytes) at t=%.2f" % (len(c["resp"]), c["t"]))
            for k in range(0, len(c["resp"]), 16):
                pr("   " + c["resp"][k:k + 16].hex(" "))
            try:
                import dump_caps
                buf = []
                dump_caps.out = buf.append
                tot = dump_caps.parse_descriptor(list(c["resp"]))
                lines.extend(buf)
                for (typ, rid), bits in sorted(tot.items(), key=lambda x: (x[0][1], x[0][0])):
                    pr("   %-7s id=0x%02x %4d bits = %d bytes" % (typ, rid, bits, (bits + 7) // 8))
            except Exception as e:      # parser problem must not hide the raw bytes
                pr("   (descriptor parse failed: %r)" % e)

    # host -> device summary
    groups = defaultdict(list)
    for c in d["ctrl"]:
        key, txt = describe(c)
        groups[key].append((c, txt))
    pr("\n== control requests (grouped)")
    for key, items in sorted(groups.items(), key=lambda kv: kv[1][0][0]["t"]):
        ts = [c["t"] for c, _ in items]
        iv = [b - a for a, b in zip(ts, ts[1:])]
        pr("%s: %d x, first %.2f s, last %.2f s%s" % (
            " ".join(str(k) for k in key), len(items), ts[0], ts[-1],
            (", median interval %.2f s" % median(iv)) if iv else ""))
        for txt, n in Counter(t for _, t in items).most_common(12):
            extra = ""
            c0 = next(c for c, t in items if t == txt)
            if c0["br"] == 0x09 and (c0["wv"] >> 8) == 2 and (c0["wv"] & 0xFF) == 0x02:
                extra = "   LEDs? %s" % led_names(c0["out"])
            pr("     %4d x  %s%s" % (n, txt, extra))
        if len(set(t for _, t in items)) > 12:
            pr("     ... %d distinct payloads" % len(set(t for _, t in items)))
    if d["outs"]:
        pr("\n== interrupt OUT: %d transfers" % len(d["outs"]))
        for txt, n in Counter(b.hex(" ") for _, b in d["outs"]).most_common(12):
            pr("     %4d x  %s" % (n, txt))

    # inputs
    pr("\n== interrupt IN reports")
    byid = defaultdict(list)
    for t, b in d["ins"]:
        byid[b[0]].append((t, b))
    for rid, items in sorted(byid.items()):
        pr("   0x%02x: %d reports, wire lengths %s" % (
            rid, len(items), dict(Counter(len(b) for _, b in items))))
    for t, b in byid.get(0x08, []):
        pr("   0x08 @ %.2f s: %s" % (t, b.hex(" ")))

    # timeline
    mk = markers(d["ins"])
    pr("\n== USER x5 markers at: %s" % ", ".join("%.1f" % m for m in mk))
    bounds = [0.0] + mk + [float("inf")]
    for si in range(len(bounds) - 1):
        lo, hi = bounds[si], bounds[si + 1]
        seg = [c for c in d["ctrl"] if lo <= c["t"] < hi]
        iout = [(t, b) for t, b in d["outs"] if lo <= t < hi]
        nin = sum(1 for t, _ in d["ins"] if lo <= t < hi)
        pr("\n-- segment %d (%.1f .. %s s): %d control requests, %d interrupt OUT, %d IN reports"
           % (si, lo, "end" if hi == float("inf") else "%.1f" % hi, len(seg), len(iout), nin))
        items = []                                  # (t, text, shown)
        for c in seg:
            txt = describe(c)[1]
            shown = txt
            if c["br"] == 0x09 and (c["wv"] >> 8) == 2 and (c["wv"] & 0xFF) == 0x02:
                shown += "   LEDs? " + led_names(c["out"])
            items.append((c["t"], txt, shown))
        for t, b in iout:
            txt = "INT OUT report 0x%02x: %s" % (b[0], b.hex(" "))
            shown = txt + (("   LEDs? " + led_names(b)) if b[0] == 0x02 else "")
            items.append((t, txt, shown))
        # button presses in this segment, so LED changes can be matched to keys
        held = set()
        for t, b in d["ins"]:
            if b[0] != 0x02 or len(b) < 9:
                continue
            now = {pos * 8 + i for pos in range(1, 9) for i in range(8) if b[pos] >> i & 1}
            if lo <= t < hi:
                for bit in sorted(now - held):
                    if bit != USER_BIT:
                        items.append((t, "press %s" % BUTTONS.get(bit, bit),
                                      "   [press %s]" % BUTTONS.get(bit, bit)))
                for bit in sorted(held - now):
                    if bit != USER_BIT:
                        items.append((t, "release %s" % BUTTONS.get(bit, bit),
                                      "   [release %s]" % BUTTONS.get(bit, bit)))
            held = now
        items.sort(key=lambda x: x[0])
        runs = []                                   # [text, count, t_first, t_last, shown]
        for t, txt, shown in items:
            if runs and runs[-1][0] == txt:
                runs[-1][1] += 1
                runs[-1][3] = t
                continue
            runs.append([txt, 1, t, t, shown])
        for txt, n, t1, t2, shown in runs:
            pr("   %7.2f  %s%s" % (t1, shown, ("   (x%d, until %.2f)" % (n, t2)) if n > 1 else ""))
    out = os.path.join(LOGDIR, "usbdecode_%s.txt" % os.path.splitext(os.path.basename(a.pcap))[0])
    with open(out, "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")
    print("\n".join(lines))
    print("\nlog: %s" % out)
    return 0


if __name__ == "__main__":
    sys.exit(main())

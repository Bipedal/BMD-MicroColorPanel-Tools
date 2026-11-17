"""Decode a USBPcap capture of a USB Bluetooth adapter: GATT table and values for one
LE connection (HCI ACL over USB bulk -> L2CAP -> ATT), without Wireshark dissectors.

Usage: python tests/bt_gatt_decode.py <capture.pcapng> [--adapter 14] [--handle 0x0e02]

Prints: services, characteristics (properties, value handle), descriptors, every value
read (Read / Read Blob reassembled), Report Reference descriptors -> report IDs, the HID
Report Map parsed like a USB report descriptor, ATT writes, and notifications.
SMP (pairing) packets are counted but their contents are NOT printed (keys).
Log: logs/btgatt_<capture name>.txt
"""
import argparse
import os
import struct
import sys
from collections import Counter, defaultdict

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pcommon import LOGDIR  # noqa: E402
from usb_decode import parse_usbpcap, read_pcapng  # noqa: E402

UUID16 = {
    0x1800: "Generic Access", 0x1801: "Generic Attribute", 0x180A: "Device Information",
    0x180F: "Battery", 0x1812: "Human Interface Device", 0x1813: "Scan Parameters",
    0x2800: "<Primary Service>", 0x2801: "<Secondary Service>", 0x2802: "<Include>",
    0x2803: "<Characteristic>",
    0x2A00: "Device Name", 0x2A01: "Appearance", 0x2A04: "Peripheral Preferred Conn Params",
    0x2A05: "Service Changed", 0x2AA6: "Central Address Resolution",
    0x2B29: "Client Supported Features", 0x2B2A: "Database Hash", 0x2B3A: "Server Supported Features",
    0x2A29: "Manufacturer Name", 0x2A24: "Model Number", 0x2A25: "Serial Number",
    0x2A26: "Firmware Revision", 0x2A27: "Hardware Revision", 0x2A28: "Software Revision",
    0x2A23: "System ID", 0x2A50: "PnP ID", 0x2A19: "Battery Level",
    0x2A4A: "HID Information", 0x2A4B: "Report Map", 0x2A4C: "HID Control Point",
    0x2A4D: "Report", 0x2A4E: "Protocol Mode", 0x2A22: "Boot Keyboard Input",
    0x2A32: "Boot Keyboard Output", 0x2A33: "Boot Mouse Input",
    0x2A4F: "Scan Interval Window", 0x2A31: "Scan Refresh",
    0x2900: "Char Extended Properties", 0x2901: "Char User Description",
    0x2902: "CCCD", 0x2907: "External Report Reference", 0x2908: "Report Reference",
}
ATT_OP = {0x01: "Error", 0x02: "MTU Req", 0x03: "MTU Rsp", 0x04: "Find Info Req",
          0x05: "Find Info Rsp", 0x06: "Find By Type Req", 0x07: "Find By Type Rsp",
          0x08: "Read By Type Req", 0x09: "Read By Type Rsp", 0x0A: "Read Req",
          0x0B: "Read Rsp", 0x0C: "Read Blob Req", 0x0D: "Read Blob Rsp",
          0x10: "Read By Group Req", 0x11: "Read By Group Rsp", 0x12: "Write Req",
          0x13: "Write Rsp", 0x52: "Write Cmd", 0x1B: "Notification", 0x1D: "Indication",
          0x1E: "Confirmation"}
PROPS = ["broadcast", "read", "write-no-rsp", "write", "notify", "indicate", "signed", "ext"]
RTYPE = {1: "input", 2: "output", 3: "feature"}


def uuid_str(b):
    if len(b) == 2:
        u = struct.unpack("<H", b)[0]
        return "0x%04X %s" % (u, UUID16.get(u, ""))
    h = b[::-1].hex().upper()
    return "%s-%s-%s-%s-%s" % (h[:8], h[8:12], h[12:16], h[16:20], h[20:])


def acl_stream(path, adapter, handle):
    """Yield (t, direction 'host->dev'|'dev->host', l2cap_cid, payload) for one handle."""
    frag = {}
    t0 = None
    for ts, raw in read_pcapng(path):
        try:
            p = parse_usbpcap(raw)
        except struct.error:
            continue
        if p["dev"] != adapter or p["xfer"] != 3 or not p["data"]:
            continue
        ep_in = bool(p["ep"] & 0x80)
        if ep_in != p["completion"]:          # IN data arrives on completion, OUT on submit
            continue
        t0 = ts if t0 is None else t0
        d = bytes(p["data"])
        if len(d) < 4:
            continue
        hf, ln = struct.unpack_from("<HH", d, 0)
        h, pb = hf & 0x0FFF, (hf >> 12) & 3
        if h != handle:
            continue
        direction = "dev->host" if ep_in else "host->dev"
        body = d[4:4 + ln]
        key = direction
        if pb in (0, 2):                      # start of L2CAP frame
            if len(body) < 4:
                continue
            l2len = struct.unpack_from("<H", body, 0)[0]
            frag[key] = [l2len, bytearray(body)]
        elif key in frag:
            frag[key][1] += body
        else:
            continue
        need, buf = frag[key]
        if len(buf) >= need + 4:
            cid = struct.unpack_from("<H", buf, 2)[0]
            yield ts - t0, direction, cid, bytes(buf[4:4 + need])
            del frag[key]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("pcap")
    ap.add_argument("--adapter", type=int, default=14)
    ap.add_argument("--handle", type=lambda s: int(s, 0), default=0x0E02)
    a = ap.parse_args()
    out = []
    pr = out.append

    services, chars, descs, values = [], {}, {}, defaultdict(bytearray)
    writes, notifs, errors, ops, cids = [], [], [], Counter(), Counter()
    pending = None                            # last request host->dev
    for t, dirn, cid, pdu in acl_stream(a.pcap, a.adapter, a.handle):
        cids[cid] += 1
        if cid != 0x0004 or not pdu:          # 0x0006 = SMP (pairing): count only
            continue
        op = pdu[0]
        ops[(dirn, ATT_OP.get(op, "0x%02x" % op))] += 1
        if dirn == "host->dev":
            if op in (0x0A, 0x0C):            # Read / Read Blob
                pending = (op, struct.unpack_from("<H", pdu, 1)[0])
            elif op in (0x12, 0x52):
                h = struct.unpack_from("<H", pdu, 1)[0]
                writes.append((t, ATT_OP[op], h, pdu[3:]))
                pending = None
            elif op == 0x08:                  # Read By Type: remember which type was asked
                pending = (op, None, bytes(pdu[5:]))
            else:
                pending = (op, None)
            continue
        # dev -> host
        if op == 0x11:                        # Read By Group Type Rsp: services
            n = pdu[1]
            for i in range(2, len(pdu), n):
                s, e = struct.unpack_from("<HH", pdu, i)
                services.append((s, e, pdu[i + 4:i + n]))
        elif op == 0x09:                      # Read By Type Rsp
            n = pdu[1]
            asked = pending[2] if pending and len(pending) > 2 else None
            for i in range(2, len(pdu), n):
                if asked == b"\x03\x28":      # characteristic declarations
                    h, props, vh = struct.unpack_from("<HBH", pdu, i)
                    chars[vh] = (h, props, pdu[i + 5:i + n])
                else:                         # a value read by type (e.g. Database Hash)
                    h = struct.unpack_from("<H", pdu, i)[0]
                    values[h] = bytearray(pdu[i + 2:i + n])
        elif op == 0x05:                      # Find Info Rsp: descriptors
            fmt = pdu[1]
            n = 4 if fmt == 1 else 18
            for i in range(2, len(pdu), n):
                h = struct.unpack_from("<H", pdu, i)[0]
                descs[h] = pdu[i + 2:i + n]
        elif op in (0x0B, 0x0D) and pending and pending[1] is not None:
            if op == 0x0B:
                values[pending[1]] = bytearray(pdu[1:])
            else:
                values[pending[1]] += pdu[1:]
        elif op == 0x01:
            req, h, code = pdu[1], struct.unpack_from("<H", pdu, 2)[0], pdu[4]
            errors.append((t, ATT_OP.get(req, req), h, code))
        elif op in (0x1B, 0x1D):
            h = struct.unpack_from("<H", pdu, 1)[0]
            notifs.append((t, h, pdu[3:]))

    pr("== %s, adapter %d, LE handle 0x%04x" % (a.pcap, a.adapter, a.handle))
    pr("L2CAP channels: %s (0x0004 ATT, 0x0005 LE signalling, 0x0006 SMP = pairing, not printed)"
       % {("0x%04x" % k): v for k, v in cids.items()})
    pr("ATT ops: %s" % ", ".join("%s %s x%d" % (d, o, n) for (d, o), n in sorted(ops.items())))

    # report reference: descriptor handle -> characteristic (nearest char value handle below)
    def owner(dh):
        c = [vh for vh in chars if vh < dh]
        return max(c) if c else None

    pr("\n== GATT table")
    for s, e, u in sorted(services):
        pr("SERVICE 0x%04x-0x%04x  %s" % (s, e, uuid_str(u)))
        for vh in sorted(v for v in chars if s <= v <= e):
            dh, props, cu = chars[vh]
            pr("  CHAR value 0x%04x  %-38s [%s]" % (vh, uuid_str(cu),
               ",".join(p for i, p in enumerate(PROPS) if props >> i & 1)))
            if vh in values:
                v = bytes(values[vh])
                asc = v.decode("ascii", "replace") if v and all(31 < x < 127 for x in v) else ""
                pr("      value (%d B): %s%s" % (len(v), v[:48].hex(" "), " ..." if len(v) > 48 else ""))
                if asc:
                    pr("      ascii: %r" % asc)
            for dh2 in sorted(h for h in descs if s <= h <= e and owner(h) == vh and h != vh):
                du = descs[dh2]
                line = "      DESC 0x%04x %s" % (dh2, uuid_str(du))
                if dh2 in values:
                    v = bytes(values[dh2])
                    line += " = %s" % v.hex(" ")
                    if du == b"\x08\x29" and len(v) >= 2:
                        line += "   -> report ID 0x%02x %s" % (v[0], RTYPE.get(v[1], v[1]))
                pr(line)

    rm = [vh for vh, (_, _, cu) in chars.items() if cu == b"\x4b\x2a"]
    for vh in rm:
        v = bytes(values.get(vh, b""))
        pr("\n== REPORT MAP (handle 0x%04x, %d bytes)" % (vh, len(v)))
        for k in range(0, len(v), 16):
            pr("   " + v[k:k + 16].hex(" "))
        if v:
            import dump_caps
            buf = []
            dump_caps.out = buf.append
            tot = dump_caps.parse_descriptor(list(v))
            out.extend(buf)
            for (typ, rid), bits in sorted(tot.items(), key=lambda x: (x[0][1], x[0][0])):
                pr("   %-7s id=0x%02x %4d bits = %d bytes" % (typ, rid, bits, (bits + 7) // 8))

    pr("\n== ATT errors: %d" % len(errors))
    for t, req, h, code in errors[:30]:
        pr("   %7.2f %s handle 0x%04x code 0x%02x" % (t, req, h, code))
    pr("\n== writes host->dev: %d" % len(writes))
    for t, op, h, v in writes:
        pr("   %7.2f %s 0x%04x = %s" % (t, op, h, v.hex(" ")))
    pr("\n== notifications dev->host: %d" % len(notifs))
    for t, h, v in notifs[:40]:
        pr("   %7.2f 0x%04x: %s" % (t, h, v.hex(" ")))

    path = os.path.join(LOGDIR, "btgatt_%s.txt" % os.path.splitext(os.path.basename(a.pcap))[0])
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(out) + "\n")
    print("\n".join(out))
    print("\nlog: %s" % path)


if __name__ == "__main__":
    sys.exit(main())

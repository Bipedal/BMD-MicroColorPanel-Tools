"""Task 1: enumerate the Micro Color Panel, dump device info, Windows HidP caps
(report lengths, per-report-ID value/button caps) and the report descriptor.

Read-only: sends NO reports to the panel.
Log: logs/dump_caps_<timestamp>.txt
"""
import ctypes
import ctypes.wintypes as wt
import os
import struct
import sys
import time

import hid

VID, PID = 0x1EDB, 0xDA0F
ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
LOGDIR = os.path.join(ROOT, "logs")
os.makedirs(LOGDIR, exist_ok=True)
LOGPATH = None
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
_log = None   # opened in main(), so other scripts can import parse_descriptor
errors = []


def out(s=""):
    print(s)
    if _log:
        _log.write(s + "\n")


def err(where, e):
    msg = "ERROR %s: %r" % (where, e)
    errors.append(msg)
    out(msg)


# ---------------------------------------------------------------- Win32 HidP
k32 = ctypes.WinDLL("kernel32", use_last_error=True)
hdll = ctypes.WinDLL("hid", use_last_error=True)
k32.CreateFileW.restype = wt.HANDLE
k32.CreateFileW.argtypes = [wt.LPCWSTR, wt.DWORD, wt.DWORD, ctypes.c_void_p,
                            wt.DWORD, wt.DWORD, wt.HANDLE]
k32.CloseHandle.argtypes = [wt.HANDLE]
hdll.HidD_GetPreparsedData.argtypes = [wt.HANDLE, ctypes.POINTER(ctypes.c_void_p)]
hdll.HidD_FreePreparsedData.argtypes = [ctypes.c_void_p]
hdll.HidP_GetCaps.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
hdll.HidP_GetCaps.restype = ctypes.c_long
hdll.HidP_GetValueCaps.argtypes = [ctypes.c_int, ctypes.c_void_p,
                                   ctypes.POINTER(ctypes.c_ushort), ctypes.c_void_p]
hdll.HidP_GetValueCaps.restype = ctypes.c_long
hdll.HidP_GetButtonCaps.argtypes = hdll.HidP_GetValueCaps.argtypes
hdll.HidP_GetButtonCaps.restype = ctypes.c_long
HIDP_OK = 0x00110000
INVALID = ctypes.c_void_p(-1).value
RTYPES = ("Input", "Output", "Feature")

CAPS_FIELDS = ["Usage", "UsagePage", "InputReportByteLength",
               "OutputReportByteLength", "FeatureReportByteLength"] + \
              ["_r%d" % i for i in range(17)] + \
              ["NumberLinkCollectionNodes",
               "NumberInputButtonCaps", "NumberInputValueCaps", "NumberInputDataIndices",
               "NumberOutputButtonCaps", "NumberOutputValueCaps", "NumberOutputDataIndices",
               "NumberFeatureButtonCaps", "NumberFeatureValueCaps", "NumberFeatureDataIndices"]


def hidp_dump(path):
    """Open the collection with zero access (allowed for caps queries)."""
    h = k32.CreateFileW(path, 0, 3, None, 3, 0, None)
    if h in (None, INVALID):
        err("CreateFileW(%s)" % path, ctypes.WinError(ctypes.get_last_error()))
        return None
    pp = ctypes.c_void_p()
    try:
        if not hdll.HidD_GetPreparsedData(h, ctypes.byref(pp)):
            err("HidD_GetPreparsedData", ctypes.WinError(ctypes.get_last_error()))
            return None
        buf = ctypes.create_string_buffer(64)
        st = hdll.HidP_GetCaps(pp, buf)
        if st != HIDP_OK:
            err("HidP_GetCaps", "NTSTATUS 0x%08x" % (st & 0xFFFFFFFF))
            return None
        caps = dict(zip(CAPS_FIELDS, struct.unpack("<32H", buf.raw)))
        out("  HidP_GetCaps:")
        out("    top-level usage page 0x%04x usage 0x%04x"
            % (caps["UsagePage"], caps["Usage"]))
        for r in RTYPES:
            out("    %-7s report byte length (incl. ID byte): %d"
                % (r, caps[r + "ReportByteLength"]))
        out("    link collection nodes: %d" % caps["NumberLinkCollectionNodes"])
        for r in RTYPES:
            out("    %-7s button caps %d, value caps %d, data indices %d"
                % (r, caps["Number%sButtonCaps" % r], caps["Number%sValueCaps" % r],
                   caps["Number%sDataIndices" % r]))
        for ti, r in enumerate(RTYPES):
            _value_caps(pp, ti, r, caps["Number%sValueCaps" % r])
            _button_caps(pp, ti, r, caps["Number%sButtonCaps" % r])
        return caps
    finally:
        if pp.value:
            hdll.HidD_FreePreparsedData(pp)
        k32.CloseHandle(h)


def _value_caps(pp, ti, r, n):
    if not n:
        return
    buf = ctypes.create_string_buffer(72 * n)
    ln = ctypes.c_ushort(n)
    st = hdll.HidP_GetValueCaps(ti, buf, ctypes.byref(ln), pp)
    if st != HIDP_OK:
        err("HidP_GetValueCaps(%s)" % r, "NTSTATUS 0x%08x" % (st & 0xFFFFFFFF))
        return
    for i in range(ln.value):
        b = buf.raw[72 * i: 72 * (i + 1)]
        up, rid, alias, bitfield = struct.unpack_from("<HBBH", b, 0)
        is_range = b[12]
        bitsize, rcount = struct.unpack_from("<HH", b, 18)
        lmin, lmax, pmin, pmax = struct.unpack_from("<4i", b, 40)
        u = struct.unpack_from("<8H", b, 56)
        usage = ("0x%04x-0x%04x" % (u[0], u[1])) if is_range else ("0x%04x" % u[0])
        di = ("%d-%d" % (u[6], u[7])) if is_range else str(u[6])
        out("      %s VALUE  id=0x%02x page=0x%04x usage=%s bits=%d count=%d "
            "logical=[%d,%d] physical=[%d,%d] flags=0x%04x dataidx=%s"
            % (r, rid, up, usage, bitsize, rcount, lmin, lmax, pmin, pmax,
               bitfield, di))


def _button_caps(pp, ti, r, n):
    if not n:
        return
    buf = ctypes.create_string_buffer(72 * n)
    ln = ctypes.c_ushort(n)
    st = hdll.HidP_GetButtonCaps(ti, buf, ctypes.byref(ln), pp)
    if st != HIDP_OK:
        err("HidP_GetButtonCaps(%s)" % r, "NTSTATUS 0x%08x" % (st & 0xFFFFFFFF))
        return
    for i in range(ln.value):
        b = buf.raw[72 * i: 72 * (i + 1)]
        up, rid, alias, bitfield = struct.unpack_from("<HBBH", b, 0)
        is_range = b[12]
        rcount = struct.unpack_from("<H", b, 16)[0]
        u = struct.unpack_from("<8H", b, 56)
        usage = ("0x%04x-0x%04x" % (u[0], u[1])) if is_range else ("0x%04x" % u[0])
        out("      %s BUTTON id=0x%02x page=0x%04x usage=%s count=%d flags=0x%04x"
            % (r, rid, up, usage, rcount, bitfield))


# ---------------------------------------------------- descriptor short-item parser
MAIN = {0x8: "Input", 0x9: "Output", 0xB: "Feature", 0xA: "Collection", 0xC: "EndCollection"}
GLOBAL = {0x0: "UsagePage", 0x1: "LogicalMin", 0x2: "LogicalMax", 0x3: "PhysicalMin",
          0x4: "PhysicalMax", 0x5: "UnitExp", 0x6: "Unit", 0x7: "ReportSize",
          0x8: "ReportID", 0x9: "ReportCount", 0xA: "Push", 0xB: "Pop"}
LOCAL = {0x0: "Usage", 0x1: "UsageMin", 0x2: "UsageMax"}


def parse_descriptor(d):
    """Print items and return {(type, report_id): total_bits}."""
    totals = {}
    g = {"ReportSize": 0, "ReportCount": 0, "ReportID": 0}
    stack, i, depth = [], 0, 0
    while i < len(d):
        p = d[i]
        if p == 0xFE:  # long item
            n = d[i + 1]
            out("    %04x  long item (%d bytes)" % (i, n))
            i += 3 + n
            continue
        size = (0, 1, 2, 4)[p & 3]
        typ = (p >> 2) & 3
        tag = p >> 4
        raw = d[i + 1: i + 1 + size]
        val = int.from_bytes(bytes(raw), "little") if size else 0
        sval = int.from_bytes(bytes(raw), "little", signed=True) if size else 0
        hexb = " ".join("%02x" % x for x in d[i: i + 1 + size])
        if typ == 0:
            name = MAIN.get(tag, "Main?%x" % tag)
            if name == "EndCollection":
                depth = max(0, depth - 1)
            extra = ""
            if name in ("Input", "Output", "Feature"):
                bits = g["ReportSize"] * g["ReportCount"]
                key = (name, g["ReportID"])
                totals[key] = totals.get(key, 0) + bits
                extra = " (%s) size=%d count=%d -> %d bits" % (
                    "Const" if val & 1 else ("Var" if val & 2 else "Array"),
                    g["ReportSize"], g["ReportCount"], bits)
            out("    %04x  %-18s %s%s 0x%x%s" % (i, hexb, "  " * depth, name, val, extra))
            if name == "Collection":
                depth += 1
        elif typ == 1:
            name = GLOBAL.get(tag, "Global?%x" % tag)
            if name == "Push":
                stack.append(dict(g))
            elif name == "Pop" and stack:
                g = stack.pop()
            else:
                g[name] = val
            v = sval if name in ("LogicalMin", "LogicalMax", "PhysicalMin",
                                 "PhysicalMax") else val
            out("    %04x  %-18s %s%s %s" % (i, hexb, "  " * depth, name,
                                           ("0x%02x" % v) if name in ("ReportID", "UsagePage") else v))
        else:
            name = LOCAL.get(tag, ("Local?%x" % tag) if typ == 2 else "Reserved")
            out("    %04x  %-18s %s%s 0x%x" % (i, hexb, "  " * depth, name, val))
        i += 1 + size
    return totals


# ------------------------------------------------------------------------ main
def main():
    global _log, LOGPATH
    LOGPATH = os.path.join(LOGDIR, time.strftime("dump_caps_" + os.environ.get("PANEL_TRANSPORT", "usb").lower() + "_%Y%m%d_%H%M%S.txt"))
    _log = open(LOGPATH, "w", encoding="utf-8")
    out("dump_caps  %s  python %s  hidapi %s" % (
        time.strftime("%Y-%m-%d %H:%M:%S"), sys.version.split()[0],
        getattr(hid, "__version__", "?")))
    devs = hid.enumerate(VID, PID)
    if not devs:
        out("No device %04x:%04x found." % (VID, PID))
        return 1
    out("\n== enumerate(%04x:%04x): %d entries" % (VID, PID, len(devs)))
    for d in devs:
        out("  iface=%s  usage_page=0x%04x usage=0x%04x  release=0x%04x  bus=%s"
            % (d.get("interface_number"), d.get("usage_page") or 0, d.get("usage") or 0,
               d.get("release_number") or 0, d.get("bus_type")))
        out("    product=%r mfr=%r serial=%r" % (d.get("product_string"),
            d.get("manufacturer_string"), d.get("serial_number")))
        out("    path=%s" % d["path"].decode(errors="replace"))

    from pcommon import TRANSPORT, is_bt     # PANEL_TRANSPORT=bt selects the BLE collection
    if TRANSPORT == "bt":
        targets = [d for d in devs if is_bt(d)]
    else:
        targets = [d for d in devs if not is_bt(d) and (d.get("interface_number") == 2
                   or b"mi_02" in d["path"].lower())]
    out("transport: %s" % TRANSPORT)
    out("\n== interface 2 collections: %d" % len(targets))
    summary = []
    for d in targets:
        path = d["path"].decode(errors="replace")
        out("\n-- %s" % path)
        caps = hidp_dump(path)
        dev = hid.device()
        try:
            dev.open_path(d["path"])
        except Exception as e:
            err("open_path", e)
            continue
        try:
            for fn in ("get_manufacturer_string", "get_product_string",
                       "get_serial_number_string"):
                try:
                    out("  %s: %r" % (fn, getattr(dev, fn)()))
                except Exception as e:
                    err(fn, e)
            try:
                desc = dev.get_report_descriptor()
            except Exception as e:
                err("get_report_descriptor", e)
                desc = None
            if desc:
                desc = list(desc)
                out("  report descriptor: %d bytes (on Windows hidapi rebuilds this "
                    "from preparsed data; it is equivalent, not byte-identical)" % len(desc))
                for k in range(0, len(desc), 16):
                    out("    " + " ".join("%02x" % x for x in desc[k:k + 16]))
                out("  parsed:")
                totals = parse_descriptor(desc)
                out("  per report ID (payload bits -> bytes, +1 for ID byte if ID != 0):")
                for (typ, rid), bits in sorted(totals.items(), key=lambda x: (x[0][1], x[0][0])):
                    line = "    %-7s id=0x%02x  %4d bits = %3d bytes payload" % (
                        typ, rid, bits, (bits + 7) // 8)
                    out(line)
                    summary.append(line.strip())
        finally:
            dev.close()
        if caps:
            summary.append("caps lengths: in=%d out=%d feat=%d" % (
                caps["InputReportByteLength"], caps["OutputReportByteLength"],
                caps["FeatureReportByteLength"]))

    out("\n== SUMMARY")
    for s in summary:
        out("  " + s)
    out("  errors: %d" % len(errors))
    for e in errors:
        out("    " + e)
    out("log: %s" % LOGPATH)
    return 1 if errors else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    finally:
        if _log:
            _log.close()

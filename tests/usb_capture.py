"""Capture USB traffic of the panel only (USBPcap via tshark).

Usage: python tests/usb_capture.py --name resolve [--addr 63] [--secs 1200]

Only device address --addr is captured (plus devices that connect during the run,
which is how a replug of the panel is caught; its address may change). Nothing
from other devices (keyboard etc.) is recorded.
Output: logs/usb_<name>_<ts>.pcapng ; decode with tests/usb_decode.py.
Stops after --secs, or when this process is stopped (the file stays readable).
"""
import argparse
import os
import subprocess
import sys

from pcommon import log_path

TSHARK = r"C:\Program Files\Wireshark\tshark.exe"
IFACE = "USBPcap1"
PREF = "extcap.____usbpcap1."


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--addr", type=int, default=63, help="panel USB device address")
    ap.add_argument("--secs", type=int, default=1200)
    a = ap.parse_args()
    out = log_path("usb_%s" % a.name, "pcapng")
    cmd = [TSHARK, "-i", IFACE,
           "-o", PREF + "capturefromalldevices:false",
           "-o", PREF + "capturefromnewdevices:true",
           "-o", PREF + "injectdescriptors:true",
           "-o", PREF + "devices:%d" % a.addr,
           "-a", "duration:%d" % a.secs, "-w", out, "-q"]
    print("capturing device %d on %s for up to %d s -> %s" % (a.addr, IFACE, a.secs, out),
          flush=True)
    rc = subprocess.call(cmd)
    print("tshark exit %d; file %s (%d bytes)" % (rc, out,
          os.path.getsize(out) if os.path.exists(out) else -1))
    return rc


if __name__ == "__main__":
    sys.exit(main())

"""Capture USB traffic of the panel only (USBPcap via tshark).

Usage: python tests/usb_capture.py --list                  # USB addresses; marks the panel
       python tests/usb_capture.py --name resolve --addr N [--secs 1200]

Only device address --addr is captured (plus devices that connect during the run).
Find the address with --list: the panel is a "[N] USB Composite Device" whose child is
"Blackmagic DaVinci Resolve Micro Color Panel". Addresses change on replug and are
reused (63, 64, later 40). The script refuses an address that is not the panel.
Lesson (session 3, 2026-11-17): a placeholder address plus "capture new devices",
stopped by killing the process, recorded nothing at all.
Output: logs/usb_<name>_<ts>.pcapng ; decode with tests/usb_decode.py.
Stops after --secs, or cleanly when the file logs/STOP_CAPTURE appears: tshark then gets
Ctrl+Break and closes the file. Killing this process instead can lose unwritten data.
"""
import argparse
import os
import re
import signal
import subprocess
import sys
import time

from pcommon import LOGDIR, log_path

TSHARK = r"C:\Program Files\Wireshark\tshark.exe"
USBPCAPCMD = r"C:\Program Files\Wireshark\extcap\USBPcapCMD.exe"
IFACE = "USBPcap1"
PREF = "extcap.____usbpcap1."
STOPFILE = os.path.join(LOGDIR, "STOP_CAPTURE")
PANEL_NAME = "Blackmagic DaVinci Resolve Micro Color Panel"


def list_devices():
    """[(address, name, is_panel)] for the numbered devices on the root hub."""
    out = subprocess.run([USBPCAPCMD, "--extcap-interface", r"\\.\%s" % IFACE,
                          "--extcap-config"], capture_output=True, text=True).stdout
    devs, panel = [], set()
    for line in out.splitlines():
        m = re.search(r"\{value=(\d+)\}\{display=\[\d+\] ([^}]*)\}", line)
        if m:
            devs.append((int(m.group(1)), m.group(2)))
        if "display=%s}" % PANEL_NAME in line:
            m = re.search(r"parent=(\d+)\}", line)
            if m:
                panel.add(int(m.group(1)))
    return [(a, n, a in panel) for a, n in devs]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name")
    ap.add_argument("--addr", type=int, help="panel USB device address (see --list)")
    ap.add_argument("--secs", type=int, default=1200)
    ap.add_argument("--list", action="store_true")
    a = ap.parse_args()
    devs = list_devices()
    panel = [d for d, _, p in devs if p]
    if a.list:
        for d, n, p in devs:
            print("  [%3d] %s%s" % (d, n, "   <- Micro Color Panel" if p else ""))
        return 0
    if not a.name or a.addr is None:
        ap.error("--name and --addr are required (find the address with --list)")
    if a.addr not in panel:
        print("!! address %d is not the panel (panel on USB: %s); refusing" % (
            a.addr, panel or "not found"))
        return 2
    if os.path.exists(STOPFILE):
        os.remove(STOPFILE)
    out = log_path("usb_%s" % a.name, "pcapng")
    cmd = [TSHARK, "-i", IFACE,
           "-o", PREF + "capturefromalldevices:false",
           "-o", PREF + "capturefromnewdevices:true",
           "-o", PREF + "injectdescriptors:true",
           "-o", PREF + "devices:%d" % a.addr,
           "-a", "duration:%d" % a.secs, "-w", out, "-q"]
    print("capturing device %d on %s for up to %d s -> %s" % (a.addr, IFACE, a.secs, out),
          flush=True)
    proc = subprocess.Popen(cmd, creationflags=getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0))
    while proc.poll() is None:
        if os.path.exists(STOPFILE):
            print("stop file seen: sending Ctrl+Break to tshark", flush=True)
            proc.send_signal(signal.CTRL_BREAK_EVENT)
            try:
                proc.wait(15)
            except subprocess.TimeoutExpired:
                print("!! tshark did not stop within 15 s; killing it (file may be short)")
                proc.kill()
            os.remove(STOPFILE)
            break
        time.sleep(0.5)
    rc = proc.wait()
    print("tshark exit %d; file %s (%d bytes)" % (rc, out,
          os.path.getsize(out) if os.path.exists(out) else -1))
    return rc


if __name__ == "__main__":
    sys.exit(main())

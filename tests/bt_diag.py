"""Why does Bluetooth connect but not answer? Read-only: nothing is written to the panel.

Usage: python tests/bt_diag.py [--wait 60] [--rounds 3]
       python tests/bt_diag.py --offline            # checks the judging with made-up results
       needs the winrt packages of vendor_gatt.py, and hidapi

The helper (and Resolve) must not be running: they would use the link too.
The panel must be unplugged from USB.

What it reads, with the time each read takes (none waits longer than 12 s):
  * the panel's own Bluetooth chip's data: device name, maker, PnP id
  * values that come from the panel itself: the vendor service's firmware (feature 0x01) and
    battery (feature 0x08) mirrors, and HID feature 0x01
If the first kind answers and the second doesn't, the link is fine and the panel behind its
Bluetooth chip isn't answering. If nothing answers, the link itself (or its encryption) is at fault.
The BT address is used to connect but never printed or logged.
Log: logs/bt_diag_<ts>.jsonl
"""
import argparse
import asyncio
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pcommon import ROOT, JsonLog, hexs, log_path  # noqa: E402

VENDOR = "1dce0a9a-d84d-4a9f-878c-b48f0fbe12d3"
READS = [  # (name, kind, service uuid, characteristic uuid)
    ("device name", "chip", "00001800-0000-1000-8000-00805f9b34fb", "00002a00-0000-1000-8000-00805f9b34fb"),
    ("maker", "chip", "0000180a-0000-1000-8000-00805f9b34fb", "00002a29-0000-1000-8000-00805f9b34fb"),
    ("PnP id", "chip", "0000180a-0000-1000-8000-00805f9b34fb", "00002a50-0000-1000-8000-00805f9b34fb"),
    ("vendor firmware", "panel", VENDOR, "5b71420f-565b-4992-883d-b154f39c5cfc"),
    ("vendor battery", "panel", VENDOR, "6d121cc9-4e98-41c6-9b18-b8dfcc75090c"),
]
LIMIT = 12.0


def judge(results, connected):
    """results: [(name, kind, ok, seconds, detail)]"""
    if not connected:
        return "The panel never connected: it is not reaching this PC at all (asleep, paired elsewhere, or its pairing is gone)."
    ok = {k: any(r[2] for r in results if r[1] == k) for k in ("chip", "panel", "hid")}
    if ok["panel"] and ok["hid"]:
        return "Everything answers: Bluetooth works when nothing else uses it."
    if ok["panel"] and not ok["hid"]:
        return "The vendor service answers but HID doesn't: Windows' HID driver or its bond is at fault."
    if ok["chip"]:
        return "The panel's Bluetooth chip answers, the panel behind it doesn't: the panel's firmware is stuck."
    if any(r[2] for r in results):
        return "Mixed result: see the log."
    return "Connected, but nothing answers: the link or its encryption (pairing) is at fault."


async def read_one(dev, svc, ch):
    import uuid
    from winrt.windows.devices.bluetooth import BluetoothCacheMode as C
    from winrt.windows.devices.bluetooth.genericattributeprofile import GattCommunicationStatus as S
    from winrt.windows.storage.streams import DataReader
    # the table may come from the cache; the value must not
    sr = await dev.get_gatt_services_for_uuid_with_cache_mode_async(uuid.UUID(svc), C.CACHED)
    if sr.status != S.SUCCESS or sr.services.size == 0:
        return False, "service: status %s, %d found" % (sr.status, sr.services.size if sr.services else 0)
    service = sr.services.get_at(0)
    try:
        cr = await service.get_characteristics_for_uuid_with_cache_mode_async(uuid.UUID(ch), C.CACHED)
        if cr.status != S.SUCCESS or cr.characteristics.size == 0:
            return False, "characteristic: status %s" % cr.status
        r = await cr.characteristics.get_at(0).read_value_with_cache_mode_async(C.UNCACHED)
        if r.status != S.SUCCESS:
            return False, "read: status %s, error %s" % (r.status, r.protocol_error)
        rd = DataReader.from_buffer(r.value)
        return True, hexs(bytes(rd.read_byte() for _ in range(r.value.length)))
    finally:
        service.close()


def hid_read():
    """HID feature 0x01 over Bluetooth (exact size 8)."""
    import hid
    for d in hid.enumerate(0x1EDB, 0xDA0F):
        if d.get("bus_type") == 2 or b"00001812" in d["path"].lower():
            h = hid.device()
            h.open_path(d["path"])
            try:
                return True, hexs(bytes(h.get_feature_report(0x01, 8)))
            finally:
                h.close()
    return False, "no HID entry over Bluetooth"


async def main(a, log):
    from winrt.windows.devices.bluetooth import BluetoothConnectionStatus, BluetoothLEDevice
    addr = json.load(open(os.path.join(ROOT, "private_ids.json"), encoding="utf-8"))["bt_address"].replace(":", "")
    dev = await BluetoothLEDevice.from_bluetooth_address_async(int(addr, 16))
    if dev is None:
        print("Windows does not know this panel over Bluetooth: it is not paired.")
        log.write(t="unpaired")
        return
    paired = dev.device_information.pairing.is_paired
    t0 = time.monotonic()
    changes = []

    def changed(d, _):
        s = "connected" if d.connection_status == BluetoothConnectionStatus.CONNECTED else "disconnected"
        changes.append((round(time.monotonic() - t0, 2), s))
        log.write(t="link", rel=changes[-1][0], state=s)
    tok = dev.add_connection_status_changed(changed)
    print("paired: %s, connected now: %s" % (paired, dev.connection_status == BluetoothConnectionStatus.CONNECTED))
    log.write(t="start", paired=paired, connected=dev.connection_status == BluetoothConnectionStatus.CONNECTED)
    results, connected = [], False
    try:
        while time.monotonic() - t0 < a.wait:
            if dev.connection_status == BluetoothConnectionStatus.CONNECTED:
                connected = True
                break
            await asyncio.sleep(0.2)
        print("connected after %.1f s" % (time.monotonic() - t0) if connected else "not connected within %d s" % a.wait)
        await asyncio.sleep(3)          # let Windows finish its own setup first
        for rnd in range(a.rounds if connected else 0):
            for name, kind, svc, ch in READS + [("HID firmware", "hid", None, None)]:
                t = time.monotonic()
                try:
                    if kind == "hid":
                        ok, what = await asyncio.wait_for(asyncio.to_thread(hid_read), LIMIT)
                    else:
                        ok, what = await asyncio.wait_for(read_one(dev, svc, ch), LIMIT)
                except asyncio.TimeoutError:
                    ok, what = False, "no answer within %d s" % LIMIT
                except Exception as e:
                    ok, what = False, "%s: %s" % (type(e).__name__, e)
                dt = round(time.monotonic() - t, 2)
                results.append((name, kind, ok, dt, what))
                log.write(t="read", round=rnd, name=name, kind=kind, ok=ok, seconds=dt, detail=what)
                print("  %d %-16s %-4s %6.2f s  %s" % (rnd + 1, name, "ok" if ok else "FAIL", dt, what if not ok or kind != "chip" else "(%d bytes)" % len(what.split())))
            await asyncio.sleep(5)
    finally:
        dev.remove_connection_status_changed(tok)
        dev.close()
    print("link changes:", changes or "none")
    v = judge(results, connected)
    log.write(t="verdict", text=v, changes=changes)
    print("=> " + v)


def offline():
    r = lambda c, p, h: [("n", "chip", c, 0.1, ""), ("f", "panel", p, 0.1, ""), ("h", "hid", h, 0.1, "")]
    want = [((r(1, 1, 1), True), "Everything"), ((r(1, 1, 0), True), "The vendor service answers"), ((r(1, 0, 0), True), "The panel's Bluetooth chip"),
            ((r(0, 0, 0), True), "Connected, but nothing"), (([], False), "The panel never connected"), ((r(0, 0, 1), True), "Mixed")]
    bad = [w for a, w in want if not judge(*a).startswith(w)]
    print("offline: %d of %d ok" % (len(want) - len(bad), len(want)), bad or "")
    return 1 if bad else 0


if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--wait", type=int, default=60)
    ap.add_argument("--rounds", type=int, default=3)
    ap.add_argument("--offline", action="store_true")
    args = ap.parse_args()
    if args.offline:
        sys.exit(offline())
    L = JsonLog(log_path("bt_diag", "jsonl"))
    try:
        asyncio.run(main(args, L))
    finally:
        L.close()

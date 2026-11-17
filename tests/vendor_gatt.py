"""Session 5: the Blackmagic vendor GATT service (spec §11.1, §9 #9). BT only.

Usage: python tests/vendor_gatt.py [--address AABBCC...]      # default: private_ids.json
       (uses the Windows WinRT GATT API: bleak cannot open an already-connected device)
       needs: pip install winrt-runtime winrt-windows-devices-bluetooth
              winrt-windows-devices-bluetooth-genericattributeprofile winrt-windows-storage-streams
       (not in requirements.txt: only this test uses them)
       python tests/vendor_gatt.py --offline

The vendor service 1DCE0A9A-... has 11 characteristics that mirror the HID reports
(logs/btgatt_pairing_20261116_190313.txt). Does it behave like HID? Timed run, ~60 s:
   0 s  read every vendor characteristic; subscribe to its 4 notify characteristics;
        open the HID side too (hidapi) and count its reports. NOTHING is enabled yet.
        You: roll a ball and press keys continuously until the backlight dims (~30 s).
  15 s  write the 0x0A mirror = 01 (the vendor-side "0a 01")
  30 s  write the 0x03 mirror = 0a 0a  (backlight 10 %)  -> backlight dims?
  36 s  write the 0x03 mirror = 64 64  (backlight 100 %)
  42 s  write the 0x04 mirror = OFFSET + BYPASS          -> those two LEDs light?
  48 s  write the 0x04 mirror = all off
  52 s  read the 0x0A / 0x03 mirrors again, and HID GET 0x0A; end
Never written: the 0x07 mirror (unknown, spec §2), the version and battery mirrors,
output 0x02. The BT address is used to connect but never printed or logged.
Log: logs/vendor_gatt_<ts>.jsonl
"""
import argparse
import asyncio
import json
import os
import sys
import time
from collections import Counter

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from pcommon import ROOT, JsonLog, hexs, log_path  # noqa: E402

SERVICE = "1dce0a9a-d84d-4a9f-878c-b48f0fbe12d3"
# uuid -> (mirror of, may we write it?)
CHARS = {
    "5b71420f-565b-4992-883d-b154f39c5cfc": ("feature 0x01", False),
    "cd343346-81a6-492d-91dd-ff0eae5a8365": ("input 0x02", False),
    "8e073915-1cfb-413a-b82d-60aba1f39348": ("output 0x02", False),
    "41ae032b-ed25-44cb-93ec-88cc0588f0a9": ("feature 0x03", True),
    "4054b034-03b4-412a-b57b-9bd062a31a41": ("output 0x04", True),
    "5a1ff3f8-c087-4762-98c0-d3318ca7d8f2": ("input 0x05", False),
    "a997aae7-5355-4e86-80a6-bd6a5e9674ce": ("input 0x06", False),
    "1afffb5d-ec03-4fde-b589-11146b49a031": ("feature 0x07", False),   # NEVER write
    "6d121cc9-4e98-41c6-9b18-b8dfcc75090c": ("feature 0x08", False),
    "8e428e8d-c9d3-434a-873e-3da62c16f4bf": ("input 0x08", False),
    "0cf74fce-fdcd-4337-892e-154c55af197a": ("feature 0x0a", True),
}
BY_NAME = {v[0]: k for k, v in CHARS.items()}
PLAN = [(15.0, "feature 0x0a", bytes([0x01]), "vendor enable"),
        (30.0, "feature 0x03", bytes([0x0A, 0x0A]), "backlight 10"),
        (36.0, "feature 0x03", bytes([0x64, 0x64]), "backlight 100"),
        (42.0, "output 0x04", bytes([0x05, 0, 0, 0, 0, 0]), "LEDs OFFSET+BYPASS"),
        (48.0, "output 0x04", bytes(6), "LEDs off")]
END = 52.0
for _, n, _, _ in PLAN:
    assert CHARS[BY_NAME[n]][1], n          # only whitelisted characteristics are written


async def run(client, hiddev, log, clock=time.monotonic, sleep=asyncio.sleep, say=print):
    t0 = clock()
    rel = lambda: round(clock() - t0, 2)
    notes = Counter()
    first_note = {}
    hid_n = Counter()
    errors = []

    def on_note(uuid):
        name = CHARS[uuid][0]

        def cb(_sender, data):
            t = rel()
            notes[(name, "before" if t < PLAN[0][0] else "after")] += 1
            first_note.setdefault(name, (t, hexs(data[:12])))
            log.write(t="notify", ch=name, rel=t, data=hexs(bytes(data)[:40]))
        return cb

    initial = {}
    for uuid, (name, _) in CHARS.items():
        try:
            v = bytes(await client.read_gatt_char(uuid))
            initial[name] = hexs(v)
        except Exception as e:
            initial[name] = "ERR %r" % e
            errors.append(("read", name, repr(e)))
    log.write(t="initial", values=initial, rel=rel())
    say("%6.1f s  read %d vendor characteristics" % (clock() - t0, len(initial)))
    for uuid, (name, _) in CHARS.items():
        if name.startswith("input"):
            try:
                await client.start_notify(uuid, on_note(uuid))
            except Exception as e:
                errors.append(("subscribe", name, repr(e)))
    say("%6.1f s  subscribed; roll a ball and press keys" % (clock() - t0))
    writes = []
    for at, name, val, why in PLAN + [(END, None, None, "end")]:
        while clock() - t0 < at:
            if hiddev is not None:
                try:
                    d = hiddev.read(65)
                    while d:
                        hid_n["before" if rel() < PLAN[0][0] else "after"] += 1
                        d = hiddev.read(65)
                except Exception as e:
                    errors.append(("hid read", "", repr(e)))
                    hiddev = None
            await sleep(0.02)
        if name is None:
            break
        uuid = BY_NAME[name]
        try:
            await client.write_gatt_char(uuid, val, response=True)
            ok, err = True, None
        except Exception as e:
            ok, err = False, repr(e)
            errors.append(("write", name, err))
        writes.append((round(at), name, hexs(val), ok))
        log.write(t="write", ch=name, data=hexs(val), why=why, ok=ok, err=err, rel=rel())
        say("%6.1f s  write %s = %s (%s): %s" % (clock() - t0, name, hexs(val), why,
                                                  "ok" if ok else "FAILED " + err))
    final = {}
    for name in ("feature 0x0a", "feature 0x03"):
        try:
            final[name] = hexs(bytes(await client.read_gatt_char(BY_NAME[name])))
        except Exception as e:
            final[name] = "ERR %r" % e
    hid_flag = None
    if hiddev is not None:
        try:
            hid_flag = hexs(bytes(hiddev.get_feature_report(0x0A, 2)))
        except Exception as e:
            hid_flag = "ERR %r" % e
    res = dict(t="result", initial=initial, final=final, hid_flag_end=hid_flag,
               notes={"%s/%s" % k: v for k, v in notes.items()}, first_note=first_note,
               hid_reports=dict(hid_n), writes=writes, errors=errors, end=rel())
    log.write(**res)
    return res


def report(r, say=print):
    say("\n== result (%.0f s)" % r["end"])
    say("   initial vendor values:")
    for k, v in r["initial"].items():
        say("      %-13s %s" % (k, v[:60]))
    say("   notifications (vendor), before/after the vendor enable at 15 s: %s" % (r["notes"] or "none"))
    say("   HID reports (hidapi), before/after: %s" % (r["hid_reports"] or "none"))
    say("   writes: %s" % r["writes"])
    say("   at end: vendor 0x0a=%s, vendor 0x03=%s, HID GET 0x0a=%s" % (
        r["final"].get("feature 0x0a"), r["final"].get("feature 0x03"), r["hid_flag_end"]))
    say("   errors: %s" % (r["errors"] or "none"))


# ---------------------------------------------------------------- offline
class FakeClock:
    def __init__(self):
        self.t = 0.0

    def __call__(self):
        return self.t

    async def sleep(self, s):
        self.t += s


class FakeClient:
    """Vendor writes act like HID: 0x0a mirror enables streaming on both sides."""

    def __init__(self, clock, hid):
        self.clock, self.hid, self.cbs, self.written = clock, hid, {}, []
        self.vals = {u: bytes(2) for u in CHARS}

    async def read_gatt_char(self, u):
        if CHARS[u][0] == "feature 0x07":
            return bytes([0, 0x0F, 0x41, 0, 0])
        return self.vals[u]

    async def start_notify(self, u, cb):
        self.cbs[u] = cb

    async def write_gatt_char(self, u, v, response=True):
        assert CHARS[u][1], "wrote a non-whitelisted characteristic: %s" % CHARS[u][0]
        self.written.append(CHARS[u][0])
        self.vals[u] = bytes(v)
        if CHARS[u][0] == "feature 0x0a":
            self.hid.enabled = True

    def tick(self):
        if self.hid.enabled:
            self.cbs[BY_NAME["input 0x05"]](None, bytearray(36))


class FakeHid:
    def __init__(self):
        self.enabled = False
        self.client = None

    def read(self, n):
        if self.enabled and self.client:
            self.client.tick()
            self.enabled_reads = getattr(self, "enabled_reads", 0) + 1
            if self.enabled_reads % 2:
                return [5] + [0] * 36
        return []

    def get_feature_report(self, rid, n):
        return [0x0A, 1 if self.enabled else 0]


def offline():
    log = JsonLog(log_path("vendor_gatt_OFFLINE"))
    clk = FakeClock()
    h = FakeHid()
    c = FakeClient(clk, h)
    h.client = c
    r = asyncio.run(run(c, h, log, clock=clk, sleep=clk.sleep, say=lambda m: None))
    log.close()
    report(r, say=lambda m: print("   " + m))
    ok = True

    def chk(cnd, m):
        nonlocal ok
        ok &= bool(cnd)
        print("  %s  %s" % ("PASS" if cnd else "FAIL", m))
    chk(c.written == ["feature 0x0a", "feature 0x03", "feature 0x03", "output 0x04", "output 0x04"],
        "only whitelisted writes, in plan order: %s" % c.written)
    chk("feature 0x07" not in c.written, "0x07 mirror never written")
    chk([w[0] for w in r["writes"]] == [15, 30, 36, 42, 48], "write times")
    chk(not any(k.endswith("/before") for k in r["notes"]) and r["notes"].get("input 0x05/after", 0) > 0,
        "notifications counted before/after enable: %s" % r["notes"])
    chk(r["hid_reports"].get("after", 0) > 0 and not r["hid_reports"].get("before"),
        "HID reports counted before/after: %s" % r["hid_reports"])
    chk(r["final"]["feature 0x0a"] == "01" and r["hid_flag_end"] == "0a 01", "final reads")
    chk(len(r["initial"]) == 11 and not r["errors"], "11 initial reads, no errors")
    print("offline:", "ALL PASS" if ok else "FAILURES")
    return 0 if ok else 1


class WinrtClient:
    """Minimal GATT client on the Windows WinRT API (bleak cannot open a device that is
    already connected, because it looks for advertisements; a bonded, connected panel
    does not advertise). Same three calls as bleak: read / write / start_notify."""

    async def open(self, addr_int):
        from winrt.windows.devices.bluetooth import BluetoothCacheMode, BluetoothLEDevice
        import uuid as U
        self.mode = BluetoothCacheMode.UNCACHED
        self.dev = await BluetoothLEDevice.from_bluetooth_address_async(addr_int)
        if self.dev is None:
            raise IOError("device not known to Windows")
        r = await self.dev.get_gatt_services_for_uuid_async(U.UUID(SERVICE), self.mode)
        if not r.services:
            raise IOError("vendor service not found (status %s)" % r.status)
        cr = await r.services[0].get_characteristics_async(self.mode)
        self.chars = {str(c.uuid).lower(): c for c in cr.characteristics}
        self.tokens = []
        return len(self.chars)

    async def read_gatt_char(self, u):
        from winrt.windows.devices.bluetooth.genericattributeprofile import GattCommunicationStatus
        v = await self.chars[u].read_value_async(self.mode)
        if v.status != GattCommunicationStatus.SUCCESS:
            raise IOError("read status %s" % v.status)
        return bytes(v.value)

    async def write_gatt_char(self, u, val, response=True):
        assert CHARS[u][1], "not whitelisted: %s" % CHARS[u][0]
        from winrt.windows.devices.bluetooth.genericattributeprofile import (
            GattCommunicationStatus, GattWriteOption)
        from winrt.windows.storage.streams import DataWriter
        w = DataWriter()
        w.write_bytes(bytes(val))         # WinRT wants a buffer, not a list
        res = await self.chars[u].write_value_with_result_async(
            w.detach_buffer(), GattWriteOption.WRITE_WITH_RESPONSE if response
            else GattWriteOption.WRITE_WITHOUT_RESPONSE)
        if res.status != GattCommunicationStatus.SUCCESS:
            raise IOError("write status %s, protocol error %s" % (res.status, res.protocol_error))

    async def start_notify(self, u, cb):
        from winrt.windows.devices.bluetooth.genericattributeprofile import (
            GattClientCharacteristicConfigurationDescriptorValue as CCCD, GattCommunicationStatus)
        c = self.chars[u]
        self.tokens.append((c, c.add_value_changed(
            lambda s, args: cb(s, bytes(args.characteristic_value)))))
        st = await c.write_client_characteristic_configuration_descriptor_async(CCCD.NOTIFY)
        if st != GattCommunicationStatus.SUCCESS:
            raise IOError("CCCD write status %s" % st)

    async def close(self):
        from winrt.windows.devices.bluetooth.genericattributeprofile import (
            GattClientCharacteristicConfigurationDescriptorValue as CCCD)
        for c, tok in self.tokens:
            try:
                await c.write_client_characteristic_configuration_descriptor_async(CCCD.NONE)
                c.remove_value_changed(tok)
            except Exception as e:
                print("   (unsubscribe failed: %s)" % type(e).__name__)
        self.dev.close()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--address")
    ap.add_argument("--offline", action="store_true")
    a = ap.parse_args()
    if a.offline:
        return offline()
    raw = a.address or json.load(open(os.path.join(ROOT, "private_ids.json")))["bt_address"]
    addr = int(raw.replace(":", ""), 16)
    import hid
    from pcommon import find_panel
    hd = hid.device()
    hd.open_path(find_panel("bt")["path"])
    hd.set_nonblocking(1)
    path = log_path("vendor_gatt")
    log = JsonLog(path)

    async def go():
        client = WinrtClient()
        try:
            n = await client.open(addr)
        except Exception as e:                # message only: the address is never printed
            print("!! could not open the vendor service: %s" % type(e).__name__)
            return None
        print("connected; vendor service with %d characteristics" % n, flush=True)
        log.write(t="service", characteristics=n)
        try:
            return await run(client, hd, log, say=lambda m: print(m, flush=True))
        finally:
            await client.close()
    try:
        r = asyncio.run(go())
    finally:
        hd.close()
        log.close()
    if r:
        report(r)
    print("log: %s" % path)
    return 0 if r else 1


if __name__ == "__main__":
    sys.exit(main())

"""LED / backlight probe. Writes ONLY reports Resolve itself writes:
output 0x04, output 0x02 (interrupt OUT), feature 0x03, feature 0x0a. Never 0x07.
On any exit restores Resolve's defaults: 0x04 = 0, 0x02 = 0, brightness 100.

Usage: python tests/led_probe.py look|look2|walk
  look : stage 1 patterns P0..P5 (P3/P4 invalid in the first run, see look2)
  look2: brightness split with status LEDs on, plus 0x02 all on; USER x5 between
  walk : one 0x04 bit at a time, press the lit key (5 s per bit)
Log: logs/led_<stage>_<ts>.jsonl
"""
import sys
from collections import defaultdict
import time

from pcommon import JsonLog, Panel, TapGate, hexs, log_path

END_BIT = 30  # USER

LOOK = [
    ("P0", "baseline (0x04=0, 0x02=0, brightness 100)",
     [("out", [0x04] + [0] * 6), ("out", [0x02] + [0] * 8), ("feat", [0x03, 100, 100])]),
    ("P1", "0x04 all 48 bits ON",
     [("out", [0x04] + [0xFF] * 6)]),
    ("P2", "0x04 off, 0x02 all 64 bits ON",
     [("out", [0x04] + [0] * 6), ("out", [0x02] + [0xFF] * 8)]),
    ("P3", "0x02 off, brightness 03 64 00",
     [("out", [0x02] + [0] * 8), ("feat", [0x03, 100, 0])]),
    ("P4", "brightness 03 00 64",
     [("feat", [0x03, 0, 100])]),
    ("P5", "brightness 100/100, 0x04 all ON and 0x02 all ON",
     [("feat", [0x03, 100, 100]), ("out", [0x04] + [0xFF] * 6), ("out", [0x02] + [0xFF] * 8)]),
]
RESTORE = [("out", [0x04] + [0] * 6), ("out", [0x02] + [0] * 8), ("feat", [0x03, 100, 100])]
ALL04 = ("out", [0x04] + [0xFF] * 6)
# rerun of P3..P5 (spoiled by the old 2 s brightness resend), with status LEDs ON
LOOK2 = [
    ("Q0", "status LEDs all ON, brightness 100/100 (reference)", [ALL04, ("feat", [0x03, 100, 100])]),
    ("Q1", "status LEDs all ON, brightness 100/0", [("feat", [0x03, 100, 0])]),
    ("Q2", "status LEDs all ON, brightness 0/100", [("feat", [0x03, 0, 100])]),
    ("Q3", "status LEDs all ON, brightness 0/0", [("feat", [0x03, 0, 0])]),
    ("Q4", "brightness 100/100, status LEDs all ON and 0x02 all ON",
     [("feat", [0x03, 100, 100]), ("out", [0x02] + [0xFF] * 8)]),
]


class Prober:
    def __init__(self, log):
        self.log = log
        self.p = Panel(log=log)        # init + keep-alive (0a 01 every 15 s)
        self.write_fail = 0

    def apply(self, ops, why):
        for kind, pkt in ops:
            if kind == "feat":
                self.p.send(pkt, why)
            else:
                try:
                    n = self.p.dev.write(bytes(pkt))
                    ok = n is not None and n > 0
                    err = None if ok else "rc=%r" % n
                except Exception as e:
                    ok, n, err = False, None, repr(e)
                if not ok:
                    self.write_fail += 1
                    print("!! output write %s failed: %s" % (hexs(pkt), err))
                self.log.write(t="write", why=why, data=hexs(pkt), ok=ok, rc=n, err=err)

    def wait_user5(self, cap=300):
        gate, t0 = TapGate(5, 3.0), time.time()
        while time.time() - t0 < cap:
            b = self.p.read(50)
            if b and gate.feed(b) == END_BIT:
                t_q = time.time()
                while time.time() - t_q < 1.0:
                    self.p.read(50)
                return True
        return False


def look(patterns=LOOK, tag="led_look"):
    path = log_path(tag)
    log = JsonLog(path)
    pr = Prober(log)
    done = []
    try:
        for name, desc, ops in patterns:
            print(">>> %s: %s   (look, then USER x5)" % (name, desc), flush=True)
            log.write(t="pattern", name=name, desc=desc)
            pr.apply(ops, name)
            if not pr.wait_user5():
                print("!! timeout waiting for USER x5")
                break
            done.append(name)
    except KeyboardInterrupt:
        print("!! interrupted")
    finally:
        pr.apply(RESTORE, "restore")
        print("restored defaults (0x04=0, 0x02=0, brightness 100)")
        pr.p.close()
    print("patterns shown: %s" % done)
    print("feature sends ok=%d failed=%d; output writes failed=%d"
          % (pr.p.sends_ok, pr.p.sends_failed, pr.write_fail))
    print("log: %s" % path)
    log.close()


def walk(timeout=5.0):
    """Light ONE 0x04 bit at a time. The user presses the key that lit (once; twice
    quickly if it looks different from the all-on view: other colour, dim, blinking).
    No press within `timeout` = nothing visible."""
    from pcommon import button_bits
    from panel_map import BUTTONS
    path = log_path("led_walk")
    log = JsonLog(path)
    pr = Prober(log)
    result = {}
    held = set()

    def press_events(deadline):
        """Yield newly pressed bits until deadline."""
        nonlocal held
        while time.time() < deadline:
            b = pr.p.read(30)
            if b and b[0] == 0x02:
                now = button_bits(b)
                new = now - held
                held = now
                for bit in sorted(new):
                    yield bit, time.time()

    try:
        pr.apply([("feat", [0x03, 100, 100]), ("out", [0x02] + [0] * 8)], "walk-init")
        for bit in range(48):
            pr.apply([("out", [0x04] + [0] * 6)], "blank")
            time.sleep(0.3)
            payload = [0] * 6
            payload[bit // 8] = 1 << (bit % 8)
            pr.apply([("out", [0x04] + payload)], "bit%d" % bit)
            log.write(t="bit", bit=bit)
            key, presses = None, 0
            for kb, t in press_events(time.time() + timeout):
                key, presses = kb, 1
                for kb2, t2 in press_events(t + 0.8):
                    if kb2 == key:
                        presses += 1
                break
            result[bit] = (key, presses)
            log.write(t="answer", bit=bit, key=key, presses=presses)
            print("   0x04 bit %2d (byte %d bit %d): %s%s" % (
                bit, bit // 8 + 1, bit % 8,
                BUTTONS.get(key, key) if key is not None else "-",
                "  (LOOKS DIFFERENT)" if presses >= 2 else ""), flush=True)
            t_rel = time.time()                     # let the key go before the next bit
            while held and time.time() - t_rel < 2.0:
                for _ in press_events(time.time() + 0.1):
                    pass
    except KeyboardInterrupt:
        print("!! interrupted")
    finally:
        pr.apply(RESTORE, "restore")
        pr.p.close()
    print("\n== 0x04 bit map")
    keys = defaultdict(list)
    for bit, (key, n) in sorted(result.items()):
        if key is not None:
            keys[key].append(bit)
            print("   bit %2d -> %s%s" % (bit, BUTTONS.get(key, key), "  (different look)" if n >= 2 else ""))
    print("   bits with no visible LED: %s" % [b for b, (k, _) in sorted(result.items()) if k is None])
    print("   keys with more than one bit: %s" % ({BUTTONS.get(k, k): v for k, v in keys.items() if len(v) > 1} or "none"))
    print("feature sends ok=%d failed=%d; output writes failed=%d"
          % (pr.p.sends_ok, pr.p.sends_failed, pr.write_fail))
    print("log: %s" % path)
    log.close()


if __name__ == "__main__":
    mode = sys.argv[1] if len(sys.argv) > 1 else "look"
    if mode == "look":
        look()
    elif mode == "look2":
        look(LOOK2, "led_look2")
    elif mode == "walk":
        walk()

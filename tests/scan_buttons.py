"""Task 3: guided button scan, block by block.

Usage: python tests/scan_buttons.py [--start N] [--cap 570]

For each block, press each button ONCE in the stated order, then end the block
by pressing the LAST button 5 more times quickly (5 presses within 3 s).

Records the order of first presses per block -> positional name -> bit.
Also flags: bits seen in more than one block, one press giving >1 bit,
buttons still held when a block ends, count mismatches, missing bits of 8..59.
Log: logs/scan_buttons_<ts>.jsonl
"""
import argparse
import sys
import time

from pcommon import JsonLog, Panel, TapGate, button_bits, log_path

BLOCKS = [
    ("KNOB", "Knob PRESSES, top row, left -> right (Y LIFT ... LUM MIX)", 12),
    ("LCOL", "LEFT two columns: left column top -> bottom (AUTO COLOR ... SHIFT UP), "
             "then right column top -> bottom (OFFSET ... SHIFT DOWN)", 14),
    ("LBALL", "3 buttons above the LEFT ball, left -> right", 3),
    ("MEXTRA", "the extra 4 buttons above the middle three, left -> right", 4),
    ("MBALL", "3 buttons above the MIDDLE ball, left -> right", 3),
    ("RBALL", "3 buttons above the RIGHT ball, left -> right", 3),
    ("RCOL", "RIGHT two columns: left column top -> bottom (PREV STILL ... PREV CLIP), "
             "then right column top -> bottom (NEXT STILL ... NEXT CLIP)", 10),
    ("TRANS", "transport: LEFT ARROW, then RIGHT ARROW, then STOP (wide key below)", 3),
]
ALL_BITS = set(range(8, 60))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--start", type=int, default=1, help="1-based block to start at")
    ap.add_argument("--only", default="", help="comma list of 1-based blocks, e.g. 6,8")
    ap.add_argument("--cap", type=float, default=570, help="total time cap, s")
    a = ap.parse_args()

    path = log_path("scan_buttons")
    log = JsonLog(path)
    p = Panel(log=log)
    t_start = time.time()
    results = []   # (block, desc, expected, [bits], notes)
    held = set()
    carry = []     # presses made during the previous block's settle -> this block
    try:
        sel = [int(x) for x in a.only.split(",") if x] or range(a.start, len(BLOCKS) + 1)
        for bi in sel:
            key, desc, expected = BLOCKS[bi - 1]
            gate = TapGate(5, 3.0)
            order, notes = [], []
            print("\n>>> BLOCK %d/%d: %s%s" % (bi, len(BLOCKS), desc,
                  "  (%d buttons)" % expected if expected else ""))
            print("    After the last button, press that SAME button 5 more times quickly.", flush=True)
            log.write(t="block", block=key, n=bi)
            for bit in carry:
                order.append(bit)
                log.write(t="btn", block=key, bit=bit, v=1, carried=True)
                print("    #%-2d bit %d (pressed during previous settle)" % (len(order), bit))
            carry = []
            end_bit = None
            while end_bit is None:
                if time.time() - t_start > a.cap:
                    raise TimeoutError
                b = p.read(50)
                if not b or b[0] != 0x02:
                    continue
                now = button_bits(b)
                down, up = now - held, held - now
                if len(down) > 1:
                    notes.append("one report pressed several bits %s" % sorted(down))
                for bit in sorted(down):
                    log.write(t="btn", block=key, bit=bit, v=1)
                    if bit not in order:
                        order.append(bit)
                        print("    #%-2d bit %d" % (len(order), bit), flush=True)
                for bit in sorted(up):
                    log.write(t="btn", block=key, bit=bit, v=0)
                held = now
                end_bit = gate.feed(b)
            # settle: swallow extra taps of the end button until 1.5 s quiet
            t_q = time.time()
            while time.time() - t_q < 1.5 and not carry:
                b = p.read(50)
                if not b or b[0] != 0x02:
                    continue
                now = button_bits(b)
                for bit in sorted(now - held):
                    log.write(t="btn", block=key, bit=bit, v=1, settle=True)
                    if bit == end_bit:
                        t_q = time.time()
                    else:
                        carry.append(bit)   # next block has started
                for bit in sorted(held - now):
                    log.write(t="btn", block=key, bit=bit, v=0, settle=True)
                held = now
            if order and order[-1] != end_bit:
                notes.append("ended with bit %d, which was not the last button" % end_bit)
            # wait briefly for releases, then report anything still held
            t_rel = time.time()
            while held and time.time() - t_rel < 1.0:
                b = p.read(50)
                if b and b[0] == 0x02:
                    for bit in sorted(held - button_bits(b)):
                        log.write(t="btn", block=key, bit=bit, v=0)
                    held = button_bits(b)
            if held:
                notes.append("still held at block end: %s" % sorted(held))
            if expected and len(order) != expected:
                notes.append("expected %d buttons, got %d" % (expected, len(order)))
            results.append((key, desc, expected, order, notes))
            print("    block done: %d buttons %s" % (len(order), order))
            for n in notes:
                print("    NOTE: " + n)
    except TimeoutError:
        print("\n!! time cap reached; partial results below. Resume with --start N.")
    except KeyboardInterrupt:
        print("\n!! interrupted; partial results below.")
    finally:
        p.close()

    summary(results, p, path, log)
    log.close()


def summary(results, p, path, log):
    out = ["", "== scan_buttons summary"]
    seen = {}
    for key, desc, expected, order, notes in results:
        out.append("%s (%s): %d buttons" % (key, desc, len(order)))
        for i, bit in enumerate(order, 1):
            out.append("   %s_%d -> bit %d" % (key, i, bit))
            seen.setdefault(bit, []).append("%s_%d" % (key, i))
        for n in notes:
            out.append("   NOTE: " + n)
    dup = {b: v for b, v in seen.items() if len(v) > 1}
    out.append("bits in more than one block: %s" % (dup or "none"))
    out.append("bits 8..59 never pressed: %s" % (sorted(ALL_BITS - set(seen)) or "none"))
    out.append("bits outside 8..59: %s" % (sorted(set(seen) - ALL_BITS) or "none"))
    out.append("")
    out.append("POSITIONS = {")
    for bit in sorted(seen):
        out.append('    %d: "%s",' % (bit, seen[bit][0]))
    out.append("}")
    out.append("feature sends ok=%d failed=%d" % (p.sends_ok, p.sends_failed))
    out.append("log: %s" % path)
    for line in out:
        print(line)
        log.write(t="summary", line=line)


if __name__ == "__main__":
    sys.exit(main())

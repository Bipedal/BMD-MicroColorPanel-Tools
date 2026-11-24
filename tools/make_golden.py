"""Replay every recorded raw report in logs/ through panel.py's decoder and write the expected
events, so the Rust port (helper/tests/golden.rs) can be checked against the reference driver.

    python tools/make_golden.py        -> helper/target/golden.jsonl (not committed; large)

One line per input report: {"f": log file, "hex": report, "ev": [events]}. A fresh decoder per
log file, reports in log order. Logs named *_ABORTED_* are skipped.
"""
import glob
import json
import os
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
import panel  # noqa: E402


def ev_json(e):
    d = e.data
    if e.kind == "button":
        return {"k": "button", "n": e.name, "bit": d["bit"], "down": d["down"]}
    if e.kind == "knob":
        return {"k": "knob", "n": e.name, "index": d["index"], "delta": d["delta"], "raw": d["raw"]}
    if e.kind == "ball":
        return {"k": "ball", "n": e.name, "dx": d["dx"], "dy": d["dy"], "raw_x": d["raw_x"], "raw_y": d["raw_y"]}
    if e.kind == "ring":
        return {"k": "ring", "n": e.name, "delta": d["delta"], "raw": d["raw"]}
    if e.kind == "battery":
        return {"k": "battery", "n": "BATTERY", "charging": d["charging"], "level": d["level"]}
    raise ValueError(e.kind)


def main():
    out_dir = os.path.join(ROOT, "helper", "target")
    os.makedirs(out_dir, exist_ok=True)
    out = os.path.join(out_dir, "golden.jsonl")
    n_rep = n_ev = n_files = 0
    with open(out, "w", encoding="utf-8") as fo:
        for f in sorted(glob.glob(os.path.join(ROOT, "logs", "*.jsonl"))):
            if "ABORTED" in f:
                continue
            p = panel.Panel(open_device=False)
            used = False
            for line in open(f, encoding="utf-8"):
                try:
                    e = json.loads(line)
                except ValueError:
                    continue
                if e.get("t") != "rx" or not isinstance(e.get("data"), str):
                    continue
                try:
                    b = bytes.fromhex(e["data"].replace(" ", ""))
                except ValueError:
                    continue
                if not b or b[0] not in (2, 5, 6, 8) or ("rid" in e and e["rid"] != b[0]):
                    continue
                b = b + bytes(65 - len(b)) if len(b) < 65 else b[:65]
                ev = [ev_json(x) for x in p.decode(b, 0.0)]
                fo.write(json.dumps({"f": os.path.basename(f), "hex": b.hex(), "ev": ev}) + "\n")
                n_rep += 1
                n_ev += len(ev)
                used = True
            n_files += used
    print("golden: %d reports, %d events from %d logs -> %s" % (n_rep, n_ev, n_files, out))


if __name__ == "__main__":
    main()

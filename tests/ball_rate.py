"""Ball report timing: Python reader (panel.py) vs the Rust helper, back to back.

The tester rolls the middle ball continuously for ~30 s. Phase 1 (12 s) reads with panel.py and
timestamps every 0x05 report with ball movement; phase 2 (14 s) runs micropanel.exe --dry --trace
and takes the timestamps of its ball events. Prints the gap statistics of both and writes
logs/ball_rate_<stamp>.jsonl. No output is sent to the mouse.

    python tests/ball_rate.py [--py 12] [--rust 14]
"""
import argparse
import json
import os
import subprocess
import sys
import time

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, ROOT)
import panel  # noqa: E402

EXE = os.path.join(ROOT, "helper", "target", "release", "micropanel.exe")
HOME = os.path.join(ROOT, "logs", "bridge_home")


def stats(name, ts):
    gaps = sorted(round((b - a) * 1000, 1) for a, b in zip(ts, ts[1:]) if b - a < 0.3)
    if not gaps:
        print("%s: no movement" % name)
        return {}
    n = len(gaps)
    s = {"reports": len(ts), "p10": gaps[n // 10], "median": gaps[n // 2], "p90": gaps[int(n * 0.9)],
         "rate_per_s": round(len(ts) / (ts[-1] - ts[0]), 1) if ts[-1] > ts[0] else None}
    print("%-6s reports %4d  gap ms p10 %5.1f median %5.1f p90 %5.1f  ~%s reports/s" % (
        name, s["reports"], s["p10"], s["median"], s["p90"], s["rate_per_s"]))
    return s


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--py", type=float, default=12)
    ap.add_argument("--rust", type=float, default=14)
    a = ap.parse_args()
    out = os.path.join(ROOT, "logs", "ball_rate_%s.jsonl" % time.strftime("%Y%m%d_%H%M%S"))
    fo = open(out, "w", encoding="utf-8")

    p = panel.Panel(reconnect=False)
    ts_py = []
    t0 = time.time()
    while time.time() - t0 < a.py:
        b = p.read_report(20)
        if b and b[0] == 0x05:
            t = time.time()
            ev = [e for e in p.decode(b, t) if e.kind == "ball" and e.name == "MIDDLE"]
            if ev:
                ts_py.append(t)
                fo.write(json.dumps({"src": "py", "ts": t, "dx": ev[0].data["dx"], "dy": ev[0].data["dy"]}) + "\n")
    p.close(dark=False)

    os.makedirs(HOME, exist_ok=True)
    log = os.path.join(HOME, "helper.log")
    if os.path.exists(log):
        os.remove(log)
    env = dict(os.environ, MICROPANEL_HOME=HOME)
    subprocess.run([EXE, "--no-tray", "--dry", "--trace", "--run-for", str(a.rust)], env=env,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    ts_rs = []
    for line in open(log, encoding="utf-8", errors="replace"):
        if " ev {" in line and '"k":"ball"' in line and '"n":"MIDDLE"' in line:
            h, m, s = line[:12].split(":")
            t = int(h) * 3600 + int(m) * 60 + float(s)
            ts_rs.append(t)
            fo.write(json.dumps({"src": "rust", "ts": t}) + "\n")
    s1, s2 = stats("python", ts_py), stats("rust", ts_rs)
    fo.write(json.dumps({"t": "summary", "python": s1, "rust": s2}) + "\n")
    print("log", out)


if __name__ == "__main__":
    sys.exit(main())

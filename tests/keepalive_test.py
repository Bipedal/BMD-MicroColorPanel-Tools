"""Is the keep-alive needed? Send init ONCE, then nothing, and watch whether data stops.

Usage: python tests/keepalive_test.py [--secs 120] [--no-init]

Timeline
  t=0      send 0a 01 + 03 64 64 once, then NO sends at all
  0..secs  you roll a ball gently on and off (a second or two every few seconds)
           -> logs every report; reports the longest silence and whether data
              stopped for good
  if data stopped: send 03 64 64 alone, listen 6 s (keep rolling)
                   then 0a 01 alone, listen 6 s (keep rolling)
                   -> which report revives the stream?
No end gesture: the run is time-based, because USER taps would not arrive if the
stream died. Log: logs/keepalive_<ts>.jsonl
"""
import argparse
import sys
import time

from pcommon import INIT, LIGHT, JsonLog, Panel, hexs, log_path


def listen(p, log, secs):
    end, got = time.time() + secs, []
    while time.time() < end:
        b = p.read(50)
        if b:
            got.append((time.time(), b[0]))
            log.write(t="rx", rid=b[0], data=hexs(b[:37]))
    return got


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--secs", type=float, default=120)
    ap.add_argument("--no-init", action="store_true", help="send nothing at all at start")
    a = ap.parse_args()
    path = log_path("keepalive")
    log = JsonLog(path)
    p = Panel(log=log, keepalive=False, init=not a.no_init)  # init once (or never), then nothing
    t0 = time.time()
    print(">>> %s at t=0." % ("NOTHING sent" if a.no_init else "Init sent once") +
          " Roll a ball gently on and off for %d s." % a.secs,
          flush=True)
    rx = listen(p, log, a.secs)
    out = ["", "== keepalive summary (%s, then no sends for %d s)"
           % ("NO init" if a.no_init else "init once", a.secs)]
    if not rx:
        out.append("NO reports at all after a single init.")
    else:
        ts = [t - t0 for t, _ in rx]
        gaps = sorted(((b - a_, a_) for a_, b in zip(ts, ts[1:])), reverse=True)[:5]
        out.append("reports: %d, first at %.1f s, last at %.1f s" % (len(ts), ts[0], ts[-1]))
        out.append("longest silences (len @ start): %s"
                   % ", ".join("%.1f s @ %.1f" % g for g in gaps))
        tail = a.secs - ts[-1]
        out.append("silence at end of run: %.1f s" % tail)
    dead = (not rx) or (a.secs - (rx[-1][0] - t0) > 10)
    out.append("VERDICT: %s" % ("stream appears DEAD at the end (or you stopped rolling); "
                                "probing revival" if dead else
                                "stream ALIVE for the whole run with no keep-alive"))
    if dead:
        for name, pkt in (("03 64 64", LIGHT), ("0a 01", INIT)):
            print(">>> sending %s; keep rolling" % name, flush=True)
            p.send(pkt, "revive")
            got = listen(p, log, 6)
            out.append("after %s alone: %d reports in 6 s" % (name, len(got)))
            if got:
                break
    p.close()
    out.append("feature sends ok=%d failed=%d" % (p.sends_ok, p.sends_failed))
    out.append("log: %s" % path)
    for line in out:
        print(line)
        log.write(t="summary", line=line)
    log.close()


if __name__ == "__main__":
    sys.exit(main())

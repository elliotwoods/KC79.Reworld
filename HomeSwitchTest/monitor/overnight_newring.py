"""Overnight bake for the injection-moulded ring gear (side A, 32:1).

The production `O` fast-home cannot run on this ring (its `T = background - 10`
calibration has no measurable background), so this harness drives the proposed
replacement instead:

  * every RECAL_MIN minutes, measure the DETECTABILITY BAND by running a census
    lap at each threshold in a ladder, and pick T_op = T_floor + F * band;
  * between recalibrations, home repeatedly from starts spread around the whole
    circle with alternating approach directions, using fixed-threshold `H`.

Crucially it does NOT abort when the band falls below the gate. Room lights go
off overnight, and ambient IR is worth ~10 counts of band on this ring, so the
band is expected to collapse. Recording homing success against a shrinking band
is the point: it yields reliability-vs-contrast, which sets how much albedo the
home reflector actually needs.

Output: reports/newring/overnight.jsonl (one record per cycle and per
recalibration), plus failures/ forensics dumps.

usage: overnight_newring.py [hours] [--port COMn]
"""
import ctypes
import json
import os
import sys
import time
import traceback

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bench_harness import Bench, REPORTS_DIR

HOURS = 18.0
PORT = None
_args = sys.argv[1:]
for i, a in enumerate(_args):
    if a == "--port" and i + 1 < len(_args):
        PORT = _args[i + 1]
    elif not a.startswith("--"):
        try:
            HOURS = float(a)
        except ValueError:
            pass

RECAL_MIN = 30.0
F = 0.55
BAND_MIN = 6
WIDTH_MIN = 40
SHOULDER_FACTOR = 3.0
T_CAP_DARK = 253
LADDER = list(range(238, 255, 2))
GOLDEN = 0.6180339887

OUT_DIR = os.path.join(REPORTS_DIR, "newring")
FAIL_DIR = os.path.join(OUT_DIR, "failures")
LOG = os.path.join(OUT_DIR, "overnight.jsonl")
os.makedirs(FAIL_DIR, exist_ok=True)


def keep_awake():
    """Stop Windows sleeping/blanking for the duration of the run."""
    try:
        ES_CONTINUOUS = 0x80000000
        ES_SYSTEM_REQUIRED = 0x00000001
        ES_DISPLAY_REQUIRED = 0x00000002
        ctypes.windll.kernel32.SetThreadExecutionState(
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED)
    except Exception:
        pass


def emit(rec):
    rec["ts"] = time.strftime("%Y-%m-%dT%H:%M:%S")
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(json.dumps(rec) + "\n")


def say(msg):
    print(f"[{time.strftime('%H:%M:%S')}] {msg}", flush=True)


def connect():
    b = Bench(port=PORT, verbose=False)
    b.send("E 1")
    time.sleep(0.4)
    b.send("U 32")          # pin the 32:1 parameter set; no detection lap
    time.sleep(0.4)
    b.drain(0.2)
    return b


def segments(b, T, timeout=150):
    r = b.census(T, vmax=10000, timeout=timeout)
    segs, op = [], None
    for pos, state in r["edges"]:
        if state == 1:
            op = pos
        elif state == 0 and op is not None:
            segs.append((op, pos - op))
            op = None
    return segs


def measure_band(b, cycle):
    """Census ladder -> (T_op, band, table). Never raises on a narrow band."""
    table = []
    for T in LADDER:
        try:
            segs = segments(b, T)
        except Exception as e:
            table.append({"T": T, "err": str(e)[:80]})
            continue
        table.append({"T": T, "n": len(segs),
                      "w": segs[0][1] if segs else 0})

    usable = [(e["T"], e["w"]) for e in table
              if e.get("n") == 1 and e.get("w", 0) >= WIDTH_MIN]
    if not usable:
        emit({"kind": "recal", "cycle": cycle, "ok": False,
              "reason": "no usable threshold", "table": table})
        say("RECAL: no usable threshold anywhere in the ladder")
        return None, 0, table

    widths = sorted(w for _, w in usable)
    med = widths[len(widths) // 2]
    t_floor = usable[0][0]
    t_shoulder = None
    for T, w in usable:
        if w > med * SHOULDER_FACTOR:
            t_shoulder = T
            break
    if t_shoulder is None:
        t_shoulder = min(usable[-1][0] + 2, T_CAP_DARK + 1)
    band = (t_shoulder - 1) - t_floor

    if band >= BAND_MIN:
        t_op = int(round(t_floor + F * band))
    else:
        # below the gate: production would refuse. Here we carry on at the
        # best available point so the night still yields data.
        t_op = int(round((t_floor + t_shoulder - 1) / 2.0))
    t_op = max(t_floor, min(t_shoulder - 1, t_op))

    emit({"kind": "recal", "cycle": cycle, "ok": True,
          "t_floor": t_floor, "t_shoulder": t_shoulder, "band": band,
          "t_op": t_op, "below_gate": band < BAND_MIN,
          "median_width": med, "table": table})
    say(f"RECAL: floor={t_floor} shoulder={t_shoulder} band={band} "
        f"-> T_op={t_op}{'  [BELOW GATE]' if band < BAND_MIN else ''}")
    return t_op, band, table


def forensics(b, cycle, note):
    """On a home failure, dump what the sensor looks like right now."""
    path = os.path.join(FAIL_DIR, f"fail_{cycle:05d}_"
                                  f"{time.strftime('%Y%m%d_%H%M%S')}.txt")
    lines = [f"cycle {cycle}  {note}", ""]
    try:
        b.send("P")
        time.sleep(0.4)
        lines.append(f"status: {b.last_status}")
    except Exception as e:
        lines.append(f"status failed: {e}")
    for T in (248, 250, 252, 253):
        try:
            segs = segments(b, T, timeout=150)
            lines.append(f"census T={T}: {len(segs)} seg "
                         f"{[(p, w) for p, w in segs][:6]}")
        except Exception as e:
            lines.append(f"census T={T} failed: {e}")
    with open(path, "w", encoding="utf-8") as f:
        f.write("\n".join(lines))
    say(f"forensics -> {os.path.basename(path)}")


def main():
    keep_awake()
    t_start = time.time()
    t_end = t_start + HOURS * 3600.0
    say(f"overnight bake starting; {HOURS:.1f} h, recal every {RECAL_MIN:.0f} min")
    emit({"kind": "session_start", "hours": HOURS, "recal_min": RECAL_MIN,
          "f": F, "band_min": BAND_MIN, "ladder": LADDER})

    b = connect()
    rev = b.usteps_per_rev
    deg = 360.0 / rev
    say(f"connected on {b.port}; rev={rev}")

    cycle = 0
    t_op, band = None, 0
    last_recal = -1e9
    ok_n = fail_n = 0
    reconnects = 0

    while time.time() < t_end:
        try:
            if time.time() - last_recal > RECAL_MIN * 60.0:
                t_op, band, _ = measure_band(b, cycle)
                last_recal = time.time()
                if t_op is None:
                    # nothing detectable at all; wait and retry next window
                    emit({"kind": "idle", "cycle": cycle,
                          "reason": "no usable threshold"})
                    time.sleep(60)
                    continue
                b.send(f"T {t_op}")
                time.sleep(0.5)
                # re-establish the datum at the new threshold
                try:
                    b.run("H", ["H,"], timeout=200)
                except Exception:
                    pass

            frac = (cycle * GOLDEN) % 1.0
            start = int(frac * rev) - rev // 2
            approach = "fwd" if cycle % 2 == 0 else "rev"
            kind = "uniform"
            if cycle % 10 == 3:
                start, kind = 0, "at_home"
            elif cycle % 10 == 7:
                start, kind = int(179.9 / deg), "wrap"

            pre = start - 4000 if approach == "fwd" else start + 4000
            b.goto(pre, speed=12000)
            time.sleep(0.15)
            b.goto(start, speed=8000)
            time.sleep(0.25)

            t0 = time.time()
            term, _ = b.run("H", ["H,"], timeout=200)
            dur = time.time() - t0
            p = term.split(",")
            if int(p[1]) == 1:
                ok_n += 1
                emit({"kind": "home", "cycle": cycle, "ok": True,
                      "start": start, "approach": approach, "start_kind": kind,
                      "t_op": t_op, "band": band, "below_gate": band < BAND_MIN,
                      "home": int(p[2]), "switch": int(p[3]),
                      "lead": int(p[4]), "trail": int(p[5]),
                      "secs": round(dur, 2)})
            else:
                fail_n += 1
                msg = term.split('"')[1] if '"' in term else ""
                emit({"kind": "home", "cycle": cycle, "ok": False,
                      "start": start, "approach": approach, "start_kind": kind,
                      "t_op": t_op, "band": band, "below_gate": band < BAND_MIN,
                      "msg": msg, "secs": round(dur, 2)})
                say(f"cycle {cycle}: FAIL {msg} (T={t_op} band={band})")
                forensics(b, cycle, f"home fail: {msg}")

            if cycle % 25 == 0:
                el = (time.time() - t_start) / 3600.0
                say(f"cycle {cycle}: {ok_n} ok / {fail_n} fail, "
                    f"T_op={t_op} band={band}, {el:.2f} h elapsed")
            cycle += 1

        except KeyboardInterrupt:
            say("interrupted")
            break
        except Exception as e:
            fail_n += 1
            say(f"cycle {cycle}: EXCEPTION {e}")
            emit({"kind": "exception", "cycle": cycle, "err": str(e)[:200],
                  "trace": traceback.format_exc()[-600:]})
            try:
                b.close()
            except Exception:
                pass
            time.sleep(5)
            try:
                b = connect()
                reconnects += 1
                say(f"reconnected ({reconnects})")
                last_recal = -1e9        # force a fresh recal after a reconnect
            except Exception as e2:
                say(f"reconnect failed: {e2}; retrying in 60 s")
                time.sleep(60)
            cycle += 1

    emit({"kind": "session_end", "cycles": cycle, "ok": ok_n, "fail": fail_n,
          "reconnects": reconnects,
          "hours": round((time.time() - t_start) / 3600.0, 3)})
    say(f"done: {ok_n} ok / {fail_n} fail over {cycle} cycles, "
        f"{reconnects} reconnects")
    try:
        b.close()
    except Exception:
        pass


if __name__ == "__main__":
    main()

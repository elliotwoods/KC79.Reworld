"""Autonomous endurance / thermal-recovery session for the 16:1 module.

Drives one batch per invocation (rest -> probe -> optional activity -> probe),
appending JSONL events to reports/endurance_16/endurance.jsonl. Designed to be
relaunched repeatedly overnight with different phases as the picture develops.

Usage:
  python endurance16.py rest <minutes>
  python endurance16.py probe [--detect]
  python endurance16.py burst <minutes>          # stay-at-park homes, slip-guarded
  python endurance16.py ladder                   # fwd/bwd jog envelope 8k..14k
  python endurance16.py cycle <run_min> <rest_min> <repeats>
  python endurance16.py session <run_min> <rest_min> <repeats> <epoch>
      # repeats x (probe -> full bake batch -> probe -> rest); every 3rd rest
      # is doubled; a HALTed bake earns a 45-min recovery rest instead
"""
import datetime
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from bench_harness import Bench, keep_awake, exp_bake, BAKE_DIR

REV = 92252   # fallback; live value comes from b.usteps_per_rev (banner)
OUT_DIR = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                       "..", "reports", "endurance_16")
LOG = os.path.join(OUT_DIR, "endurance.jsonl")
SAFE_V_FWD = 10000      # gentle forward jog speed for probes
SAFE_V_BWD = 2000       # backward: crawl - verified clean even warm
HOME_SEEK = 8000        # gentle seek for probe homes


def jog_speed(delta):
    return SAFE_V_FWD if delta >= 0 else SAFE_V_BWD


def log(event, **kw):
    os.makedirs(OUT_DIR, exist_ok=True)
    rec = {"ts": datetime.datetime.now().isoformat(timespec="seconds"),
           "event": event}
    rec.update(kw)
    with open(LOG, "a") as f:
        f.write(json.dumps(rec) + "\n")
    print(f"{rec['ts']} {event} {kw}", flush=True)


def coils(b, on):
    b.send(f"E {1 if on else 0}")
    time.sleep(0.3)
    b.drain()


def rest(b, minutes):
    coils(b, False)
    log("rest_begin", minutes=minutes)
    time.sleep(minutes * 60)
    log("rest_end")


def probe(b, detect=False):
    """Gentle health probe (gear-ratio agnostic: rev + jog speeds come from
    the module the firmware detected). Returns dict with healthy flag."""
    coils(b, True)
    out = {"healthy": False, "home_ok": 0, "err_fwd": None, "err_bwd": None,
           "detect_rev": None, "T": None, "backlash": None, "rev": None}
    force = 1 if detect else 0
    r = b.fast_home(0, 0, 0, 0, force, 0, timeout=150)
    if r.get("motor"):
        out["detect_rev"] = r["motor"][1]
    out["home_ok"], out["T"], out["backlash"] = r["ok"], r["T"], r["backlash"]
    rev = b.usteps_per_rev
    out["rev"] = rev
    is32 = rev > 120000
    v_fwd = 20000 if is32 else SAFE_V_FWD
    v_bwd = 20000 if is32 else SAFE_V_BWD
    if not r["ok"]:
        log("probe", **out, msg=r["msg"])
        coils(b, False)
        return out
    b.ramped_jog(rev, v_fwd, 100000, timeout=180)
    r2 = b.fast_home(0, 0, 0, 0, 0, 0, timeout=150)
    if r2["ok"]:
        out["err_fwd"] = r2["home"] - rev
    # NB backward leg uses 3/4 rev: a from-park jog of EXACTLY one rev is a
    # degenerate measurement (the flag lands back at the sensor whether the
    # ring moved perfectly or not at all - it cost us a day of phantom
    # "backward dead" findings). 0.75 rev leaves moved/stalled distinguishable.
    d = 3 * rev // 4
    b.ramped_jog(-d, v_bwd, 100000, timeout=180)
    r3 = b.fast_home(0, 0, 0, 0, 0, 0, timeout=150)
    if r3["ok"]:
        out["err_bwd"] = r3["home"]   # == slip of the backward leg (0 if clean)
    out["healthy"] = (r2["ok"] and r3["ok"]
                      and abs(out["err_fwd"]) < 300 and abs(out["err_bwd"]) < 300)
    log("probe", **out)
    return out


def burst(b, minutes):
    """Continuous warm homes; every 5th adds a +/-1-rev jog check. Stops early
    the moment slip exceeds the guard - the time-to-degradation IS the datum."""
    coils(b, True)
    t0 = time.time()
    n = 0
    log("burst_begin", minutes=minutes)
    while time.time() - t0 < minutes * 60:
        if n % 5 == 4:
            d = REV if (n % 10 == 4) else -REV
            b.ramped_jog(d, jog_speed(d), 100000, timeout=180)
        r = b.fast_home(0, 0, 0, 0, 0, 0, timeout=150)
        if not r["ok"]:
            log("burst_fail", n=n, t_s=round(time.time() - t0), msg=r["msg"])
            break
        err = r["home"] - round(r["home"] / REV) * REV
        if abs(err) > 300:
            log("burst_slip", n=n, t_s=round(time.time() - t0), err=err)
            break
        n += 1
    log("burst_end", n=n, t_s=round(time.time() - t0))
    coils(b, False)


def ladder(b):
    coils(b, True)
    b.run("U 16", ["U,"], timeout=10)
    r = b.fast_home(0, 0, HOME_SEEK, 0, 0, 0, timeout=150)
    if not r["ok"]:
        log("ladder_abort", msg=r["msg"])
        coils(b, False)
        return
    for v in (8000, 10000, 12000, 14000):
        for sign, name in ((1, "fwd"), (-1, "bwd")):
            b.ramped_jog(sign * REV, v, 100000, timeout=180)
            r = b.fast_home(0, 0, HOME_SEEK, 0, 0, 0, timeout=150)
            err = (r["home"] - (REV if sign > 0 else 0)) if r["ok"] else None
            log("ladder_point", v=v, dir=name, err=err,
                msg=None if r["ok"] else r["msg"])
    coils(b, False)


def sustain(b, mA, cap_min, move_v=10000):
    """Realistic production duty: forward-only jogs of 0.1-0.5 rev with 3 s
    dwells (driver auto-sleeps after 1 s idle), a warm home every 10 moves
    measuring the block's accumulated slip. Runs to cap_min or failure
    (hard home fail, or two consecutive blocks with |slip| > 2000)."""
    coils(b, True)
    b.run(f"D 5 {int(mA)}", ["D,"], timeout=10)
    r = b.fast_home(0, 0, 0, 0, 0, 0, timeout=150)
    if not r["ok"]:
        log("sustain_abort", mA=mA, msg=r["msg"])
        coils(b, False)
        return
    rev = b.usteps_per_rev
    log("sustain_begin", mA=mA, cap_min=cap_min, move_v=move_v, rev=rev)
    t0 = time.time()
    n = block = bad_blocks = 0
    dists = [rev // 10, 3 * rev // 10, rev // 2, 3 * rev // 20, 2 * rev // 5,
             rev // 4, 9 * rev // 20, rev // 5, 7 * rev // 20, 3 * rev // 10]
    while time.time() - t0 < cap_min * 60:
        for d in dists:
            b.ramped_jog(d, move_v, 100000, timeout=120)
            time.sleep(3.0)          # production-style dwell; sleeps at 1 s
            n += 1
        r = b.fast_home(0, 0, 0, 0, 0, 0, timeout=150)
        if not r["ok"]:
            log("sustain_fail", mA=mA, n=n, t_s=round(time.time() - t0),
                msg=r["msg"])
            break
        err = r["home"] - round(r["home"] / rev) * rev
        block += 1
        log("sustain_block", mA=mA, block=block, n=n,
            t_s=round(time.time() - t0), err=err, backlash=r["backlash"],
            T=r["T"], fw_s=round(r["fw_ms"] / 1000.0, 1))
        if abs(err) > 2000:
            bad_blocks += 1
            if bad_blocks >= 2:
                log("sustain_degraded", mA=mA, n=n,
                    t_s=round(time.time() - t0))
                break
        else:
            bad_blocks = 0
    log("sustain_end", mA=mA, n=n, t_s=round(time.time() - t0))
    b.run("D 5 250", ["D,"], timeout=10)
    coils(b, False)


def session(b, run_min, rest_min, repeats, epoch):
    """The long-haul protocol: repeats x (probe -> bake -> probe -> rest).
    Every 3rd rest is doubled (sheds accumulated core heat); a bake that
    HALTs (2 consecutive recovery failures) earns a 45-min recovery rest and
    the session continues - today's data says the mechanism recovers."""
    halt_marker = os.path.join(BAKE_DIR, "HALT.txt")
    for i in range(repeats):
        log("session_cycle", i=i, of=repeats)
        probe(b)
        if os.path.exists(halt_marker):
            os.remove(halt_marker)
        coils(b, True)
        b = exp_bake(b, None, run_min, 5, 0, 0, 0, epoch)
        halted = os.path.exists(halt_marker)
        probe(b)
        if halted:
            log("session_halt_recovery", i=i)
            rest(b, 45)
        elif (i + 1) % 3 == 0:
            rest(b, rest_min * 2)
        else:
            rest(b, rest_min)
    return b


def main():
    keep_awake()
    mode = sys.argv[1] if len(sys.argv) > 1 else "probe"
    b = Bench()
    b.send("X"); time.sleep(0.5); b.drain()
    try:
        if mode == "rest":
            rest(b, float(sys.argv[2]))
        elif mode == "probe":
            probe(b, detect="--detect" in sys.argv)
        elif mode == "burst":
            burst(b, float(sys.argv[2]))
        elif mode == "ladder":
            ladder(b)
        elif mode == "cycle":
            run_min, rest_min, reps = (float(sys.argv[2]), float(sys.argv[3]),
                                       int(sys.argv[4]))
            for i in range(reps):
                p = probe(b)
                if p["healthy"]:
                    burst(b, run_min)
                rest(b, rest_min)
        elif mode == "session":
            b = session(b, float(sys.argv[2]), float(sys.argv[3]),
                        int(sys.argv[4]), sys.argv[5])
        elif mode == "sustain":
            sustain(b, float(sys.argv[2]), float(sys.argv[3]),
                    int(sys.argv[4]) if len(sys.argv) > 4 else 10000)
        else:
            sys.exit(f"unknown mode {mode}")
    finally:
        coils(b, False)
        b.close()


if __name__ == "__main__":
    main()

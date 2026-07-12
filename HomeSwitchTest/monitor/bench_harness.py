"""Headless bench harness for autonomous homing-routine development.

Drives the home_switch_bench firmware over the ST-Link VCP (no GUI) and runs
the fast-home (O) experiment suite. Every experiment writes a CSV (+ PNG where
useful) into HomeSwitchTest/reports/.

Usage (from HomeSwitchTest/monitor, using the shared venv):
  .venv/Scripts/python bench_harness.py census  [--T 244 248 252] [--v 20000]
  .venv/Scripts/python bench_harness.py probe   [--speeds 24000 32000 40000 48000]
                                                [--accels 50000 100000 200000]
  .venv/Scripts/python bench_harness.py knee    [--vedges 1000 2000 4000 8000]
                                                [--debounces 1 8 16] [--n 8]
  .venv/Scripts/python bench_harness.py matrix  [--vedge 4000] [--m 8] [--n 5]
  .venv/Scripts/python bench_harness.py backlash [--n 10] [--vedge 4000] [--m 8]
  .venv/Scripts/python bench_harness.py cmd "O 4000 8"    # ad-hoc, streams output
"""

import argparse
import csv
import datetime
import json
import os
import sys
import time

import serial
import serial.tools.list_ports

BAUD = 115200
# Exact rational, rounded (the double-truncated form 189696 is 7.9 short/rev);
# the firmware banner overrides this on connect anyway.
USTEPS_PER_REV_DEFAULT = (32 * 118 * 9759 * 32 + (296 * 21) // 2) // (296 * 21)  # 189704

REPORTS_DIR = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "reports"))


def autodetect_port():
    """Finds the most likely serial port for the ST-Link VCP."""
    ports = sorted(serial.tools.list_ports.comports())
    for p in ports:
        d = (p.description or "").upper()
        if "ST-LINK" in d or "STMICROELECTRONICS" in d:
            return p.device
    for p in ports:
        if "usbmodem" in p.device or "ACM" in p.device:
            return p.device
    return ports[0].device if ports else None


class Bench:
    """Synchronous request/terminal-line protocol driver.

    The firmware streams S-status at ~60 Hz permanently; every blocking command
    ends with a distinct terminal line (O,done / H, / K,end / N,end / an S line
    for plain moves). run() sends one command and collects lines until its
    terminal shows up.
    """

    def __init__(self, port=None, baud=BAUD, verbose=False):
        port = port or autodetect_port()
        if not port:
            sys.exit("No serial port found. Plug in the board or pass --port.")
        s = serial.Serial()
        s.port = port
        s.baudrate = baud
        s.timeout = 0.2
        s.dtr = False          # don't reset the board on open
        s.rts = False
        s.open()
        self.ser = s
        self.port = port
        self.verbose = verbose
        self.usteps_per_rev = USTEPS_PER_REV_DEFAULT
        self.last_status = None
        time.sleep(0.3)
        self.drain()

    def close(self):
        try:
            self.ser.close()
        except Exception:
            pass

    # -- line level -----------------------------------------------------------
    def _handle_passive(self, line):
        if line.startswith("S,"):
            p = line.split(",")
            if len(p) >= 10:
                self.last_status = {
                    "ms": int(p[1]), "level": int(p[2]), "thr": int(p[3]),
                    "pos": int(p[4]), "degx10": int(p[5]), "running": int(p[6]),
                    "enabled": int(p[7]), "fault": int(p[8]), "homed": int(p[9]),
                }
        elif line.startswith("#") and "usteps_per_rev=" in line:
            for tok in line.split():
                if tok.startswith("usteps_per_rev="):
                    self.usteps_per_rev = int(tok.split("=")[1])

    def readline(self):
        raw = self.ser.readline()
        if not raw:
            return None
        line = raw.decode("utf-8", "replace").strip()
        if line:
            self._handle_passive(line)
        return line or None

    def drain(self, seconds=0.3):
        end = time.time() + seconds
        while time.time() < end:
            if self.readline() is None:
                time.sleep(0.02)

    def send(self, text):
        if self.verbose:
            print(f">> {text}")
        self.ser.write((text + "\n").encode("ascii", "replace"))

    def run(self, cmd, terminals, timeout=90.0, collect_prefixes=None):
        """Send `cmd`; return (terminal_line, collected) where collected holds
        every line matching collect_prefixes (default: the command's tag)."""
        self.drain(0.05)
        self.send(cmd)
        collected = []
        t0 = time.time()
        while time.time() - t0 < timeout:
            line = self.readline()
            if line is None:
                continue
            if self.verbose and not line.startswith("S,"):
                print(f"<< {line}")
            if line.startswith("L,1,busy drop"):
                raise RuntimeError(f"firmware dropped command (busy): {cmd!r}")
            if collect_prefixes and any(line.startswith(p) for p in collect_prefixes):
                collected.append(line)
            for t in terminals:
                if line.startswith(t):
                    return line, collected
        raise TimeoutError(f"no terminal {terminals} within {timeout}s for {cmd!r}")

    # -- command level --------------------------------------------------------
    def goto(self, usteps, speed=14080):
        # Plain moves have no dedicated terminal line; the firmware emits one S
        # immediately after busy clears. Poll position until it settles instead.
        self.send(f"G {int(usteps)} {int(speed)}")
        t0 = time.time()
        while time.time() - t0 < 120:
            line = self.readline()
            if line is None:
                continue
            st = self.last_status
            if st and not st["running"] and abs(st["pos"] - int(usteps)) < 64:
                return
        raise TimeoutError(f"goto {usteps} did not settle")

    def ramped_jog(self, delta, vmax, accel, timeout=120.0):
        self.drain(0.05)
        self.send(f"Y {int(delta)} {int(vmax)} {int(accel)}")
        target = None
        t0 = time.time()
        time.sleep(0.2)
        while time.time() - t0 < timeout:
            line = self.readline()
            if line is None:
                continue
            st = self.last_status
            if st and not st["running"] and time.time() - t0 > 1.0:
                return
        raise TimeoutError("ramped jog did not settle")

    def fast_home(self, vedge=0, m=0, vseek=0, accel=0, force=0, passes=0,
                  timeout=150.0):
        """Run O; returns dict with the full result + duration + sub-lines."""
        args = [vedge, m, vseek, accel, force, passes]
        # positional args: trailing zeros can be dropped
        while args and not args[-1]:
            args.pop()
        cmd = "O" + ("" if not args else " " + " ".join(str(int(a)) for a in args))
        t_host0 = time.time()
        term, lines = self.run(cmd, ["O,done,"], timeout=timeout,
                               collect_prefixes=["O,"])
        host_ms = (time.time() - t_host0) * 1000.0
        p = term.split(",")
        msg = term.split('"')[1] if '"' in term else ""
        res = {
            "ok": int(p[2]), "home": int(p[3]), "lead": int(p[4]),
            "trail": int(p[5]), "switch": int(p[6]), "backlash": int(p[7]),
            "T": int(p[8]), "fw_ms": int(p[9]), "host_ms": host_ms,
            "msg": msg, "lines": lines, "passes": [], "reenter": None,
        }
        for ln in lines:
            q = ln.split(",")
            if ln.startswith("O,pass,"):
                res["passes"].append((int(q[3]), int(q[4])))
            elif ln.startswith("O,backlash,"):
                res["reenter"] = int(q[3])
        return res

    def census(self, T, vmax=20000, accel=100000, m=8, timeout=60.0):
        term, lines = self.run(f"N {T} {int(vmax)} {int(accel)} {m}",
                               ["N,end,"], timeout=timeout,
                               collect_prefixes=["N,"])
        p = term.split(",")
        edges = []
        start_pos = None
        for ln in lines:
            q = ln.split(",")
            if ln.startswith("N,begin,"):
                start_pos = int(q[2])
            elif ln.startswith("N,edge,"):
                edges.append((int(q[3]), int(q[4])))   # (pos, new_state)
        return {"count": int(p[2]), "aborted": int(p[3]),
                "start": start_pos, "edges": edges}


# ------------------------------------------------------------------------
# report helpers
# ------------------------------------------------------------------------

def report_path(name):
    os.makedirs(REPORTS_DIR, exist_ok=True)
    return os.path.join(REPORTS_DIR, name)


def write_csv(name, header, rows, meta=None):
    path = report_path(name)
    with open(path, "w", newline="") as f:
        if meta:
            for k, v in meta.items():
                f.write(f"# {k}={v}\n")
        w = csv.writer(f)
        w.writerow(header)
        w.writerows(rows)
    print(f"wrote {path} ({len(rows)} rows)")
    return path


def stats(values):
    if not values:
        return (float("nan"),) * 4
    n = len(values)
    mean = sum(values) / n
    var = sum((v - mean) ** 2 for v in values) / n
    sd = var ** 0.5
    return mean, sd, min(values), max(values)


# ------------------------------------------------------------------------
# experiments
# ------------------------------------------------------------------------

def exp_census(b, thresholds, vmax):
    """Full-rev feature map at each threshold; plots segments + writes CSVs."""
    all_results = {}
    for T in thresholds:
        r = b.census(T, vmax=vmax)
        rev = b.usteps_per_rev
        rows = [(i, pos, state, (pos - (r["start"] or 0)) % rev,
                 360.0 * ((pos - (r["start"] or 0)) % rev) / rev)
                for i, (pos, state) in enumerate(r["edges"])]
        write_csv(f"census_T{T}.csv",
                  ["index", "pos_usteps", "new_state", "lap_usteps", "lap_deg"],
                  rows, meta={"threshold": T, "vmax": vmax,
                              "usteps_per_rev": rev, "aborted": r["aborted"],
                              "start_pos": r["start"]})
        all_results[T] = r
        segs = segments_from_edges(r["edges"], b.usteps_per_rev, r["start"])
        print(f"T={T}: {len(r['edges'])} edges, active segments: "
              + ", ".join(f"[{a:.2f}..{z:.2f}deg w={w}]" for a, z, w in segs))

    try:
        plot_census(all_results, b.usteps_per_rev)
    except Exception as e:
        print(f"(census plot failed: {e})")
    return all_results


def segments_from_edges(edges, rev, start):
    """[(startDeg, endDeg, width_usteps)] of ACTIVE spans within the lap."""
    segs = []
    open_pos = None
    for pos, state in edges:
        if state == 1:
            open_pos = pos
        elif state == 0 and open_pos is not None:
            a = 360.0 * ((open_pos - (start or 0)) % rev) / rev
            z = 360.0 * ((pos - (start or 0)) % rev) / rev
            segs.append((a, z, pos - open_pos))
            open_pos = None
    return segs


def plot_census(results, rev):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    fig, ax = plt.subplots(figsize=(12, 1.0 + 0.8 * len(results)))
    for row, (T, r) in enumerate(sorted(results.items())):
        for a, z, w in segments_from_edges(r["edges"], rev, r["start"]):
            ax.barh(row, z - a, left=a, height=0.6, color="tab:red")
        ax.text(-8, row, f"T={T}", va="center", ha="right")
    ax.set_xlim(-15, 370)
    ax.set_xlabel("lap position (deg)")
    ax.set_yticks([])
    ax.set_title("Sensor census: ACTIVE segments per threshold (one full rev)")
    out = report_path("census.png")
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    print(f"wrote {out}")


def exp_probe(b, speeds, accels, vedge, m):
    """Find max reliable seek speed/accel: for each combo, start half a rev
    before home and O with that (vSeek, accel); the reported home should land
    at ~1 rev in the pre-zero frame - deviation = step loss during the seek."""
    rev = b.usteps_per_rev
    rows = []
    print("establishing datum with a baseline O ...")
    r0 = b.fast_home(vedge, m)
    if not r0["ok"]:
        sys.exit(f"baseline O failed: {r0['msg']}")
    for v in speeds:
        for a in accels:
            b.goto(-rev // 2)          # half a rev behind home -> seek travels ~1/2+...
            r = b.fast_home(vedge, m, v, a)
            if r["ok"]:
                # nearest whole-rev multiple: residual = step loss + edge noise
                k = round(r["home"] / rev)
                loss = r["home"] - k * rev
            else:
                loss = None
            rows.append((v, a, r["ok"], r["home"], loss, r["fw_ms"], r["msg"]))
            print(f"vseek={v} accel={a}: ok={r['ok']} loss={loss} "
                  f"t={r['fw_ms']}ms {r['msg']}")
    write_csv("probe.csv",
              ["vseek", "accel", "ok", "home_reported", "loss_usteps",
               "fw_ms", "msg"],
              rows, meta={"vedge": vedge, "m": m, "usteps_per_rev": rev})
    return rows


def exp_knee(b, vedges, debounces, n):
    """Edge-capture speed/debounce sweep: N repeats per combo from a fixed
    short-seek start; per-trial home error = reported home (pre-zero frame)."""
    rows = []
    print("establishing datum with a baseline O ...")
    r0 = b.fast_home()
    if not r0["ok"]:
        sys.exit(f"baseline O failed: {r0['msg']}")
    for v in vedges:
        for m in debounces:
            errs, widths, backlashes, times = [], [], [], []
            for i in range(n):
                b.goto(-10000)
                r = b.fast_home(v, m)
                if not r["ok"]:
                    print(f"  run failed: {r['msg']}")
                    continue
                errs.append(r["home"])
                widths.append(r["switch"])
                backlashes.append(r["backlash"])
                times.append(r["fw_ms"])
                rows.append((v, m, i, r["home"], r["lead"], r["trail"],
                             r["switch"], r["backlash"], r["fw_ms"]))
            # Drop the first run per combo: changing (v, M) moves the datum, so
            # run 0 measures the step to the new datum, not repeatability.
            mean, sd, lo, hi = stats(errs[1:])
            wmean = stats(widths)[0]
            bmean, bsd = stats(backlashes)[:2]
            print(f"vedge={v} M={m}: home mean={mean:.1f} sd={sd:.1f} "
                  f"range=[{lo},{hi}] width~{wmean:.0f} "
                  f"backlash~{bmean:.0f}+/-{bsd:.0f} t~{stats(times)[0]:.0f}ms "
                  f"(datum step {errs[0] if errs else 'n/a'})")
    write_csv("knee.csv",
              ["vedge", "m", "trial", "home_err", "lead", "trail",
               "switch", "backlash", "fw_ms"],
              rows, meta={"n": n})
    try:
        plot_knee(rows)
    except Exception as e:
        print(f"(knee plot failed: {e})")
    return rows


def plot_knee(rows):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from collections import defaultdict
    groups = defaultdict(list)
    for v, m, i, err, lead, trail, w, bl, ms in rows:
        if i > 0:   # drop the datum-step run per combo
            groups[(v, m)].append(err)
    fig, ax = plt.subplots(figsize=(9, 5))
    ms_set = sorted({m for _, m in groups})
    for m in ms_set:
        vs = sorted({v for v, mm in groups if mm == m})
        sds = [stats(groups[(v, m)])[1] for v in vs]
        ax.plot(vs, sds, "o-", label=f"M={m}")
    ax.set_xlabel("edge-capture speed (usteps/s)")
    ax.set_ylabel("home repeatability sd (usteps)")
    ax.set_xscale("log")
    ax.legend()
    ax.grid(True, alpha=0.3)
    ax.set_title("Repeatability vs edge speed / debounce (knee search)")
    out = report_path("knee.png")
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    print(f"wrote {out}")


def exp_matrix(b, vedge, m, n, offsets_deg):
    """Acceptance: home from a range of start offsets, N runs each."""
    rev = b.usteps_per_rev
    rows = []
    print("establishing datum with a baseline O ...")
    r0 = b.fast_home(vedge, m)
    if not r0["ok"]:
        sys.exit(f"baseline O failed: {r0['msg']}")
    for off in offsets_deg:
        start = int(rev * off / 360.0)
        for i in range(n):
            b.goto(start)
            r = b.fast_home(vedge, m)
            if r["ok"]:
                k = round(r["home"] / rev)
                err = r["home"] - k * rev
            else:
                err = None
            rows.append((off, i, r["ok"], r["home"], err, r["switch"],
                         r["backlash"], r["fw_ms"], r["msg"]))
            print(f"start={off:+.0f}deg run{i}: ok={r['ok']} err={err} "
                  f"switch={r['switch']} backlash={r['backlash']} "
                  f"t={r['fw_ms']}ms")
    write_csv("matrix.csv",
              ["start_deg", "trial", "ok", "home_reported", "home_err",
               "switch", "backlash", "fw_ms", "msg"],
              rows, meta={"vedge": vedge, "m": m, "usteps_per_rev": rev})
    try:
        plot_matrix(rows)
    except Exception as e:
        print(f"(matrix plot failed: {e})")
    return rows


def plot_matrix(rows):
    import matplotlib
    matplotlib.use("Agg")
    import matplotlib.pyplot as plt
    from collections import defaultdict
    times = defaultdict(list)
    errs = defaultdict(list)
    for off, i, ok, home, err, sw, bl, ms, msg in rows:
        if ok:
            times[off].append(ms / 1000.0)
            errs[off].append(err)
    offs = sorted(times)
    fig, (ax1, ax2) = plt.subplots(1, 2, figsize=(12, 4.5))
    ax1.boxplot([times[o] for o in offs], tick_labels=[f"{o:+.0f}" for o in offs])
    ax1.set_xlabel("start offset (deg)")
    ax1.set_ylabel("O duration (s)")
    ax1.set_title("Homing time vs start position")
    ax1.grid(True, alpha=0.3)
    ax2.boxplot([errs[o] for o in offs], tick_labels=[f"{o:+.0f}" for o in offs])
    ax2.set_xlabel("start offset (deg)")
    ax2.set_ylabel("home error (usteps)")
    ax2.set_title("Home consistency vs start position")
    ax2.grid(True, alpha=0.3)
    out = report_path("matrix.png")
    fig.tight_layout()
    fig.savefig(out, dpi=120)
    print(f"wrote {out}")


def exp_stability(b, n, vedge, m, passes, tag=""):
    """N warm homes back-to-back from the same short-seek start. Per-run
    reported home == successive shift (each run re-zeros). Decomposes the
    within-run pass spread from the between-run scatter."""
    rows = []
    print(f"stability: n={n} vedge={vedge} M={m} passes={passes}")
    print("baseline O to align the datum ...")
    r0 = b.fast_home(vedge, m, passes=passes)
    if not r0["ok"]:
        sys.exit(f"baseline O failed: {r0['msg']}")
    shifts, withins, widths, times = [], [], [], []
    fails = 0
    for i in range(n):
        b.goto(-8000)
        r = b.fast_home(vedge, m, passes=passes)
        if not r["ok"]:
            fails += 1
            print(f"run {i:2d}: FAIL {r['msg']}")
            rows.append((i, 0, None, None, None, None, None, r["msg"]))
            continue
        pmids = [(l + t) / 2 for l, t in r["passes"]]
        within = round(max(pmids) - min(pmids)) if len(pmids) > 1 else None
        shifts.append(r["home"])
        if within is not None:
            withins.append(within)
        widths.append(r["switch"])
        times.append(r["fw_ms"])
        rows.append((i, 1, r["home"], within, r["switch"], r["backlash"],
                     r["fw_ms"], ""))
        print(f"run {i:2d}: shift={r['home']:+4d}  pass-spread={within}  "
              f"w={r['switch']} B={r['backlash']} t={r['fw_ms']}ms")
    mean, sd, lo, hi = stats(shifts)
    mx = max((abs(s) for s in shifts), default=float("nan"))
    deg = 360.0 / b.usteps_per_rev
    print(f"\n=> shifts: mean={mean:+.1f} sd={sd:.2f} max|shift|={mx:.0f} usteps "
          f"({mx * deg:.4f} deg)  [target 15.8u = 0.03deg, stretch 5.3u = 0.01deg]")
    if withins:
        wm = stats(withins)[0]
        print(f"=> within-run pass spread mean={wm:.1f} (measurement noise); "
              f"between-run sd={sd / (2 ** 0.5):.2f} per-datum")
    print(f"=> width sd={stats(widths)[1]:.1f}  time mean={stats(times)[0]:.0f}ms  fails={fails}")
    name = f"stability_{tag}" if tag else "stability"
    write_csv(f"{name}.csv",
              ["trial", "ok", "shift", "pass_spread", "switch", "backlash",
               "fw_ms", "msg"],
              rows, meta={"n": n, "vedge": vedge, "m": m, "passes": passes,
                          "sd_shift": f"{sd:.2f}", "max_abs_shift": mx,
                          "usteps_per_rev": b.usteps_per_rev})
    return shifts, withins


def exp_rotation(b, n, vedge, m, passes):
    """Home, then alternate [one commanded full rev -> home] in each direction.
    Mean shift: +eps forward / -eps backward => eps = rev-constant error;
    a same-sign component is mechanical direction bias."""
    rev = b.usteps_per_rev
    print(f"rotation: commanded rev = {rev} (firmware banner)")
    r0 = b.fast_home(vedge, m, passes=passes)
    if not r0["ok"]:
        sys.exit(f"baseline O failed: {r0['msg']}")
    rows = []
    means = {}
    for dirn, sign in (("fwd", +1), ("bwd", -1)):
        shifts = []
        for i in range(n):
            b.ramped_jog(sign * rev, 24000, 100000)
            r = b.fast_home(vedge, m, passes=passes)
            if not r["ok"]:
                print(f"{dirn} {i}: FAIL {r['msg']}")
                rows.append((dirn, i, 0, None, None, r["msg"]))
                continue
            k = round(r["home"] / rev)
            shift = r["home"] - k * rev
            shifts.append(shift)
            rows.append((dirn, i, 1, r["home"], shift, ""))
            print(f"{dirn} {i}: shift={shift:+4d} (home={r['home']:+d})")
        mean, sd, lo, hi = stats(shifts)
        means[dirn] = mean
        print(f"=> {dirn}: mean={mean:+.1f} sd={sd:.1f} n={len(shifts)}")
    if "fwd" in means and "bwd" in means:
        eps = (means["fwd"] - means["bwd"]) / 2
        bias = (means["fwd"] + means["bwd"]) / 2
        print(f"\n=> rev-constant error eps = {eps:+.1f} usteps/rev "
              f"(true rev ~ {rev + eps:.1f}); direction bias = {bias:+.1f}")
    write_csv("rotation.csv",
              ["dir", "trial", "ok", "home_reported", "shift", "msg"],
              rows, meta={"n": n, "commanded_rev": rev, "vedge": vedge,
                          "m": m, "passes": passes})
    return means


def exp_backlash(b, n, vedge, m):
    """Backlash stability: N repeats from one start, plus a speed-independence
    check at half and double vedge."""
    rows = []
    print("establishing datum with a baseline O ...")
    r0 = b.fast_home(vedge, m)
    if not r0["ok"]:
        sys.exit(f"baseline O failed: {r0['msg']}")
    for label, v in (("nominal", vedge), ("half", vedge // 2), ("double", vedge * 2)):
        vals = []
        runs = n if label == "nominal" else max(3, n // 2)
        for i in range(runs):
            b.goto(-8000)
            r = b.fast_home(v, m)
            if r["ok"]:
                vals.append(r["backlash"])
                rows.append((label, v, i, r["backlash"], r["switch"], r["fw_ms"]))
        mean, sd, lo, hi = stats(vals)
        print(f"{label} (vedge={v}): backlash mean={mean:.1f} sd={sd:.1f} "
              f"range=[{lo},{hi}] n={len(vals)}")
    write_csv("backlash.csv",
              ["set", "vedge", "trial", "backlash", "switch", "fw_ms"],
              rows, meta={"m": m})
    return rows


# ------------------------------------------------------------------------
# resilience bake (overnight): home from every starting angle, log everything
# ------------------------------------------------------------------------

BAKE_DIR = os.path.join(REPORTS_DIR, "bake")
BAKE_LOG = os.path.join(BAKE_DIR, "bake_log.jsonl")
BAKE_FAIL_DIR = os.path.join(BAKE_DIR, "failures")
GOLDEN_DEG = 137.50776405003785
BAKE_OVERSHOOT = 2000          # final approach leg, sets the mesh direction
BAKE_ADV = 8                   # adversarial entries per lap
BAKE_LAP = 24 + BAKE_ADV + 1   # + 1 stay-at-park entry


def keep_awake():
    """Stop Windows sleeping mid-bake (auto-released when the process exits)."""
    if os.name == "nt":
        import ctypes
        ES_CONTINUOUS, ES_SYSTEM_REQUIRED = 0x80000000, 0x00000001
        ctypes.windll.kernel32.SetThreadExecutionState(
            ES_CONTINUOUS | ES_SYSTEM_REQUIRED)


def bake_plan(cycle, width, rev):
    """Deterministic start spec for a cycle -> (kind, target_usteps|None, approach).

    Lap = 24 uniform angles (rotated by the golden angle each lap so the circle
    fills in densely) + adversarial starts on/inside the flag and at the seek
    boundaries + one home-from-park. Approach direction alternates per cycle.
    """
    lap, idx = divmod(cycle, BAKE_LAP)
    approach = +1 if cycle % 2 == 0 else -1
    half = max(1, width // 2)
    if idx < 24:
        deg = (idx * 15.0 + lap * GOLDEN_DEG) % 360.0
        if deg > 180.0:
            deg -= 360.0
        return (f"uniform{deg:+.1f}", int(rev * deg / 360.0), approach)
    adv = (
        ("at-home",        0),
        ("in-flag+",       half // 2),
        ("in-flag-",       -half // 2),
        ("lead-edge",      -half - 100),
        ("trail-edge",     half + 100),
        ("wrap+0.5deg",    int(rev * 0.5 / 360.0)),
        ("bound+179.9deg", int(rev * 179.9 / 360.0)),
        ("bound-179.9deg", int(-rev * 179.9 / 360.0)),
    )
    j = idx - 24
    if j < len(adv):
        return (adv[j][0], adv[j][1], approach)
    return ("stay-at-park", None, 0)


def bake_reposition(b, target, approach, multi_rev=0):
    """Move to `target` arriving in the `approach` direction (overshoot + return);
    optional +/-N full-rev detour first. Returns (final_pos, seconds)."""
    t0 = time.time()
    if multi_rev:
        b.ramped_jog(multi_rev * b.usteps_per_rev, 24000, 100000)
    if target is not None:
        pos = b.last_status["pos"] if b.last_status else 0
        pre = target - approach * BAKE_OVERSHOOT
        if pre != pos:
            b.ramped_jog(pre - pos, 24000, 100000)
        b.ramped_jog(approach * BAKE_OVERSHOOT, 14000, 100000)
    pos = b.last_status["pos"] if b.last_status else target
    return pos, time.time() - t0


def bake_forensics(b, nfail, note, lines):
    """Freeze the evidence for a hard failure: transcript + W debug + census
    laps around the current threshold. Never raises."""
    os.makedirs(BAKE_FAIL_DIR, exist_ok=True)
    ts = datetime.datetime.now().strftime("%Y%m%d_%H%M%S")
    path = os.path.join(BAKE_FAIL_DIR, f"fail_{nfail:03d}_{ts}.txt")
    out = [f"# {ts}  {note}", f"# last_status: {b.last_status}"]
    out += ["# --- run transcript ---"] + list(lines or [])
    try:
        b.send("X")
        b.drain(1.0)
        out.append("# --- W debug burst ---")
        b.send("W")
        t0 = time.time()
        while time.time() - t0 < 3.0:
            ln = b.readline()
            if ln and not ln.startswith("S,"):
                out.append(ln)
        T = (b.last_status or {}).get("thr", 0)
        for dT in (-6, 0, 6):
            if not 16 <= T + dT <= 250:
                continue
            out.append(f"# --- census at T={T + dT} ---")
            try:
                r = b.census(T + dT, vmax=20000)
                out.append(f"# edges={r['count']} aborted={r['aborted']} "
                           f"start={r['start']}")
                out += [f"{pos},{state}" for pos, state in r["edges"]]
            except Exception as e:
                out.append(f"# census failed: {e!r}")
    except Exception as e:
        out.append(f"# forensics aborted: {e!r}")
    with open(path, "w") as f:
        f.write("\n".join(out) + "\n")
    print(f"  forensics -> {path}")
    return path


def bake_reopen(b):
    """Serial dropped: reopen the same port (board keeps running)."""
    port = b.port
    b.close()
    for attempt in range(5):
        time.sleep(2.0)
        try:
            nb = Bench(port=port)
            print(f"  serial reopened on {port} (attempt {attempt + 1})")
            return nb
        except Exception as e:
            print(f"  reopen attempt {attempt + 1} failed: {e!r}")
    raise RuntimeError(f"could not reopen {port}")


def bake_recover(b, vedge, m, passes):
    """After a hard failure: abort whatever is running, forced cold home.
    Never raises: returns an ok=0 dict on any exception."""
    try:
        b.send("X")
        b.drain(1.0)
        return b.fast_home(vedge, m, 0, 0, 1, passes, timeout=150)
    except Exception as e:
        return {"ok": 0, "msg": f"recover: {e!r}", "switch": None, "T": None}


def bake_anomalies(rec, cold):
    a = []
    if rec["home_err"] is not None and abs(rec["home_err"]) > 300:
        a.append("home_err>300")
    if rec["backlash"] is not None and not 400 <= rec["backlash"] <= 700:
        a.append("backlash-band")
    if not cold and rec["fw_ms"] is not None and rec["fw_ms"] > 40000:
        a.append("slow>40s")
    if rec["pass_spread"] is not None and rec["pass_spread"] > 25:
        a.append("pass-spread>25")
    return a


def exp_bake(b, until_iso, max_minutes, cold_every, vedge, m, passes, epoch):
    keep_awake()
    os.makedirs(BAKE_FAIL_DIR, exist_ok=True)
    rev = b.usteps_per_rev

    deadline = time.time() + max_minutes * 60.0
    if until_iso:
        until_ts = datetime.datetime.fromisoformat(until_iso).timestamp()
        deadline = min(deadline, until_ts)
    if deadline <= time.time():
        print("deadline already passed; nothing to do")
        return b

    # resume global cycle numbering from the existing log
    cycle = 0
    if os.path.exists(BAKE_LOG):
        with open(BAKE_LOG) as f:
            for line in f:
                try:
                    cycle = max(cycle, json.loads(line)["cycle"] + 1)
                except Exception:
                    pass
    print(f"bake: starting at cycle {cycle}, "
          f"{(deadline - time.time()) / 60.0:.1f} min budget, "
          f"rev={rev}, epoch={epoch}")

    def log(rec):
        with open(BAKE_LOG, "a") as f:
            f.write(json.dumps(rec) + "\n")

    # baseline home: establish the frame + current width
    r0 = b.fast_home(vedge, m, 0, 0, 0, passes, timeout=150)
    if not r0["ok"]:
        print(f"baseline warm home FAILED ({r0['msg']}); trying cold ...")
        bake_forensics(b, 0, f"baseline warm fail: {r0['msg']}", r0["lines"])
        r0 = b.fast_home(vedge, m, 0, 0, 1, passes, timeout=150)
        if not r0["ok"]:
            with open(os.path.join(BAKE_DIR, "HALT.txt"), "w") as f:
                f.write(f"baseline cold home failed: {r0['msg']}\n")
            print("HALT: baseline cold home failed")
            return b
    width = r0["switch"]

    n_ok = n_fail = n_anom = 0
    t_first = t_last = r0["T"]
    w_lo = w_hi = width
    consec_recover_fails = 0
    fail_seq = len(os.listdir(BAKE_FAIL_DIR)) if os.path.isdir(BAKE_FAIL_DIR) else 0

    while time.time() < deadline:
        kind, target, approach = bake_plan(cycle, width, rev)
        multi = 0
        if cycle % 50 == 49:
            multi = 2 if (cycle // 50) % 2 == 0 else -2
        cold = 1 if (cold_every and cycle % cold_every == cold_every - 1) else 0
        rec = {
            "ts": datetime.datetime.now().isoformat(timespec="seconds"),
            "cycle": cycle, "epoch": epoch, "kind": kind,
            "target": target, "approach": approach, "multi_rev": multi,
            "cold": cold, "ok": 0, "msg": "", "home": None, "home_err": None,
            "lead": None, "trail": None, "switch": None, "backlash": None,
            "reenter": None, "T": None, "fw_ms": None, "repos_s": None,
            "start_pos": None, "pass_mids": [], "pass_spread": None,
            "thr_events": [], "gate_events": [], "anomalies": [],
        }
        r = None
        try:
            start_pos, repos_s = bake_reposition(b, target, approach, multi)
            rec["start_pos"], rec["repos_s"] = start_pos, round(repos_s, 2)
            r = b.fast_home(vedge, m, 0, 0, cold, passes, timeout=150)
            rec["ok"], rec["msg"] = r["ok"], r["msg"]
            rec["home"], rec["lead"], rec["trail"] = r["home"], r["lead"], r["trail"]
            rec["switch"], rec["backlash"], rec["reenter"] = \
                r["switch"], r["backlash"], r["reenter"]
            rec["T"], rec["fw_ms"] = r["T"], r["fw_ms"]
            rec["thr_events"] = [ln for ln in r["lines"] if ln.startswith("O,thr,")]
            rec["gate_events"] = [ln for ln in r["lines"] if ln.startswith("O,gate,")]
            if r["ok"]:
                k = round(r["home"] / rev)
                rec["home_err"] = r["home"] - k * rev
                mids = [(l + t) / 2 for l, t in r["passes"]]
                rec["pass_mids"] = mids
                if len(mids) > 1:
                    rec["pass_spread"] = round(max(mids) - min(mids))
                rec["anomalies"] = bake_anomalies(rec, cold)
                width = r["switch"]
                t_last = r["T"]
                w_lo, w_hi = min(w_lo, width), max(w_hi, width)
                n_ok += 1
                consec_recover_fails = 0
                if rec["anomalies"]:
                    n_anom += 1
                flag = " !! " + ",".join(rec["anomalies"]) if rec["anomalies"] else ""
                print(f"c{cycle:4d} {kind:>15s} {'C' if cold else 'w'} "
                      f"err={rec['home_err']:+4d} w={width} B={r['backlash']} "
                      f"T={r['T']} {r['fw_ms'] / 1000.0:4.1f}s{flag}")
            else:
                raise RuntimeError(f"O failed: {r['msg']}")
        except (serial.SerialException, OSError) as e:
            rec["msg"] = f"serial: {e!r}"
            print(f"c{cycle:4d} {kind:>15s} SERIAL FAIL {e!r}")
            n_fail += 1
            fail_seq += 1
            try:
                b = bake_reopen(b)
                rr = bake_recover(b, vedge, m, passes)
                consec_recover_fails = 0 if rr["ok"] else consec_recover_fails + 1
            except Exception as e2:
                rec["msg"] += f" / reopen: {e2!r}"
                consec_recover_fails += 1
        except (RuntimeError, TimeoutError) as e:
            lines = r["lines"] if isinstance(r, dict) else []
            rec["msg"] = rec["msg"] or str(e)
            print(f"c{cycle:4d} {kind:>15s} HARD FAIL: {rec['msg']}")
            n_fail += 1
            fail_seq += 1
            bake_forensics(b, fail_seq, f"cycle {cycle} {kind}: {rec['msg']}", lines)
            rr = bake_recover(b, vedge, m, passes)
            if rr["ok"]:
                consec_recover_fails = 0
                width = rr["switch"]
                print(f"  recovered (cold home ok, T={rr['T']})")
            else:
                consec_recover_fails += 1
                print(f"  recovery cold home FAILED: {rr['msg']} "
                      f"({consec_recover_fails} consecutive)")
        log(rec)
        cycle += 1
        if consec_recover_fails >= 2:
            with open(os.path.join(BAKE_DIR, "HALT.txt"), "w") as f:
                f.write(f"cycle {cycle}: {consec_recover_fails} consecutive "
                        f"recovery failures; last msg: {rec['msg']}\n")
            print("HALT: two consecutive recovery failures — stopping batch "
                  "for triage")
            break

    print(f"\nBAKE BATCH SUMMARY: cycles={n_ok + n_fail} ok={n_ok} "
          f"hardfail={n_fail} anomalies={n_anom} "
          f"T {t_first}->{t_last} width [{w_lo}..{w_hi}] "
          f"next_cycle={cycle}")
    return b


# ------------------------------------------------------------------------

def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", default=None)
    ap.add_argument("--verbose", action="store_true")
    sub = ap.add_subparsers(dest="exp", required=True)

    p = sub.add_parser("census")
    p.add_argument("--T", type=int, nargs="+", default=[244, 248, 252])
    p.add_argument("--v", type=int, default=20000)

    p = sub.add_parser("probe")
    p.add_argument("--speeds", type=int, nargs="+",
                   default=[24000, 32000, 40000, 48000])
    p.add_argument("--accels", type=int, nargs="+",
                   default=[50000, 100000, 200000])
    p.add_argument("--vedge", type=int, default=4000)
    p.add_argument("--m", type=int, default=8)

    p = sub.add_parser("knee")
    p.add_argument("--vedges", type=int, nargs="+",
                   default=[1000, 2000, 4000, 8000])
    p.add_argument("--debounces", type=int, nargs="+", default=[1, 8, 16])
    p.add_argument("--n", type=int, default=8)

    p = sub.add_parser("matrix")
    p.add_argument("--vedge", type=int, default=4000)
    p.add_argument("--m", type=int, default=8)
    p.add_argument("--n", type=int, default=5)
    p.add_argument("--offsets", type=float, nargs="+",
                   default=[-90, -30, -5, 0, 5, 30, 90, 180])

    p = sub.add_parser("backlash")
    p.add_argument("--n", type=int, default=10)
    p.add_argument("--vedge", type=int, default=4000)
    p.add_argument("--m", type=int, default=8)

    p = sub.add_parser("stability")
    p.add_argument("--n", type=int, default=25)
    p.add_argument("--vedge", type=int, default=2000)
    p.add_argument("--m", type=int, default=32)
    p.add_argument("--passes", type=int, default=2)
    p.add_argument("--tag", default="")

    p = sub.add_parser("rotation")
    p.add_argument("--n", type=int, default=10)
    p.add_argument("--vedge", type=int, default=2000)
    p.add_argument("--m", type=int, default=32)
    p.add_argument("--passes", type=int, default=2)

    p = sub.add_parser("bake")
    p.add_argument("--until", default=None,
                   help="ISO datetime hard deadline, e.g. 2026-07-11T09:30")
    p.add_argument("--max-minutes", type=float, default=55.0)
    p.add_argument("--cold-every", type=int, default=10)
    p.add_argument("--vedge", type=int, default=0)   # 0 = firmware default
    p.add_argument("--m", type=int, default=0)
    p.add_argument("--passes", type=int, default=0)
    p.add_argument("--epoch", default="f4bbc77")

    p = sub.add_parser("cmd")
    p.add_argument("text")
    p.add_argument("--timeout", type=float, default=90.0)

    args = ap.parse_args()
    b = Bench(port=args.port, verbose=args.verbose)
    print(f"connected on {b.port}")
    try:
        if args.exp == "census":
            exp_census(b, args.T, args.v)
        elif args.exp == "probe":
            exp_probe(b, args.speeds, args.accels, args.vedge, args.m)
        elif args.exp == "knee":
            exp_knee(b, args.vedges, args.debounces, args.n)
        elif args.exp == "matrix":
            exp_matrix(b, args.vedge, args.m, args.n, args.offsets)
        elif args.exp == "backlash":
            exp_backlash(b, args.n, args.vedge, args.m)
        elif args.exp == "stability":
            exp_stability(b, args.n, args.vedge, args.m, args.passes, args.tag)
        elif args.exp == "rotation":
            exp_rotation(b, args.n, args.vedge, args.m, args.passes)
        elif args.exp == "bake":
            b = exp_bake(b, args.until, args.max_minutes, args.cold_every,
                         args.vedge, args.m, args.passes, args.epoch)
        elif args.exp == "cmd":
            b.verbose = True
            tag = args.text.strip()[0].upper()
            terminals = {
                "O": ["O,done,"], "H": ["H,"], "K": ["K,end,"],
                "N": ["N,end,"], "A": ["A,done,"], "Q": ["Q,"],
            }.get(tag)
            if terminals:
                term, _ = b.run(args.text, terminals, timeout=args.timeout)
                print(term)
            else:
                b.send(args.text)
                t0 = time.time()
                while time.time() - t0 < 3.0:
                    ln = b.readline()
                    if ln and not ln.startswith("S,"):
                        print(ln)
                if b.last_status:
                    print(f"status: {b.last_status}")
    finally:
        b.close()


if __name__ == "__main__":
    main()

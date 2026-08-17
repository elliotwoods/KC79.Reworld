"""Full-range optical profile for a HIGH-ALBEDO (painted) home feature.

The unpainted tooling only sweeps T = 234..255, because the unpainted flag's
crossing sits just under the censored background. Crossing duty is INVERSE to
reflectance, so a silver-painted feature pushes the flag's crossing far lower -
possibly to the bottom of the DAC. Everything here sweeps the full 0..255.

Two instruments, both trustworthy (the K grid scan is not - it sweeps the
RC-filtered DAC and reads 10-20 counts high):

  1. static level ladder - park, step T, read the live comparator level from the
     S status. Gives the crossing at a point.
  2. census width-vs-T - one ramped lap per threshold, latching debounced
     transitions. Exactly what homing does; gives band edges and flag width, and
     exposes specular multi-peak structure as extra segments.

usage: painted_profile.py [--seek-T 247] [--coarse 16]
"""
import sys
import time

sys.path.insert(0, __file__.rsplit("\\", 1)[0] if "\\" in __file__ else ".")
from bench_harness import Bench

SEEK_T = 247
COARSE = 16
_a = sys.argv[1:]
for i, x in enumerate(_a):
    if x == "--seek-T" and i + 1 < len(_a):
        SEEK_T = int(_a[i + 1])
    elif x == "--coarse" and i + 1 < len(_a):
        COARSE = int(_a[i + 1])

b = Bench(verbose=False)
b.send("E 1")
time.sleep(0.3)
rev = b.usteps_per_rev
print(f"connected {b.port}; rev={rev}\n", flush=True)


def lap(T, timeout=150):
    r = b.census(T, vmax=10000, timeout=timeout)
    segs, op = [], None
    for pos, state in r["edges"]:
        if state == 1:
            op = pos
        elif state == 0 and op is not None:
            segs.append((op, pos - op))
            op = None
    return segs


def level_at(T, settle=0.7):
    b.send(f"T {T}")
    time.sleep(settle)
    b.drain(0.05)
    b.send("P")
    t0 = time.time()
    while time.time() - t0 < 2.0:
        ln = b.readline()
        if ln and ln.startswith("S,"):
            return b.last_status["level"]
    return None


def ladder(pos, label):
    """Coarse then fine sweep of T; returns the crossing duty (or None)."""
    b.goto(pos - 3000, speed=10000)
    time.sleep(0.2)
    b.goto(pos, speed=4000)
    time.sleep(0.5)
    print(f"--- {label} (pos={pos}) ---", flush=True)

    coarse = list(range(0, 256, COARSE))
    if coarse[-1] != 255:
        coarse.append(255)
    prev_T, prev_l = None, None
    lo_bound = hi_bound = None
    for T in coarse:
        l = level_at(T)
        print(f"  T={T:3d} level={l}", flush=True)
        if prev_l == 0 and l == 1:
            lo_bound, hi_bound = prev_T, T
            break
        if T == 0 and l == 1:
            print("  => ACTIVE already at T=0: brighter than the DAC can "
                  "resolve (crossing <= 0)")
            return 0
        prev_T, prev_l = T, l
    if lo_bound is None:
        print("  => never active up to T=255: crossing > 255 (censored)")
        return None

    print(f"  refining in ({lo_bound}, {hi_bound}] ...", flush=True)
    first = hi_bound
    for T in range(lo_bound + 1, hi_bound + 1):
        l = level_at(T)
        print(f"  T={T:3d} level={l}", flush=True)
        if l == 1:
            first = T
            break
    print(f"  => crossing = {first - 1} (first ACTIVE at T={first})")
    return first - 1


# locate the flag
segs = lap(SEEK_T)
if not segs:
    sys.exit(f"no flag found at seek T={SEEK_T}")
start, width = segs[0]
centre = start + width // 2
print(f"flag at {360.0*(centre%rev)/rev:.3f}deg, width {width} at T={SEEK_T}\n",
      flush=True)

peak = ladder(centre, "ON FLAG (centre)")
print()
bg = ladder(centre + 25000, "OFF FLAG (background)")

print("\n=== census width vs threshold (full usable range) ===", flush=True)
lo = 0 if peak is None else max(0, peak - 4)
ladder_ts = sorted(set(list(range(lo, 256, 12)) + [250, 251, 252, 253, 254, 255]))
table = []
for T in ladder_ts:
    try:
        s = lap(T)
    except Exception as e:
        print(f"  T={T:3d}: error {e}", flush=True)
        continue
    n = len(s)
    w = s[0][1] if s else 0
    tot = sum(x[1] for x in s)
    table.append((T, n, w, tot))
    extra = ""
    if n > 1:
        extra = "   <== MULTI-SEGMENT (specular split?) " + \
                str([x[1] for x in s][:5])
    print(f"  T={T:3d}: {n} seg  width={w:6d}  total={tot:6d}{extra}",
          flush=True)

print("\n=== summary ===")
print(f"flag peak crossing  = {peak if peak is not None else 'censored'}")
print(f"background crossing = {bg if bg is not None else 'censored (>255)'}")
single = [(T, w) for T, n, w, _ in table if n == 1 and w > 0]
if single:
    t_floor = single[0][0]
    t_top = single[-1][0]
    print(f"single-segment from T={t_floor} to T={t_top} "
          f"=> band >= {t_top - t_floor} counts")
    ws = [w for _, w in single]
    print(f"width over that range: {min(ws)}..{max(ws)} usteps")
b.close()

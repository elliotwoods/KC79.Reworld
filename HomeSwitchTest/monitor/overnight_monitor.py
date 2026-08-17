"""Live dashboard for the overnight new-ring bake.

Tails reports/newring/overnight.jsonl (read-only - it never touches the serial
port, so it can run alongside overnight_newring.py) and plots:

  * detectability BAND vs time, against the gate - the key question is whether
    the band survives the night as the room goes dark and the optics drift;
  * chosen operating threshold T_op vs time;
  * homing outcome per cycle, failures marked;
  * measured flag width per home.

usage: overnight_monitor.py [--log <path>] [--interval <s>]
"""
import json
import os
import sys
import tkinter as tk
from tkinter import ttk
from datetime import datetime

import matplotlib
matplotlib.use("TkAgg")
from matplotlib.figure import Figure
from matplotlib.backends.backend_tkagg import FigureCanvasTkAgg

HERE = os.path.dirname(os.path.abspath(__file__))
LOG = os.path.join(HERE, "..", "reports", "newring", "overnight.jsonl")
INTERVAL = 3000

_a = sys.argv[1:]
for i, x in enumerate(_a):
    if x == "--log" and i + 1 < len(_a):
        LOG = _a[i + 1]
    elif x == "--interval" and i + 1 < len(_a):
        INTERVAL = int(float(_a[i + 1]) * 1000)
LOG = os.path.abspath(LOG)

BAND_MIN = 6


def parse_ts(s):
    try:
        return datetime.strptime(s, "%Y-%m-%dT%H:%M:%S")
    except Exception:
        return None


class App:
    def __init__(self, root):
        self.root = root
        root.title("New ring — overnight bake")
        root.geometry("1180x820")

        self.offset = 0
        self.recals = []     # (dt, band, t_op, below_gate)
        self.homes = []      # (dt, ok, switch, t_op, band, secs)
        self.events = []
        self.start_dt = None
        self.session_end = None

        top = ttk.Frame(root, padding=8)
        top.pack(fill=tk.X)
        self.lbl = {}
        for key, text in (("elapsed", "elapsed"), ("cycles", "homes"),
                          ("rate", "success"), ("t_op", "T_op"),
                          ("band", "band"), ("gate", "gate")):
            f = ttk.Frame(top)
            f.pack(side=tk.LEFT, padx=14)
            ttk.Label(f, text=text.upper(),
                      font=("Segoe UI", 8)).pack(anchor="w")
            v = ttk.Label(f, text="—", font=("Segoe UI", 16, "bold"))
            v.pack(anchor="w")
            self.lbl[key] = v

        self.status = ttk.Label(root, text=f"watching {LOG}",
                                font=("Consolas", 8))
        self.status.pack(fill=tk.X, padx=8)

        self.fig = Figure(figsize=(11, 6.4), dpi=100)
        self.fig.subplots_adjust(hspace=0.45, left=0.08, right=0.95,
                                 top=0.96, bottom=0.08)
        self.ax_band = self.fig.add_subplot(311)
        self.ax_res = self.fig.add_subplot(312)
        self.ax_w = self.fig.add_subplot(313)
        self.canvas = FigureCanvasTkAgg(self.fig, master=root)
        self.canvas.get_tk_widget().pack(fill=tk.BOTH, expand=True,
                                         padx=8, pady=4)

        ttk.Label(root, text="recent events",
                  font=("Segoe UI", 8)).pack(anchor="w", padx=10)
        self.txt = tk.Text(root, height=7, font=("Consolas", 8))
        self.txt.pack(fill=tk.X, padx=8, pady=(0, 8))

        self.tick()

    # ---------------------------------------------------------------- data
    def read_new(self):
        if not os.path.exists(LOG):
            return False
        try:
            size = os.path.getsize(LOG)
            if size < self.offset:      # file replaced
                self.offset = 0
                self.recals.clear()
                self.homes.clear()
                self.events.clear()
            with open(LOG, "r", encoding="utf-8") as f:
                f.seek(self.offset)
                chunk = f.read()
                self.offset = f.tell()
        except Exception as e:
            self.status.config(text=f"read error: {e}")
            return False

        got = False
        for line in chunk.splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                r = json.loads(line)
            except Exception:
                continue
            got = True
            dt = parse_ts(r.get("ts", ""))
            k = r.get("kind")
            if k == "session_start":
                self.start_dt = dt
                self.events.append((dt, "session start"))
            elif k == "session_end":
                self.session_end = r
                self.events.append(
                    (dt, f"session end: {r.get('ok')} ok / {r.get('fail')} fail"))
            elif k == "recal":
                if r.get("ok"):
                    self.recals.append((dt, r["band"], r["t_op"],
                                        r.get("below_gate", False)))
                    self.events.append(
                        (dt, f"recal: floor={r['t_floor']} shoulder="
                             f"{r['t_shoulder']} band={r['band']} T_op={r['t_op']}"
                             + ("  [BELOW GATE]" if r.get("below_gate") else "")))
                else:
                    self.recals.append((dt, 0, None, True))
                    self.events.append((dt, f"recal FAILED: {r.get('reason')}"))
            elif k == "home":
                self.homes.append((dt, bool(r.get("ok")), r.get("switch"),
                                   r.get("t_op"), r.get("band"),
                                   r.get("secs")))
                if not r.get("ok"):
                    self.events.append(
                        (dt, f"HOME FAIL cycle {r.get('cycle')} "
                             f"T={r.get('t_op')} band={r.get('band')}: "
                             f"{r.get('msg')}"))
            elif k == "exception":
                self.events.append((dt, f"EXCEPTION: {r.get('err')}"))
            elif k == "idle":
                self.events.append((dt, f"idle: {r.get('reason')}"))
        return got

    # ---------------------------------------------------------------- view
    def hours(self, dt):
        if dt is None or self.start_dt is None:
            return 0.0
        return (dt - self.start_dt).total_seconds() / 3600.0

    def redraw(self):
        for ax in (self.ax_band, self.ax_res, self.ax_w):
            ax.clear()

        # band + T_op
        if self.recals:
            hs = [self.hours(d) for d, *_ in self.recals]
            bands = [b for _, b, _, _ in self.recals]
            self.ax_band.plot(hs, bands, "o-", color="#2b7", lw=1.6, ms=4,
                              label="band (counts)")
            self.ax_band.axhline(BAND_MIN, color="#c33", ls="--", lw=1,
                                 label=f"gate ({BAND_MIN})")
            below = [(h, b) for h, b in zip(hs, bands) if b < BAND_MIN]
            if below:
                self.ax_band.plot([h for h, _ in below], [b for _, b in below],
                                  "o", color="#c33", ms=7, zorder=5)
            ax2 = self.ax_band.twinx()
            tops = [t if t is not None else float("nan")
                    for _, _, t, _ in self.recals]
            ax2.plot(hs, tops, "s--", color="#69c", lw=1, ms=3, label="T_op")
            ax2.set_ylabel("T_op", color="#69c", fontsize=8)
            ax2.tick_params(labelsize=7)
            self.ax_band.legend(fontsize=7, loc="upper left")
        self.ax_band.set_ylabel("band (counts)", fontsize=8)
        self.ax_band.set_title("detectability band vs time  "
                               "(red = below gate: production would refuse)",
                               fontsize=9)
        self.ax_band.grid(alpha=0.3)
        self.ax_band.tick_params(labelsize=7)

        # outcomes
        if self.homes:
            hs = [self.hours(d) for d, *_ in self.homes]
            oks = [1 if ok else 0 for _, ok, *_ in self.homes]
            run, cum = 0, []
            for i, o in enumerate(oks):
                run += o
                cum.append(100.0 * run / (i + 1))
            self.ax_res.plot(hs, cum, "-", color="#37a", lw=1.5,
                             label="cumulative success %")
            fx = [h for h, o in zip(hs, oks) if not o]
            if fx:
                self.ax_res.plot(fx, [2] * len(fx), "x", color="#c33", ms=7,
                                 label=f"failures ({len(fx)})")
            self.ax_res.set_ylim(-3, 105)
            self.ax_res.legend(fontsize=7, loc="lower left")
        self.ax_res.set_ylabel("success %", fontsize=8)
        self.ax_res.grid(alpha=0.3)
        self.ax_res.tick_params(labelsize=7)

        # flag width
        wpts = [(self.hours(d), w) for d, ok, w, *_ in self.homes
                if ok and w is not None]
        if wpts:
            self.ax_w.plot([h for h, _ in wpts], [w for _, w in wpts],
                           ".", color="#846", ms=3)
        self.ax_w.set_ylabel("flag width (µsteps)", fontsize=8)
        self.ax_w.set_xlabel("hours elapsed", fontsize=8)
        self.ax_w.grid(alpha=0.3)
        self.ax_w.tick_params(labelsize=7)

        self.canvas.draw_idle()

        # headline numbers
        n = len(self.homes)
        ok = sum(1 for _, o, *_ in self.homes if o)
        last_dt = self.homes[-1][0] if self.homes else (
            self.recals[-1][0] if self.recals else None)
        self.lbl["elapsed"].config(
            text=f"{self.hours(last_dt):.2f} h" if last_dt else "—")
        self.lbl["cycles"].config(text=str(n))
        self.lbl["rate"].config(
            text=f"{100.0*ok/n:.1f}%" if n else "—")
        if self.recals:
            _, band, t_op, below = self.recals[-1]
            self.lbl["t_op"].config(text=str(t_op) if t_op else "—")
            self.lbl["band"].config(text=str(band))
            self.lbl["gate"].config(text="BELOW" if below else "ok",
                                    foreground="#c33" if below else "#2a2")
        if self.session_end:
            self.status.config(
                text=f"FINISHED — {self.session_end.get('ok')} ok / "
                     f"{self.session_end.get('fail')} fail over "
                     f"{self.session_end.get('hours')} h   ({LOG})")
        else:
            self.status.config(text=f"watching {LOG}")

        self.txt.delete("1.0", tk.END)
        for dt, msg in self.events[-40:]:
            stamp = dt.strftime("%H:%M:%S") if dt else "--:--:--"
            self.txt.insert(tk.END, f"{stamp}  {msg}\n")
        self.txt.see(tk.END)

    def tick(self):
        try:
            if self.read_new():
                self.redraw()
            elif not self.homes and not self.recals:
                self.status.config(
                    text=f"waiting for data — {LOG}")
        except Exception as e:
            self.status.config(text=f"monitor error: {e}")
        self.root.after(INTERVAL, self.tick)


if __name__ == "__main__":
    root = tk.Tk()
    App(root)
    root.mainloop()

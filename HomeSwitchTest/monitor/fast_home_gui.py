#!/usr/bin/env python3
"""Fast-home GUI for the Side-A optical home-switch bench rig.

A focused front-end for the `O` fast home + backlash routine
(src/bench_main.cpp): a ring-dial visualisation of the prism motion, a jog
panel, one HOME button, and a full report card when the routine finishes
(threshold calibration, edges, switch size, backlash, phase timings, and a
run history with repeatability).

    python fast_home_gui.py             # auto-detect the ST-Link port
    python fast_home_gui.py --port COM3

Reuses SerialClient / port autodetection from home_switch_gui.py.
"""

import argparse
import math
import queue
import time
from collections import deque

import tkinter as tk
from tkinter import ttk, messagebox

from home_switch_gui import (SerialClient, autodetect_port, BAUD_DEFAULT,
                             DEFAULT_USTEPS_PER_REV, JOG_MAX_SPEED)

RING = 430          # canvas size (px)
TRAIL_N = 48        # motion-trail samples

PHASE_TEXT = {
    "cal":      "calibrating threshold…",
    "seek":     "seeking the flag…",
    "gate":     "validating the feature…",
    "edges":    "measuring the flag edges…",
    "backlash": "measuring backlash…",
    "park":     "parking at home…",
}


class FastHomeGUI:
    def __init__(self, root, port, baud):
        self.root = root
        self.client = SerialClient()
        self.baud = baud
        self.usteps_per_rev = DEFAULT_USTEPS_PER_REV

        # live state (from the S stream)
        self.pos = 0
        self.level = 0
        self.thr = 0
        self.running = 0
        self.enabled = 0
        self.fault = 0
        self.homed = 0
        self.trail = deque(maxlen=TRAIL_N)

        # O-routine state
        self.o_active = False
        self.o_t0 = 0.0
        self.o_events = []          # (host_time, tag) for phase timings
        self.o_cal = None           # (c1, c2, bg, T)
        self.o_thr_bumps = []
        self.o_lead = None
        self.o_trail = None
        self.flag_arc = None        # (half_width_usteps) once homed
        self.history = []           # dicts of finished runs

        self._jog_last_sent = 0.0
        self._jog_last_val = 0

        root.title("Fast home — Side A bench")
        root.protocol("WM_DELETE_WINDOW", self._on_close)
        self._build(port)
        self._drain()

    # ------------------------------------------------------------------ UI
    def _build(self, port):
        top = ttk.Frame(self.root, padding=(8, 6))
        top.pack(fill="x")

        ttk.Label(top, text="Port").pack(side="left")
        self.port_var = tk.StringVar(value=port or (autodetect_port() or ""))
        ttk.Entry(top, textvariable=self.port_var, width=10).pack(side="left", padx=(4, 6))
        self.connect_btn = ttk.Button(top, text="Connect", command=self._toggle_connect)
        self.connect_btn.pack(side="left")
        self.enable_var = tk.IntVar(value=0)
        ttk.Checkbutton(top, text="Motor coils", variable=self.enable_var,
                        command=lambda: self._send(f"E {self.enable_var.get()}")
                        ).pack(side="left", padx=10)

        self.lamps = {}
        for name in ("FLAG", "RUN", "FAULT", "HOMED"):
            f = tk.Frame(top, width=14, height=14, bg="#bbb",
                         highlightthickness=1, highlightbackground="#888")
            f.pack(side="right", padx=(2, 6))
            ttk.Label(top, text=name).pack(side="right")
            self.lamps[name] = f

        body = ttk.Frame(self.root, padding=8)
        body.pack(fill="both", expand=True)

        # --- ring visualisation ------------------------------------------
        left = ttk.Frame(body)
        left.pack(side="left", fill="y")
        self.canvas = tk.Canvas(left, width=RING, height=RING,
                                bg="#fafafa", highlightthickness=0)
        self.canvas.pack()
        self._build_ring()

        # --- right column: home + report ----------------------------------
        right = ttk.Frame(body, padding=(12, 0, 0, 0))
        right.pack(side="left", fill="both", expand=True)

        hf = ttk.LabelFrame(right, text="Home routine (O)", padding=8)
        hf.pack(fill="x")
        row = ttk.Frame(hf)
        row.pack(fill="x")
        self.home_btn = ttk.Button(row, text="⌂  HOME NOW",
                                   command=self._run_home)
        self.home_btn.pack(side="left")
        ttk.Button(row, text="Stop", command=lambda: self._send("X")
                   ).pack(side="left", padx=6)
        self.recal_var = tk.IntVar(value=0)
        ttk.Checkbutton(row, text="force recalibrate",
                        variable=self.recal_var).pack(side="left", padx=6)
        self.phase_lbl = ttk.Label(hf, text="idle", font=("Segoe UI", 10, "italic"))
        self.phase_lbl.pack(anchor="w", pady=(6, 0))

        rf = ttk.LabelFrame(right, text="Last run report", padding=8)
        rf.pack(fill="x", pady=(8, 0))
        self.verdict_lbl = tk.Label(rf, text="— no run yet —",
                                    font=("Segoe UI", 13, "bold"), fg="#666")
        self.verdict_lbl.pack(anchor="w")
        self.report_grid = ttk.Frame(rf)
        self.report_grid.pack(fill="x", pady=(4, 0))
        self.report_rows = {}
        for r, key in enumerate(("Home shift", "Switch size", "Backlash",
                                 "Threshold", "Calibration", "Phases")):
            ttk.Label(self.report_grid, text=key, width=11,
                      font=("Segoe UI", 9, "bold")).grid(row=r, column=0, sticky="nw")
            v = ttk.Label(self.report_grid, text="–", wraplength=380,
                          justify="left")
            v.grid(row=r, column=1, sticky="w", padx=(6, 0))
            self.report_rows[key] = v

        hist = ttk.LabelFrame(right, text="Run history", padding=(8, 4))
        hist.pack(fill="both", expand=True, pady=(8, 0))
        cols = ("t", "ok", "shift", "switch", "backlash", "T", "dur")
        self.tree = ttk.Treeview(hist, columns=cols, show="headings", height=6)
        for c, w, txt in (("t", 60, "time"), ("ok", 36, "ok"),
                          ("shift", 84, "shift (µst)"),
                          ("switch", 74, "switch"), ("backlash", 74, "backlash"),
                          ("T", 40, "T"), ("dur", 60, "dur (s)")):
            self.tree.heading(c, text=txt)
            self.tree.column(c, width=w, anchor="e", stretch=(c == "t"))
        self.tree.pack(fill="both", expand=True)
        self.spread_lbl = ttk.Label(hist, text="")
        self.spread_lbl.pack(anchor="w")

        # --- jog panel -----------------------------------------------------
        jog = ttk.LabelFrame(self.root, text="Jog / go-to", padding=8)
        jog.pack(fill="x", padx=8, pady=(0, 8))
        for d in (-10.0, -1.0, -0.1):
            ttk.Button(jog, text=f"{d:+g}°", width=6,
                       command=lambda d=d: self._step(d)).pack(side="left", padx=2)
        self.jog_var = tk.DoubleVar(value=0.0)
        self.jog = ttk.Scale(jog, from_=-100, to=100, variable=self.jog_var,
                             length=260, command=self._jog_drag)
        self.jog.pack(side="left", padx=10, fill="x", expand=True)
        self.jog.bind("<ButtonRelease-1>", self._jog_release)
        for d in (0.1, 1.0, 10.0):
            ttk.Button(jog, text=f"{d:+g}°", width=6,
                       command=lambda d=d: self._step(d)).pack(side="left", padx=2)
        ttk.Label(jog, text="  Go to").pack(side="left")
        self.goto_var = tk.StringVar(value="0.0")
        ttk.Entry(jog, textvariable=self.goto_var, width=8).pack(side="left", padx=3)
        ttk.Label(jog, text="°").pack(side="left")
        ttk.Button(jog, text="Go", command=self._goto).pack(side="left", padx=3)
        stop = tk.Button(jog, text="STOP", bg="#c0392b", fg="white",
                         font=("Segoe UI", 10, "bold"),
                         command=lambda: self._send("X"))
        stop.pack(side="left", padx=(14, 2))

        self.log_lbl = ttk.Label(self.root, text="", foreground="#666")
        self.log_lbl.pack(fill="x", padx=10, pady=(0, 4))

        if port:
            self.root.after(200, self._toggle_connect)

    # ------------------------------------------------------------- ring viz
    def _build_ring(self):
        c = self.canvas
        cx = cy = RING / 2
        self.ring_r = r = RING / 2 - 34
        c.create_oval(cx - r, cy - r, cx + r, cy + r, width=2, outline="#444")
        for deg in range(0, 360, 30):
            a = math.radians(90 - deg)
            x1 = cx + (r - 7) * math.cos(a); y1 = cy - (r - 7) * math.sin(a)
            x2 = cx + r * math.cos(a);       y2 = cy - r * math.sin(a)
            c.create_line(x1, y1, x2, y2, fill="#999")
            xt = cx + (r + 15) * math.cos(a); yt = cy - (r + 15) * math.sin(a)
            c.create_text(xt, yt, text=f"{deg}°", fill="#777",
                          font=("Segoe UI", 8))
        # dynamic items (created once, coords updated live)
        self.flag_item = c.create_arc(cx - r, cy - r, cx + r, cy + r,
                                      start=0, extent=0, style="arc",
                                      width=9, outline="#e74c3c", state="hidden")
        self.home_item = c.create_line(0, 0, 0, 0, width=3, fill="#27ae60",
                                       state="hidden")
        self.edge_items = [c.create_oval(0, 0, 0, 0, fill="#e74c3c", width=0,
                                         state="hidden") for _ in range(2)]
        self.trail_items = [c.create_oval(0, 0, 0, 0, width=0, state="hidden")
                            for _ in range(TRAIL_N)]
        self.needle = c.create_line(cx, cy, cx, cy - r + 12, width=3,
                                    fill="#d35400", arrow="last")
        c.create_oval(cx - 4, cy - 4, cx + 4, cy + 4, fill="#d35400", width=0)
        self.center_txt = c.create_text(cx, cy + 34, text="",
                                        font=("Consolas", 12), fill="#333")
        self.center_sub = c.create_text(cx, cy + 54, text="",
                                        font=("Consolas", 9), fill="#888")

    def _deg(self, usteps):
        return (usteps % self.usteps_per_rev) / self.usteps_per_rev * 360.0

    def _xy(self, deg, radius):
        cx = cy = RING / 2
        a = math.radians(90 - deg)
        return cx + radius * math.cos(a), cy - radius * math.sin(a)

    def _redraw_ring(self):
        c = self.canvas
        cx = cy = RING / 2
        r = self.ring_r
        deg = self._deg(self.pos)
        x, y = self._xy(deg, r - 12)
        c.coords(self.needle, cx, cy, x, y)
        c.itemconfig(self.needle,
                     fill="#27ae60" if self.level else "#d35400")
        c.itemconfig(self.center_txt, text=f"{self.pos - round(self.pos / self.usteps_per_rev) * self.usteps_per_rev if self.homed else self.pos:+d} µst")
        c.itemconfig(self.center_sub, text=f"{deg:6.2f}°   thr {self.thr}")

        # motion trail (fading dots)
        shades = ("#d35400", "#dd6a22", "#e58044", "#ec9666", "#f2ac88",
                  "#f7c2aa", "#fbd8cc", "#fdeee6")
        n = len(self.trail)
        for i, item in enumerate(self.trail_items):
            if i < n:
                d = self.trail[n - 1 - i]
                x, y = self._xy(d, r - 22)
                s = max(1.5, 4 - i * 0.06)
                c.coords(item, x - s, y - s, x + s, y + s)
                c.itemconfig(item, state="normal",
                             fill=shades[min(i * len(shades) // TRAIL_N,
                                             len(shades) - 1)])
            else:
                c.itemconfig(item, state="hidden")

        # flag arc + home marker (post-home frame: flag centred at 0 deg)
        if self.flag_arc:
            half_deg = self.flag_arc / self.usteps_per_rev * 360.0
            c.itemconfig(self.flag_item, state="normal",
                         start=90 - half_deg, extent=2 * half_deg)
            x1, y1 = self._xy(0, r - 16)
            x2, y2 = self._xy(0, r + 16)
            c.coords(self.home_item, x1, y1, x2, y2)
            c.itemconfig(self.home_item, state="normal")

    # -------------------------------------------------------------- actions
    def _toggle_connect(self):
        if self.client.connected:
            self.client.close()
            self.connect_btn.config(text="Connect")
            return
        port = self.port_var.get().strip() or autodetect_port()
        if not port:
            messagebox.showerror("No port", "No serial port found.")
            return
        try:
            self.client.open(port, self.baud)
        except Exception as e:
            messagebox.showerror("Connect failed", str(e))
            return
        self.connect_btn.config(text="Disconnect")
        self._send("M 1")
        self._send("P")

    def _send(self, text):
        self.client.send(text)

    def _run_home(self):
        if self.o_active:
            return
        self.o_active = True
        self.o_t0 = time.time()
        self.o_events = [(self.o_t0, "begin")]
        self.o_cal = None
        self.o_thr_bumps = []
        self.o_lead = self.o_trail = None
        for item in self.edge_items:
            self.canvas.itemconfig(item, state="hidden")
        self.home_btn.state(["disabled"])
        self.enable_var.set(1)
        self.phase_lbl.config(text=PHASE_TEXT["seek"])
        self._send("O 0 0 0 0 " + ("1" if self.recal_var.get() else "0"))

    def _step(self, ddeg):
        if self.o_active:
            return
        self.enable_var.set(1)
        du = int(round(ddeg / 360.0 * self.usteps_per_rev))
        self._send(f"J {du} {JOG_MAX_SPEED}")

    def _goto(self):
        if self.o_active:
            return
        try:
            deg = float(self.goto_var.get())
        except ValueError:
            return
        self.enable_var.set(1)
        self._send(f"G {int(round(deg / 360.0 * self.usteps_per_rev))} {JOG_MAX_SPEED}")

    def _jog_drag(self, _val):
        if self.o_active:
            return
        v = self.jog_var.get()
        speed = int(math.copysign((abs(v) / 100.0) ** 2 * JOG_MAX_SPEED, v))
        now = time.time()
        if speed != self._jog_last_val and now - self._jog_last_sent > 0.12:
            self.enable_var.set(1)
            self._send(f"C {speed}")
            self._jog_last_sent = now
            self._jog_last_val = speed

    def _jog_release(self, _ev):
        self.jog_var.set(0)
        self._send("C 0")
        self._jog_last_val = 0

    # ------------------------------------------------------------ telemetry
    def _drain(self):
        try:
            while True:
                self._handle(self.client.rx.get_nowait())
        except queue.Empty:
            pass
        self._redraw_ring()
        self.root.after(33, self._drain)

    def _handle(self, line):
        try:
            if line.startswith("S,"):
                self._handle_s(line)
            elif line.startswith("O,"):
                self._handle_o(line)
            elif line.startswith("L,"):
                self.log_lbl.config(text=line[4:] if len(line) > 4 else line)
            elif line.startswith("#") and "usteps_per_rev=" in line:
                for tok in line.split():
                    if tok.startswith("usteps_per_rev="):
                        self.usteps_per_rev = int(tok.split("=")[1])
        except Exception:
            pass

    def _handle_s(self, line):
        p = line.split(",")
        if len(p) < 10:
            return
        self.level, self.thr = int(p[2]), int(p[3])
        newpos = int(p[4])
        self.running, self.enabled = int(p[6]), int(p[7])
        self.fault, self.homed = int(p[8]), int(p[9])
        if self.running or newpos != self.pos:
            self.trail.append(self._deg(newpos))
        self.pos = newpos
        for name, on, col in (("FLAG", self.level, "#27ae60"),
                              ("RUN", self.running, "#2980b9"),
                              ("FAULT", self.fault, "#c0392b"),
                              ("HOMED", self.homed, "#27ae60")):
            self.lamps[name].config(bg=col if on else "#bbb")
        if self.enable_var.get() != self.enabled:
            self.enable_var.set(self.enabled)

    def _handle_o(self, line):
        p = line.split(",")
        tag = p[1]
        now = time.time()
        self.o_events.append((now, tag))
        if tag == "cal":
            self.o_cal = tuple(int(x) for x in p[2:6])
            self.phase_lbl.config(text=PHASE_TEXT["seek"])
        elif tag == "thr":
            self.o_thr_bumps.append(f"{p[2]}{'↑' if p[3] == 'up' else '↓'}")
        elif tag == "seek":
            self.phase_lbl.config(text=PHASE_TEXT["gate"])
        elif tag == "gate":
            self.phase_lbl.config(text=PHASE_TEXT["edges"])
        elif tag == "edge":
            pos = int(p[3])
            if p[2] == "0":
                self.o_lead = pos
            else:
                self.o_trail = pos
                self.phase_lbl.config(text=PHASE_TEXT["backlash"])
            x, y = self._xy(self._deg(pos), self.ring_r)
            item = self.edge_items[int(p[2])]
            self.canvas.coords(item, x - 5, y - 5, x + 5, y + 5)
            self.canvas.itemconfig(item, state="normal")
        elif tag == "backlash":
            self.phase_lbl.config(text=PHASE_TEXT["park"])
        elif tag == "done":
            self._finish_o(p, line)

    def _finish_o(self, p, line):
        self.o_active = False
        self.home_btn.state(["!disabled"])
        ok = p[2] == "1"
        msg = line.split('"')[1] if '"' in line else ""
        home, lead, trail_e = int(p[3]), int(p[4]), int(p[5])
        switch, backlash, T, ms = int(p[6]), int(p[7]), int(p[8]), int(p[9])
        rev = self.usteps_per_rev

        if ok:
            self.flag_arc = switch / 2
            shift = home - round(home / rev) * rev
            self.verdict_lbl.config(text=f"PASS — homed in {ms / 1000.0:.1f} s",
                                    fg="#27ae60")
            deg = 360.0 / rev
            self.report_rows["Home shift"].config(
                text=f"{shift:+d} µsteps ({shift * deg:+.3f}°) vs previous home"
                     + (f"  •  found after {abs(home)} µsteps of travel"
                        if abs(home) > 3000 else ""))
            self.report_rows["Switch size"].config(
                text=f"{switch} µsteps ({switch * deg:.2f}°), edges at "
                     f"{lead - home:+d} / {trail_e - home:+d} µsteps")
            self.report_rows["Backlash"].config(
                text=f"{backlash} µsteps ({backlash * deg:.2f}°)")
            cal = (f"cold: bg {self.o_cal[2]} → T {self.o_cal[3]}"
                   if self.o_cal else "warm (cached threshold)")
            if self.o_thr_bumps:
                cal += "  bumps: " + " ".join(self.o_thr_bumps)
            self.report_rows["Threshold"].config(text=str(T))
            self.report_rows["Calibration"].config(text=cal)
            self.report_rows["Phases"].config(text=self._phase_summary())
        else:
            shift = None
            self.verdict_lbl.config(text=f"FAIL — {msg}", fg="#c0392b")
            for k in self.report_rows:
                self.report_rows[k].config(text="–")
            self.report_rows["Phases"].config(text=self._phase_summary())
        self.phase_lbl.config(text=f"done: {msg}")

        self.history.append({"ok": ok, "shift": shift, "switch": switch,
                             "backlash": backlash, "T": T, "ms": ms})
        self.tree.insert("", 0, values=(
            time.strftime("%H:%M:%S"), "✓" if ok else "✗",
            f"{shift:+d}" if shift is not None else "–",
            switch if ok else "–", backlash if ok else "–",
            T, f"{ms / 1000.0:.1f}"))
        shifts = [h["shift"] for h in self.history if h["ok"] and h["shift"] is not None]
        if len(shifts) >= 3:
            m = sum(shifts) / len(shifts)
            sd = (sum((s - m) ** 2 for s in shifts) / len(shifts)) ** 0.5
            mx = max(abs(s) for s in shifts)
            deg = 360.0 / self.usteps_per_rev
            self.spread_lbl.config(
                text=f"repeatability over {len(shifts)} runs: σ = {sd:.1f} µsteps"
                     f" ({sd * deg:.4f}°) · max |shift| = {mx} ({mx * deg:.4f}°)")

    def _phase_summary(self):
        if len(self.o_events) < 2:
            return "–"
        names = {"begin": None, "cal": "cal", "seek": "seek", "gate": "gates",
                 "edge": "edges", "backlash": "backlash", "done": "park+finish"}
        out = []
        for (t0, tag0), (t1, _tag1) in zip(self.o_events, self.o_events[1:]):
            label = names.get(tag0, tag0)
            dt = t1 - t0
            if label and dt >= 0.05:
                out.append(f"{label} {dt:.1f}s")
        # merge duplicate consecutive labels (e.g. two edge segments)
        merged = []
        for part in out:
            name = part.rsplit(" ", 1)[0]
            if merged and merged[-1][0] == name:
                merged[-1][1] += float(part.rsplit(" ", 1)[1][:-1])
            else:
                merged.append([name, float(part.rsplit(" ", 1)[1][:-1])])
        return " · ".join(f"{n} {v:.1f}s" for n, v in merged)

    def _on_close(self):
        try:
            self._send("C 0")
            self.client.close()
        finally:
            self.root.destroy()


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", default=None, help="serial port (auto-detect if omitted)")
    ap.add_argument("--baud", type=int, default=BAUD_DEFAULT)
    args = ap.parse_args()
    root = tk.Tk()
    FastHomeGUI(root, args.port or autodetect_port(), args.baud)
    root.mainloop()


if __name__ == "__main__":
    main()

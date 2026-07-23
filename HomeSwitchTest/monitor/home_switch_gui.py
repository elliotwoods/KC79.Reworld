#!/usr/bin/env python3
"""Guided GUI for the Side-A optical home-switch bench rig.

Pairs with the `home_switch_bench` firmware (src/bench_main.cpp). It presents a
4-step WIZARD for finding the optimum home position + threshold:

  1. Get on home   - jog (universal panel) to drive the ring gear onto the home flag
  2. Scan shape     - scan +/-N microsteps, plot position->signal, save to disk
  3. Home (auto)    - self-calibrate the dip threshold + two-edge home to its centre
  4. Test           - run a homing routine and jog anywhere in the cycle

An ADVANCED tab keeps all the manual controls + raw plots. A persistent status
bar (position, flag, threshold, motor state) is always visible.

Only Python stdlib + pyserial + matplotlib are needed (Tkinter ships with the
standard python.org / conda CPython on Windows and macOS).

    python home_switch_gui.py               # auto-detect the ST-Link port
    python home_switch_gui.py --port COM3
"""

import argparse
import csv
import json
import math
import queue
import sys
import threading
import time
from collections import deque

import serial
import serial.tools.list_ports

import tkinter as tk
from tkinter import ttk
from tkinter import filedialog, messagebox

import matplotlib
matplotlib.use("TkAgg")
from matplotlib.figure import Figure
from matplotlib.backends.backend_tkagg import FigureCanvasTkAgg

BAUD_DEFAULT = 115200
VREF = 3.3
# Exact rational, rounded (the double-truncated integer form is 7.9 short/rev).
# 32:1 modules; 16:1 modules measure 92,252. The firmware banner overrides this
# on connect and again whenever fast home's motor detection changes the ratio.
DEFAULT_USTEPS_PER_REV = (32 * 118 * 9759 * 32 + (296 * 21) // 2) // (296 * 21)  # 189704
LIVE_WINDOW_S = 30.0
SWEEP_WINDOW_S = 60.0
JOG_MAX_SPEED = 14080        # jog-dial full-scale (usteps/s, ~27 deg/s)

SCAN_DEFAULT_RANGE = 500     # +/- microsteps scanned around the current position
SCAN_DEFAULT_STEP = 10       # microsteps between samples (~101 points)
SCAN_SAT_DUTY = 250          # crossing >= this is treated as a saturated / clipped plateau

WIZARD_TITLES = [
    "Get on the home flag",
    "Scan the home shape",
    "Home (auto)",
    "Test homing",
]


def duty_to_volts(duty):
    return VREF * duty / 255.0


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


# ---------------------------------------------------------------------------
# Serial client - background reader thread, line queue
# ---------------------------------------------------------------------------
class SerialClient:
    def __init__(self):
        self.ser = None
        self.rx = queue.Queue()
        self._reader = None
        self._stop = threading.Event()

    @property
    def connected(self):
        return self.ser is not None and self.ser.is_open

    def open(self, port, baud):
        s = serial.Serial()
        s.port = port
        s.baudrate = baud
        s.timeout = 0.1
        s.dtr = False          # don't reset the board on open
        s.rts = False
        s.open()
        self.ser = s
        self._stop.clear()
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()

    def close(self):
        self._stop.set()
        if self._reader:
            self._reader.join(timeout=1.0)
            self._reader = None
        if self.ser:
            try:
                self.ser.close()
            except Exception:
                pass
            self.ser = None

    def _read_loop(self):
        buf = b""
        while not self._stop.is_set():
            try:
                data = self.ser.read(256)
            except Exception:
                break
            if not data:
                continue
            buf += data
            while b"\n" in buf:
                line, buf = buf.split(b"\n", 1)
                self.rx.put(line.decode("utf-8", "replace").strip())

    def send(self, text):
        if self.connected:
            try:
                self.ser.write((text + "\n").encode("ascii", "replace"))
            except Exception:
                pass


# ---------------------------------------------------------------------------
# Application
# ---------------------------------------------------------------------------
class BenchGUI:
    def __init__(self, root, initial_port, baud):
        self.root = root
        self.baud = baud
        self.client = SerialClient()
        self.usteps_per_rev = DEFAULT_USTEPS_PER_REV

        # live telemetry
        self.t0 = time.monotonic()
        self.live_t, self.live_deg, self.live_level = deque(), deque(), deque()
        self.sweep_t, self.sweep_duty = deque(), deque()
        self.recent_duty = deque(maxlen=8)
        self.status = dict(level=0, thr=0, pos=0, deg=0.0,
                           running=0, enabled=0, fault=0, homed=0)

        # wizard / search / homing state
        self.wiz_step = 0
        self.search_active = False
        self.awaiting_sweep_here = False
        self.search_pos, self.search_cross = [], []
        self.search_best = None

        # home-shape scan state (parallel lists, like search_pos/search_cross)
        self.scan_active = False
        self.scan_done = False              # a saveable scan (complete or stopped) is in hand
        self.scan_center = None             # motor pos at scan centre (from K,begin)
        self.scan_n = SCAN_DEFAULT_RANGE    # +/- range usteps (from K,begin)
        self.scan_step = SCAN_DEFAULT_STEP  # step usteps (from K,begin)
        self.scan_expected = 0              # planned sample count (from K,begin)
        self.scan_pos, self.scan_off = [], []   # absolute pos, offset-from-centre
        self.scan_cross = []                # crossing duty (may be -1)
        self.scan_lo, self.scan_hi = [], []     # comparator rail states at duty 0 / 255
        self.scan_dmin = 200                    # duty band swept + shown on the scan plot
        self.scan_dmax = 255

        # auto-home (self-calibrating dip home) state
        self.autohome_active = False
        self.autohome_done = False
        self.ah_char_pos, self.ah_char_cross = [], []   # coarse characterisation curve
        self.ah_thr = self.ah_cmin = self.ah_pmin = self.ah_dipdepth = None
        self.ah_edges = []                              # list of (which, pos)
        self.ah_home = None
        self.ah_result = None                           # last A,done, for revisit
        self.repeat_cmd = "H"
        self.repeat_remaining = 0
        self.repeat_homes = []
        self.last_home = None
        self._last_jog_v = 0
        self._last_jog_t = 0.0
        self.capture_home = None
        self.capture_nothome = None
        self.recommended = None
        self.sweeping = False

        root.title("Home-switch bench - Side A")
        root.geometry("1080x720")
        root.protocol("WM_DELETE_WINDOW", self.on_close)

        self._build_ui(initial_port)
        self._last_redraw = 0.0
        self.root.after(50, self._drain)

    # ====================================================================
    # UI construction
    # ====================================================================
    def _build_ui(self, initial_port):
        self._build_statusbar(initial_port)     # packed to the bottom first
        nb = ttk.Notebook(self.root)
        nb.pack(fill="both", expand=True)
        self.tabs = nb

        self.wizard_tab = ttk.Frame(nb)
        self.advanced_tab = ttk.Frame(nb)
        nb.add(self.wizard_tab, text="  Wizard  ")
        nb.add(self.advanced_tab, text="  Advanced  ")

        self._build_wizard(self.wizard_tab)
        self._build_advanced(self.advanced_tab)
        self._wiz_goto(0)

    # --- persistent status bar -------------------------------------------
    def _build_statusbar(self, initial_port):
        bar = ttk.Frame(self.root, relief="groove", padding=(6, 3))
        bar.pack(side="bottom", fill="x")

        self.port_var = tk.StringVar(value=initial_port or "")
        ports = [p.device for p in sorted(serial.tools.list_ports.comports())]
        self.port_combo = ttk.Combobox(bar, textvariable=self.port_var,
                                       values=ports, width=12)
        self.port_combo.pack(side="left")
        self.connect_btn = ttk.Button(bar, text="Connect", width=11,
                                      command=self._toggle_connect)
        self.connect_btn.pack(side="left", padx=(4, 10))

        self.flag_lbl = tk.Label(bar, text=" -- ", width=12,
                                 font=("TkDefaultFont", 10, "bold"),
                                 background="#cccccc")
        self.flag_lbl.pack(side="left", padx=4)
        self.pos_lbl = ttk.Label(bar, text="pos --  (--)", width=24)
        self.pos_lbl.pack(side="left", padx=8)
        self.thr_stat_lbl = ttk.Label(bar, text="thr --", width=10)
        self.thr_stat_lbl.pack(side="left", padx=4)
        self.motor_lbl = ttk.Label(bar, text="", width=22)
        self.motor_lbl.pack(side="left", padx=8)
        self.fault_lbl = tk.Label(bar, text="FAULT", width=6, background="#cccccc")
        self.fault_lbl.pack(side="right")

    # --- wizard ----------------------------------------------------------
    def _build_wizard(self, parent):
        header = ttk.Frame(parent, padding=(8, 8, 8, 2))
        header.pack(fill="x")
        self.wiz_title = ttk.Label(header, text="", font=("TkDefaultFont", 13, "bold"))
        self.wiz_title.pack(side="left")
        self.wiz_back = ttk.Button(header, text="< Back", command=self._wiz_back)
        self.wiz_next = ttk.Button(header, text="Next >", command=self._wiz_next)
        self.wiz_next.pack(side="right")
        self.wiz_back.pack(side="right", padx=4)
        self.wiz_progress = ttk.Label(header, text="")
        self.wiz_progress.pack(side="right", padx=12)

        # universal motion panel: packed to the bottom (before the body) so it is
        # visible on every step, outside the swappable per-step frames.
        self._build_wiz_motion(parent)

        body = ttk.Frame(parent, padding=6)
        body.pack(fill="both", expand=True)
        left = ttk.Frame(body, width=340)
        left.pack(side="left", fill="y")
        left.pack_propagate(False)
        right = ttk.Frame(body)
        right.pack(side="left", fill="both", expand=True, padx=(8, 0))

        # one reconfigurable plot for the whole wizard
        self.wiz_fig = Figure(figsize=(5.5, 4.4), tight_layout=True)
        self.wiz_canvas = FigureCanvasTkAgg(self.wiz_fig, master=right)
        self.wiz_canvas.get_tk_widget().pack(fill="both", expand=True)

        # step control panels (only one shown at a time)
        self.step_frames = [ttk.Frame(left) for _ in range(4)]
        self._build_step1(self.step_frames[0])
        self._build_step2(self.step_frames[1])
        self._build_step3(self.step_frames[2])
        self._build_step4(self.step_frames[3])

    def _build_step1(self, f):
        ttk.Label(f, text="Use the jog dial at the bottom to drive the ring gear\n"
                          "onto the home flag. Speed follows the dial; release snaps\n"
                          "to STOP. Watch the live position / FLAG plot on the right.",
                  justify="left").pack(anchor="w", pady=(0, 8))
        ttk.Label(f, text="When FLAG PRESENT shows in the status bar, click Next.",
                  foreground="#555", wraplength=320, justify="left").pack(anchor="w", pady=8)

    def _build_step2(self, f):
        ttk.Label(f, text="Scan the home flag shape: the motor sweeps +/-range in\n"
                          "steps, measuring the sensor crossing (signal strength) at\n"
                          "each position. Study the curve to choose a homing strategy.",
                  justify="left").pack(anchor="w", pady=(0, 8))
        r1 = ttk.Frame(f); r1.pack(fill="x")
        ttk.Label(r1, text="range (usteps) +/-").pack(side="left")
        self.scan_n_var = tk.StringVar(value=str(SCAN_DEFAULT_RANGE))
        ttk.Entry(r1, textvariable=self.scan_n_var, width=7).pack(side="left", padx=4)
        r2 = ttk.Frame(f); r2.pack(fill="x", pady=2)
        ttk.Label(r2, text="step (usteps)").pack(side="left")
        self.scan_step_var = tk.StringVar(value=str(SCAN_DEFAULT_STEP))
        ttk.Entry(r2, textvariable=self.scan_step_var, width=7).pack(side="left", padx=4)
        r3 = ttk.Frame(f); r3.pack(fill="x", pady=2)
        ttk.Label(r3, text="duty min / max").pack(side="left")
        self.scan_dmin_var = tk.StringVar(value=str(self.scan_dmin))
        ttk.Entry(r3, textvariable=self.scan_dmin_var, width=5).pack(side="left", padx=4)
        self.scan_dmax_var = tk.StringVar(value=str(self.scan_dmax))
        ttk.Entry(r3, textvariable=self.scan_dmax_var, width=5).pack(side="left")
        br = ttk.Frame(f); br.pack(fill="x", pady=4)
        ttk.Button(br, text="Run scan", command=self._run_scan).pack(side="left", expand=True, fill="x")
        ttk.Button(br, text="Stop", command=self._stop_scan).pack(side="left", expand=True, fill="x")
        pr = ttk.Frame(f); pr.pack(fill="x")
        ttk.Label(pr, text="progress:").pack(side="left")
        self.scan_progress_lbl = ttk.Label(pr, text="0 / 0")
        self.scan_progress_lbl.pack(side="left", padx=6)
        self.scan_save_btn = ttk.Button(f, text="Save scan...", command=self._save_scan,
                                        state="disabled")
        self.scan_save_btn.pack(fill="x", pady=(6, 0))
        self.scan_lbl = ttk.Label(f, text="", font=("TkDefaultFont", 11), justify="left")
        self.scan_lbl.pack(anchor="w", pady=8)

    def _build_step3(self, f):
        ttk.Label(f, text="Auto-home: the rig characterises the home dip, picks a\n"
                          "threshold into the dip, then two-edge homes to its centre.\n"
                          "Blue dashed = threshold; red = edges; green = home.",
                  justify="left").pack(anchor="w", pady=(0, 8))
        brow = ttk.Frame(f); brow.pack(fill="x", pady=4)
        ttk.Button(brow, text="Calibrate & home",
                   command=self._run_autohome).pack(side="left", expand=True, fill="x")
        ttk.Button(brow, text="Stop",
                   command=self._stop_autohome).pack(side="left", expand=True, fill="x")
        self.ah_lbl = ttk.Label(f, text="not homed",
                                font=("TkDefaultFont", 11), justify="left")
        self.ah_lbl.pack(anchor="w", pady=8)
        ttk.Separator(f, orient="horizontal").pack(fill="x", pady=6)
        rr = ttk.Frame(f); rr.pack(fill="x")
        ttk.Label(rr, text="repeat x").pack(side="left")
        self.ah_repeat_var = tk.StringVar(value="5")
        ttk.Entry(rr, textvariable=self.ah_repeat_var, width=4).pack(side="left")
        self.ah_repeat_btn = ttk.Button(rr, text="Repeatability",
                                        command=self._run_autohome_repeat, state="disabled")
        self.ah_repeat_btn.pack(side="left", padx=4)
        self.ah_repeat_lbl = ttk.Label(f, text="", foreground="#0a58ca", justify="left")
        self.ah_repeat_lbl.pack(anchor="w")

    def _build_step4(self, f):
        ttk.Label(f, text="Threshold (peak). Nudge if homing is flickery:").pack(anchor="w")
        trow = ttk.Frame(f); trow.pack(fill="x", pady=2)
        ttk.Button(trow, text="-", width=3, command=lambda: self._nudge_threshold(-2)).pack(side="left")
        self.test_thr_lbl = ttk.Label(trow, text="--", width=8, anchor="center")
        self.test_thr_lbl.pack(side="left", padx=4)
        ttk.Button(trow, text="+", width=3, command=lambda: self._nudge_threshold(+2)).pack(side="left")

        ttk.Separator(f, orient="horizontal").pack(fill="x", pady=8)
        ttk.Button(f, text="Run homing", command=self._run_home).pack(fill="x")
        self.home_lbl = ttk.Label(f, text="not homed", justify="left")
        self.home_lbl.pack(anchor="w", pady=4)
        rr = ttk.Frame(f); rr.pack(fill="x")
        ttk.Label(rr, text="repeat x").pack(side="left")
        self.repeat_var = tk.StringVar(value="5")
        ttk.Entry(rr, textvariable=self.repeat_var, width=4).pack(side="left")
        ttk.Button(rr, text="Repeatability", command=self._run_repeat).pack(side="left", padx=4)
        self.repeat_lbl = ttk.Label(f, text="", foreground="#0a58ca", justify="left")
        self.repeat_lbl.pack(anchor="w")

        ttk.Separator(f, orient="horizontal").pack(fill="x", pady=8)
        ttk.Label(f, text="Jog to any angle in the cycle:").pack(anchor="w")
        gr = ttk.Frame(f); gr.pack(fill="x", pady=2)
        self.goto_deg = tk.StringVar(value="0")
        ttk.Entry(gr, textvariable=self.goto_deg, width=8).pack(side="left")
        ttk.Label(gr, text="deg").pack(side="left", padx=(2, 6))
        ttk.Button(gr, text="Go", command=self._goto_degrees).pack(side="left")
        ttk.Button(f, text="Restart wizard", command=lambda: self._wiz_goto(0)).pack(fill="x", pady=(12, 0))

    # --- universal motion panel (visible on every wizard step) -----------
    def _build_wiz_motion(self, parent):
        strip = ttk.LabelFrame(parent, text="Motion (all steps)", padding=(8, 4))
        strip.pack(side="bottom", fill="x")

        jf = ttk.Frame(strip); jf.pack(side="left", fill="x", expand=True)
        ttk.Label(jf, text="jog").pack(side="left", padx=(0, 4))
        self.jog_dial_var = tk.DoubleVar(value=0)              # owned here now
        self.jog_dial = ttk.Scale(jf, from_=-1000, to=1000, orient="horizontal",
                                  variable=self.jog_dial_var,
                                  command=self._jog_dial_move, length=240)
        self.jog_dial.pack(side="left", fill="x", expand=True)
        self.jog_dial.bind("<ButtonRelease-1>", self._jog_dial_release)  # spring back + C 0
        self.jog_speed_lbl = ttk.Label(jf, text="0 usteps/s", width=13)
        self.jog_speed_lbl.pack(side="left", padx=6)

        gf = ttk.Frame(strip); gf.pack(side="left", padx=10)
        ttk.Label(gf, text="go to").pack(side="left")
        self.uni_goto_var = tk.StringVar(value="0")
        ttk.Entry(gf, textvariable=self.uni_goto_var, width=8).pack(side="left", padx=3)
        self.uni_goto_unit = tk.StringVar(value="deg")
        ttk.Combobox(gf, textvariable=self.uni_goto_unit, values=["deg", "usteps"],
                     width=7, state="readonly").pack(side="left")
        ttk.Button(gf, text="Go", command=self._uni_goto).pack(side="left", padx=3)

        ttk.Button(strip, text="STOP", command=self._stop_all).pack(side="right", padx=6)

    def _uni_goto(self):
        try:
            val = float(self.uni_goto_var.get())
        except ValueError:
            return
        if self.uni_goto_unit.get() == "deg":
            usteps = int(round(val / 360.0 * self.usteps_per_rev))
        else:
            usteps = int(round(val))
        self.client.send(f"G {usteps} {JOG_MAX_SPEED}")

    # --- advanced tab (v1 controls retained) -----------------------------
    def _build_advanced(self, parent):
        left = ttk.Frame(parent, padding=6, width=320)
        left.pack(side="left", fill="y")
        left.pack_propagate(False)
        right = ttk.Frame(parent, padding=6)
        right.pack(side="left", fill="both", expand=True)

        # threshold
        tf = ttk.LabelFrame(left, text="Threshold", padding=6); tf.pack(fill="x", pady=3)
        self.thr_var = tk.IntVar(value=220)
        sc = ttk.Scale(tf, from_=0, to=255, orient="horizontal",
                       variable=self.thr_var, command=self._on_thr_slide)
        sc.grid(row=0, column=0, columnspan=3, sticky="ew")
        sc.bind("<ButtonRelease-1>", lambda e: self._send_threshold())
        self.thr_lbl = ttk.Label(tf, text="220 (2.85V)")
        self.thr_lbl.grid(row=1, column=0, sticky="w")
        ttk.Button(tf, text="Set", command=self._send_threshold).grid(row=1, column=2, sticky="e")
        self.sweep_btn = ttk.Button(tf, text="Start sweep", command=self._toggle_sweep)
        self.sweep_btn.grid(row=2, column=0, columnspan=3, sticky="ew", pady=(6, 0))
        ttk.Button(tf, text="Capture HOME", command=lambda: self._capture(True)).grid(row=3, column=0, sticky="ew", pady=2)
        ttk.Button(tf, text="Capture NOT", command=lambda: self._capture(False)).grid(row=3, column=1, sticky="ew", pady=2)
        ttk.Button(tf, text="Apply", command=self._apply_recommended).grid(row=3, column=2, sticky="ew")
        self.recommend_lbl = ttk.Label(tf, text="home - / not - / thr -")
        self.recommend_lbl.grid(row=4, column=0, columnspan=3, sticky="w")

        # motor
        mf = ttk.LabelFrame(left, text="Motor (Axis A)", padding=6); mf.pack(fill="x", pady=3)
        self.enable_var = tk.IntVar(value=0)
        ttk.Checkbutton(mf, text="Enable", variable=self.enable_var,
                        command=self._on_enable).grid(row=0, column=0, columnspan=3, sticky="w")
        ttk.Label(mf, text="step").grid(row=1, column=0, sticky="w")
        self.jog_step = tk.StringVar(value="2000")
        ttk.Entry(mf, textvariable=self.jog_step, width=8).grid(row=1, column=1, sticky="w")
        ttk.Label(mf, text="speed").grid(row=2, column=0, sticky="w")
        self.speed_var = tk.StringVar(value="14080")
        ttk.Entry(mf, textvariable=self.speed_var, width=8).grid(row=2, column=1, sticky="w")
        ttk.Button(mf, text="< Jog -", command=lambda: self._jog(-1)).grid(row=3, column=0, sticky="ew", pady=2)
        ttk.Button(mf, text="Jog + >", command=lambda: self._jog(+1)).grid(row=3, column=1, sticky="ew", pady=2)
        ttk.Button(mf, text="STOP", command=self._stop_all).grid(row=3, column=2, sticky="ew", pady=2)
        ttk.Label(mf, text="go to").grid(row=4, column=0, sticky="w")
        self.goto_var = tk.StringVar(value="0")
        ttk.Entry(mf, textvariable=self.goto_var, width=8).grid(row=4, column=1, sticky="w")
        ttk.Button(mf, text="Go", command=self._goto).grid(row=4, column=2, sticky="ew")
        ttk.Button(mf, text="Zero here", command=lambda: self.client.send("Z")).grid(row=5, column=0, columnspan=2, sticky="ew", pady=2)

        # homing
        hf = ttk.LabelFrame(left, text="Homing", padding=6); hf.pack(fill="x", pady=3)
        ttk.Button(hf, text="HOME", command=self._run_home).grid(row=0, column=0, sticky="ew")
        ttk.Label(hf, text="x").grid(row=0, column=1, sticky="e")
        self.repeat_var2 = tk.StringVar(value="5")
        ttk.Entry(hf, textvariable=self.repeat_var2, width=4).grid(row=0, column=2, sticky="w")
        ttk.Button(hf, text="Repeatability", command=self._run_repeat2).grid(row=1, column=0, columnspan=3, sticky="ew", pady=2)

        # plots notebook
        pnb = ttk.Notebook(right); pnb.pack(fill="both", expand=True)
        live = ttk.Frame(pnb); pnb.add(live, text="Live")
        self.adv_live_fig = Figure(figsize=(5, 3.6), tight_layout=True)
        self._setup_live_axes(self.adv_live_fig)
        self.adv_live_canvas = FigureCanvasTkAgg(self.adv_live_fig, master=live)
        self.adv_live_canvas.get_tk_widget().pack(fill="both", expand=True)

        swp = ttk.Frame(pnb); pnb.add(swp, text="Sweep")
        self.adv_sweep_fig = Figure(figsize=(5, 3.6), tight_layout=True)
        self.adv_sweep_ax = self.adv_sweep_fig.add_subplot(111)
        self.adv_sweep_ax.set_xlabel("time (s)"); self.adv_sweep_ax.set_ylabel("crossing duty")
        self.adv_sweep_ax.set_ylim(-5, 260); self.adv_sweep_ax.grid(True, alpha=0.3)
        (self.adv_sweep_line,) = self.adv_sweep_ax.plot([], [], "o-", ms=3, color="tab:green")
        self.adv_sweep_canvas = FigureCanvasTkAgg(self.adv_sweep_fig, master=swp)
        self.adv_sweep_canvas.get_tk_widget().pack(fill="both", expand=True)

    def _setup_live_axes(self, fig):
        ax = fig.add_subplot(111)
        ax.set_xlabel("time (s)"); ax.set_ylabel("position (deg)", color="tab:blue")
        ax.grid(True, alpha=0.3)
        axl = ax.twinx()
        axl.set_ylabel("sensor", color="tab:orange")
        axl.set_ylim(-0.1, 1.1); axl.set_yticks([0, 1]); axl.set_yticklabels(["off", "FLAG"])
        (ld,) = ax.plot([], [], "-", color="tab:blue", lw=1.2)
        (ll,) = axl.plot([], [], "-", color="tab:orange", lw=1.0, drawstyle="steps-post")
        return ax, axl, ld, ll

    # ====================================================================
    # Wizard navigation
    # ====================================================================
    def _wiz_goto(self, step):
        step = max(0, min(3, step))
        self.wiz_step = step
        for i, fr in enumerate(self.step_frames):
            if i == step:
                fr.pack(fill="both", expand=True)
            else:
                fr.pack_forget()
        self.wiz_title.config(text=f"Step {step + 1}: {WIZARD_TITLES[step]}")
        self.wiz_progress.config(text=f"Step {step + 1} of 4")
        self.wiz_back.state(["!disabled"] if step > 0 else ["disabled"])
        self.wiz_next.state(["!disabled"] if step < 3 else ["disabled"])
        self._setup_wizard_plot(step)

    def _wiz_next(self):
        self._wiz_goto(self.wiz_step + 1)

    def _wiz_back(self):
        self._wiz_goto(self.wiz_step - 1)

    def _setup_wizard_plot(self, step):
        self.wiz_fig.clear()
        if step in (0, 3):
            self.wiz_ax, self.wiz_axl, self.wiz_ld, self.wiz_ll = \
                self._setup_live_axes(self.wiz_fig)
            self.wiz_edges = []
        elif step == 1:
            lo = self.scan_dmin - 4
            ax = self.wiz_fig.add_subplot(111)
            ax.set_xlabel("offset from centre (usteps)")
            ax.set_ylabel("crossing duty (signal)", color="tab:purple")
            ax.set_ylim(lo, 258); ax.grid(True, alpha=0.3)
            ax.axhline(255, color="0.7", ls=":", lw=0.8)          # saturation rail
            (self.scan_line,) = ax.plot([], [], "-", color="tab:purple", lw=1.3)
            (self.scan_pts,) = ax.plot([], [], "o", ms=3, color="tab:purple")
            (self.scan_sat,) = ax.plot([], [], "o", ms=6, color="tab:red")    # saturated
            (self.scan_gap,) = ax.plot([], [], "x", ms=6, color="tab:gray")   # no signal
            self.scan_center_line = ax.axvline(0, color="green", lw=1.0, alpha=0.6)
            axv = ax.twinx()                                       # volts, locked to duty
            axv.set_ylabel("volts", color="tab:gray")
            axv.set_ylim(duty_to_volts(lo), duty_to_volts(258))
            axdeg = ax.secondary_xaxis(
                "top", functions=(lambda x: 360.0 * x / self.usteps_per_rev,
                                  lambda d: d * self.usteps_per_rev / 360.0))
            axdeg.set_xlabel("offset (deg)")
            self.scan_ax = ax
            if self.scan_off:                                     # re-render on revisit
                self._render_scan()
        elif step == 2:
            ax = self.wiz_fig.add_subplot(111)
            ax.set_xlabel("position (usteps)")
            ax.set_ylabel("crossing duty (signal)", color="tab:purple")
            ax.grid(True, alpha=0.3)
            (self.ah_line,) = ax.plot([], [], "o-", ms=3, color="tab:purple")   # char curve
            self.ah_thr_line = None                                             # created lazily
            self.ah_min_line = ax.axvline(0, color="tab:orange", lw=1.0, alpha=0.0)  # dip centre
            self.ah_lead_line = ax.axvline(0, color="red", ls=":", lw=0.9, alpha=0.0)
            self.ah_trail_line = ax.axvline(0, color="red", ls=":", lw=0.9, alpha=0.0)
            self.ah_home_line = ax.axvline(0, color="green", lw=1.4, alpha=0.0)
            axdeg = ax.secondary_xaxis(
                "top", functions=(lambda x: 360.0 * x / self.usteps_per_rev,
                                  lambda dg: dg * self.usteps_per_rev / 360.0))
            axdeg.set_xlabel("position (deg)")
            self.wiz_ax = ax
            if self.ah_char_pos or self.ah_result:    # re-render last run on revisit
                self._render_autohome()
        self.wiz_canvas.draw_idle()

    # ====================================================================
    # Connection
    # ====================================================================
    def _toggle_connect(self):
        if self.client.connected:
            self.client.close()
            self.connect_btn.config(text="Connect")
            self.scan_active = False
            self.autohome_active = False
            self.repeat_remaining = 0
            return
        port = self.port_var.get() or autodetect_port()
        if not port:
            self.connect_btn.config(text="no port")
            return
        try:
            self.client.open(port, self.baud)
        except Exception as e:
            self.connect_btn.config(text="error")
            print("open error:", e)
            return
        self.port_var.set(port)
        self.connect_btn.config(text="Disconnect")
        self.t0 = time.monotonic()
        self.live_t.clear(); self.live_deg.clear(); self.live_level.clear()
        self.sweep_t.clear(); self.sweep_duty.clear()
        self.scan_active = False
        self.autohome_active = False
        self.repeat_remaining = 0
        self.client.send("M 1")
        self._send_threshold()

    # ====================================================================
    # Step actions
    # ====================================================================
    # -- step 1: jog dial (exponential taper, spring back to centre)
    def _dial_to_speed(self, pos):
        # pos in [-1000, 1000] -> signed speed. Exponential taper gives fine
        # control near the centre and ramps up toward the ends; small dead zone.
        x = max(-1.0, min(1.0, pos / 1000.0))
        if abs(x) < 0.03:
            return 0
        k = 5.0
        frac = (math.exp(k * abs(x)) - 1.0) / (math.exp(k) - 1.0)
        return int((1 if x > 0 else -1) * JOG_MAX_SPEED * frac)

    def _jog_dial_move(self, _v):
        speed = self._dial_to_speed(float(self.jog_dial_var.get()))
        self.jog_speed_lbl.config(text=f"{speed} usteps/s")
        now = time.monotonic()
        if speed != self._last_jog_v and (now - self._last_jog_t) >= 0.04:
            self.client.send(f"C {speed}")
            self._last_jog_v = speed
            self._last_jog_t = now

    def _jog_dial_release(self, _e=None):
        self.jog_dial_var.set(0)          # spring the thumb back to centre
        self._last_jog_v = 0
        self._last_jog_t = 0.0
        self.jog_speed_lbl.config(text="0 usteps/s")
        self.client.send("C 0")           # ...and stop the motor

    def _stop_all(self):
        self.client.send("X")

    # -- step 2 (legacy): single sweep here (orphaned; kept harmless)
    def _sweep_here(self):
        self.awaiting_sweep_here = True
        self.sweep_here_lbl.config(text="sweeping...")
        self.client.send("Q")

    # -- step 2: home-shape scan
    def _run_scan(self):
        try:
            n = int(self.scan_n_var.get())
        except ValueError:
            n = SCAN_DEFAULT_RANGE
        try:
            step = int(self.scan_step_var.get())
        except ValueError:
            step = SCAN_DEFAULT_STEP
        if n <= 0 or step <= 0:
            self.scan_lbl.config(text="range and step must be positive")
            return
        try:
            dmin = int(self.scan_dmin_var.get()); dmax = int(self.scan_dmax_var.get())
        except ValueError:
            dmin, dmax = 200, 255
        dmin = max(0, min(254, dmin)); dmax = max(dmin + 1, min(255, dmax))
        self.scan_dmin, self.scan_dmax = dmin, dmax
        self.scan_pos, self.scan_off = [], []
        self.scan_cross, self.scan_lo, self.scan_hi = [], [], []
        self.scan_center = None
        self.scan_n, self.scan_step = n, step
        self.scan_expected = 2 * (n // step) + 1        # provisional; K,begin overwrites
        self.scan_active, self.scan_done = True, False
        self.scan_save_btn.state(["disabled"])
        self.scan_lbl.config(text="scanning...")
        if self.wiz_step == 1:
            self._setup_wizard_plot(1)                  # clear the plot (reads scan_dmin)
        self._scan_status()
        self.client.send(f"K {n} {step} {dmin} {dmax}")

    def _stop_scan(self):
        self.client.send("X")                           # same abort path as search
        self.scan_active = False
        if self.scan_pos:
            self.scan_done = True
            self.scan_save_btn.state(["!disabled"])     # allow saving the partial scan
        self.scan_lbl.config(
            text=f"stopped at {len(self.scan_pos)} / {self.scan_expected}")

    def _scan_status(self):
        self.scan_progress_lbl.config(
            text=f"{len(self.scan_pos)} / {self.scan_expected}")

    def _scan_finish(self):
        n = len(self.scan_pos)
        if n == 0:
            self.scan_lbl.config(text="scan produced no samples")
            return
        self.scan_done = True
        self.scan_save_btn.state(["!disabled"])
        valid = [c for c in self.scan_cross if c >= 0]
        if valid:
            best = max(valid)
            bi = self.scan_cross.index(best)
            deg = 360.0 * self.scan_off[bi] / self.usteps_per_rev
            sat = " (saturated/clipped)" if best >= SCAN_SAT_DUTY else ""
            self.scan_lbl.config(
                text=f"done: {n} pts, peak {best}{sat}\n"
                     f"at {self.scan_off[bi]:+d} usteps ({deg:+.2f} deg)")
        else:
            self.scan_lbl.config(
                text=f"done: {n} pts, no crossing anywhere (off the flag?)")
        self._render_scan()

    def _save_scan(self):
        if not self.scan_pos:
            messagebox.showinfo("Save scan", "No scan data yet - run a scan first.")
            return
        stamp = time.strftime("%Y%m%d_%H%M%S")
        path = filedialog.asksaveasfilename(
            title="Save home-shape scan", defaultextension=".csv",
            initialfile=f"home_scan_{stamp}.csv",
            filetypes=[("CSV data", "*.csv"), ("All files", "*.*")])
        if not path:
            return
        base = path[:-4] if path.lower().endswith(".csv") else path
        try:
            self._write_scan_csv(base + ".csv")
            self._write_scan_json(base + ".json")
        except OSError as e:
            messagebox.showerror("Save scan", f"Could not write scan files:\n{e}")
            return
        try:
            self.wiz_fig.savefig(base + ".png", dpi=120)
        except Exception as e:
            print("png save error:", e)
        self.scan_lbl.config(
            text=f"saved {len(self.scan_pos)} pts ->\n{base}.csv/.png/.json")

    def _scan_meta(self):
        return dict(
            timestamp=time.strftime("%Y-%m-%d %H:%M:%S"),
            usteps_per_rev=self.usteps_per_rev,
            scan_center_pos=self.scan_center,
            range_usteps=self.scan_n,
            step_usteps=self.scan_step,
            samples=len(self.scan_pos),
            expected=self.scan_expected,
            vref=VREF)

    def _write_scan_csv(self, path):
        upr = self.usteps_per_rev
        m = self._scan_meta()
        with open(path, "w", newline="") as fh:
            fh.write(f"# home-shape scan  {m['timestamp']}\n")
            fh.write(f"# usteps_per_rev={upr}\n")
            fh.write(f"# scan_center_pos={m['scan_center_pos'] if m['scan_center_pos'] is not None else ''}\n")
            fh.write(f"# range_usteps=+/-{m['range_usteps']}  step_usteps={m['step_usteps']}\n")
            fh.write(f"# samples={m['samples']}  expected={m['expected']}\n")
            fh.write(f"# vref={m['vref']}\n")
            fh.write("# crossing_duty=-1 means no comparator crossing at that position\n")
            w = csv.writer(fh)
            w.writerow(["index", "pos_usteps", "offset_usteps", "offset_deg",
                        "crossing_duty", "crossing_volts", "lo_rail", "hi_rail"])
            for i in range(len(self.scan_pos)):
                cross = self.scan_cross[i]
                off = self.scan_off[i]
                deg = 360.0 * off / upr if upr else float("nan")
                volts = f"{duty_to_volts(cross):.4f}" if cross >= 0 else ""
                w.writerow([i, self.scan_pos[i], off, f"{deg:.4f}",
                            cross, volts, self.scan_lo[i], self.scan_hi[i]])

    def _write_scan_json(self, path):
        upr = self.usteps_per_rev
        data = dict(self._scan_meta())
        data["samples_data"] = [
            dict(index=i, pos_usteps=self.scan_pos[i], offset_usteps=self.scan_off[i],
                 offset_deg=360.0 * self.scan_off[i] / upr if upr else None,
                 crossing_duty=self.scan_cross[i],
                 crossing_volts=(duty_to_volts(self.scan_cross[i])
                                 if self.scan_cross[i] >= 0 else None),
                 lo_rail=self.scan_lo[i], hi_rail=self.scan_hi[i])
            for i in range(len(self.scan_pos))]
        with open(path, "w") as fh:
            json.dump(data, fh, indent=2)

    # -- step 3: auto-home (self-calibrating dip home)
    def _ah_reset(self):
        self.ah_char_pos, self.ah_char_cross = [], []
        self.ah_edges = []
        self.ah_thr = self.ah_cmin = self.ah_pmin = self.ah_dipdepth = None
        self.ah_home = None

    def _run_autohome(self):
        self.repeat_remaining = 0
        self._ah_reset(); self.ah_result = None
        self.autohome_active, self.autohome_done = True, False
        self.ah_repeat_btn.state(["disabled"])
        self.ah_lbl.config(text="calibrating & homing...")
        if self.wiz_step == 2:
            self._setup_wizard_plot(2)                  # clear the plot
        self.client.send("A")

    def _stop_autohome(self):
        self.client.send("X")                           # same abort path as scan/search
        self.autohome_active = False
        self.repeat_remaining = 0                        # cancel any pending repeat chain
        self.ah_lbl.config(text="aborted")

    def _run_autohome_repeat(self):
        if not self.autohome_done:
            self.ah_repeat_lbl.config(text="calibrate & home first")
            return
        self._start_repeat(self.ah_repeat_var, "H", self.ah_lbl, self.ah_repeat_lbl)

    def _finish_autohome(self, ok, home, lead, trail, sw, thr, msg):
        d = lambda s: 360.0 * s / self.usteps_per_rev
        if ok:
            dtxt = f", dip {self.ah_dipdepth}" if self.ah_dipdepth is not None else ""
            self.ah_lbl.config(
                text=f"homed ok - home {home} ({d(home):.2f} deg)\n"
                     f"thr {thr}{dtxt}, switch {d(sw):.2f} deg\n"
                     f"edges {d(lead - home):+.2f} / {d(trail - home):+.2f} deg")
            self.ah_repeat_btn.state(["!disabled"])
        else:
            self.ah_lbl.config(text=f"auto-home FAILED: {msg or 'no dip / edge found'}")
        self._render_autohome()

    # -- step 3 (legacy): peak search (orphaned; kept harmless)
    def _run_search(self):
        try:
            n = int(self.search_n.get())
        except ValueError:
            n = 500
        self.search_pos, self.search_cross = [], []
        self.search_best = None
        self.search_active = True
        self.search_lbl.config(text="searching...")
        if self.wiz_step == 2:
            self._setup_wizard_plot(2)     # clear the plot
        self.client.send(f"F {n}")

    # -- step 4: test
    def _nudge_threshold(self, delta):
        v = max(0, min(255, int(self.status.get("thr", 0)) + delta))
        self.client.send(f"T {v}")
        self.test_thr_lbl.config(text=str(v))

    def _run_home(self):
        self.repeat_remaining = 0
        self.home_lbl.config(text="homing...")
        self.client.send("H")

    def _run_repeat(self):
        self._start_repeat(self.repeat_var, "H", self.home_lbl, self.repeat_lbl)

    def _run_repeat2(self):
        self._start_repeat(self.repeat_var2, "H", self.home_lbl, self.repeat_lbl)

    def _start_repeat(self, var, cmd="H", status_lbl=None, repeat_lbl=None):
        try:
            n = int(var.get())
        except ValueError:
            return
        if n < 1:
            return
        self.repeat_cmd = cmd
        self.repeat_status_lbl = status_lbl or self.home_lbl
        self.repeat_result_lbl = repeat_lbl or self.repeat_lbl
        self.repeat_remaining = n
        self.repeat_homes = []
        self.repeat_result_lbl.config(text=f"repeatability: 0/{n}")
        self.repeat_status_lbl.config(text="homing...")
        self.client.send(cmd)

    def _repeat_step(self, ok, home):
        # Chain the next repeat (or finish) after each home result. Called from
        # _handle_home; a no-op when no repeat run is active (repeat_remaining==0).
        if self.repeat_remaining <= 0:
            return
        if ok:
            self.repeat_homes.append(home)
        self.repeat_remaining -= 1
        if self.repeat_remaining > 0:
            self.repeat_result_lbl.config(
                text=f"repeatability: {len(self.repeat_homes)} ok, {self.repeat_remaining} left...")
            self.root.after(300, lambda: self.client.send(self.repeat_cmd))
        else:
            self._finish_repeat()

    def _goto_degrees(self):
        try:
            deg = float(self.goto_deg.get())
        except ValueError:
            return
        usteps = int(round(deg / 360.0 * self.usteps_per_rev))
        self.client.send(f"G {usteps} 14080")

    # ====================================================================
    # Advanced-tab actions (v1)
    # ====================================================================
    def _on_thr_slide(self, _v):
        v = int(float(self.thr_var.get()))
        self.thr_lbl.config(text=f"{v} ({duty_to_volts(v):.2f}V)")

    def _send_threshold(self):
        self.client.send(f"T {int(float(self.thr_var.get()))}")

    def _toggle_sweep(self):
        if self.sweeping:
            self.sweeping = False
            self.sweep_btn.config(text="Start sweep")
            self.client.send("M 1"); self._send_threshold()
        else:
            self.sweeping = True
            self.sweep_btn.config(text="Stop sweep")
            self.sweep_t.clear(); self.sweep_duty.clear(); self.recent_duty.clear()
            self.client.send("M 2")

    def _capture(self, is_home):
        if not self.recent_duty:
            return
        mean = sum(self.recent_duty) / len(self.recent_duty)
        if is_home:
            self.capture_home = mean
        else:
            self.capture_nothome = mean
        if self.capture_home is not None and self.capture_nothome is not None:
            self.recommended = int(round((self.capture_home + self.capture_nothome) / 2))
        h = "-" if self.capture_home is None else f"{self.capture_home:.0f}"
        n = "-" if self.capture_nothome is None else f"{self.capture_nothome:.0f}"
        r = "-" if self.recommended is None else f"{self.recommended}"
        self.recommend_lbl.config(text=f"home {h} / not {n} / thr {r}")

    def _apply_recommended(self):
        if self.recommended is None:
            return
        self.thr_var.set(self.recommended); self._on_thr_slide(None)
        if self.sweeping:
            self._toggle_sweep()
        self.client.send(f"T {self.recommended}")

    def _speed(self):
        try:
            return int(float(self.speed_var.get()))
        except ValueError:
            return 14080

    def _on_enable(self):
        self.client.send(f"E {self.enable_var.get()}")

    def _jog(self, sign):
        try:
            step = int(float(self.jog_step.get()))
        except ValueError:
            return
        self.client.send(f"J {sign * step} {self._speed()}")

    def _goto(self):
        try:
            target = int(float(self.goto_var.get()))
        except ValueError:
            return
        self.client.send(f"G {target} {self._speed()}")

    # ====================================================================
    # Incoming lines
    # ====================================================================
    def _drain(self):
        while True:
            try:
                line = self.client.rx.get_nowait()
            except queue.Empty:
                break
            try:
                self._handle(line)
            except Exception:
                pass
        now = time.monotonic()
        if now - self._last_redraw > 0.1:
            self._last_redraw = now
            self._redraw()
        self.root.after(50, self._drain)

    def _handle(self, line):
        if not line:
            return
        if line.startswith("#"):
            for tok in line.split():
                if tok.startswith("usteps_per_rev="):
                    try:
                        self.usteps_per_rev = int(tok.split("=", 1)[1])
                    except ValueError:
                        pass
                elif tok.startswith("default_threshold="):
                    try:
                        self.thr_var.set(int(tok.split("=", 1)[1])); self._on_thr_slide(None)
                    except ValueError:
                        pass
            return
        tag = line[0]
        p = line.split(",")
        if tag == "S":
            self._handle_status(p)
        elif tag == "D":
            self._handle_sweep(p)
        elif tag == "Q":
            self._handle_q(p)
        elif tag == "B":
            self._handle_b(p)
        elif tag == "H":
            self._handle_home(line, p)
        elif tag == "K":
            self._handle_k(p)
        elif tag == "A":
            self._handle_a(p, line)

    def _handle_status(self, p):
        if len(p) < 10:
            return
        try:
            self.status = dict(
                level=int(p[2]), thr=int(p[3]), pos=int(p[4]), deg=int(p[5]) / 10.0,
                running=int(p[6]), enabled=int(p[7]), fault=int(p[8]), homed=int(p[9]))
        except ValueError:
            return
        t = time.monotonic() - self.t0
        self.live_t.append(t); self.live_deg.append(self.status["deg"])
        self.live_level.append(self.status["level"])
        cutoff = t - LIVE_WINDOW_S
        while self.live_t and self.live_t[0] < cutoff:
            self.live_t.popleft(); self.live_deg.popleft(); self.live_level.popleft()
        self._update_statusbar()

    def _handle_sweep(self, p):
        # D fields: 0=D 1=ms 2=A_duty 3=B 4=A_sig 5=B 6=A_lo 7=A_hi ...
        try:
            duty = int(p[2])
            a_hi = int(p[7]) if len(p) > 7 else 0
        except ValueError:
            return
        t = time.monotonic() - self.t0
        val = duty if duty >= 0 else (255 if a_hi else 0)
        self.sweep_t.append(t); self.sweep_duty.append(val)
        if duty >= 0:
            self.recent_duty.append(duty)
        cutoff = t - SWEEP_WINDOW_S
        while self.sweep_t and self.sweep_t[0] < cutoff:
            self.sweep_t.popleft(); self.sweep_duty.popleft()

    def _handle_q(self, p):
        try:
            pos = int(p[1]); cross = int(p[2])
        except (ValueError, IndexError):
            return
        lo = int(p[3]) if len(p) > 3 and p[3].lstrip("-").isdigit() else 0
        hi = int(p[4]) if len(p) > 4 and p[4].lstrip("-").isdigit() else 0
        if self.search_active:
            self.search_pos.append(pos)
            self.search_cross.append(cross)
            best = max((c for c in self.search_cross if c >= 0), default=-1)
            self.search_lbl.config(text=f"searching... {len(self.search_pos)} pts, best {best}")
        elif self.awaiting_sweep_here:
            self.awaiting_sweep_here = False
            if cross >= 0:
                self.sweep_here_lbl.config(text=f"crossing: {cross}  ->  stored as threshold")
                self.client.send(f"T {cross}")
            elif hi:
                self.sweep_here_lbl.config(
                    text="signal ABOVE range (comparator stuck HIGH):\n"
                         "very strong reflection here - can't set a\n"
                         "crossing threshold. Reduce sensor current, or\n"
                         "the search will still find the peak.")
            else:
                self.sweep_here_lbl.config(
                    text="no signal (comparator stuck LOW):\n"
                         "off the flag? jog back onto the home first.")
            self._draw_level_bar(cross)

    def _handle_b(self, p):
        # B,<bestPos>,<bestCross>,<threshold>
        self.search_active = False
        try:
            best_pos = int(p[1]); best_cross = int(p[2]); thr = int(p[3])
        except (ValueError, IndexError):
            return
        self.search_best = (best_pos, best_cross)
        deg = 360.0 * best_pos / self.usteps_per_rev
        self.search_lbl.config(
            text=f"best pos {best_pos} ({deg:.2f} deg)\ncrossing {best_cross}  ->  threshold {thr}")

    def _handle_k(self, p):
        # K,begin,<center>,<n>,<step>,<count> | K,<i>,<pos>,<cross>,<lo>,<hi> | K,end,...
        if len(p) < 2:
            return
        kind = p[1]
        if kind == "begin":
            try:
                self.scan_center = int(p[2]); self.scan_n = int(p[3])
                self.scan_step = int(p[4]); self.scan_expected = int(p[5])
            except (ValueError, IndexError):
                return
            if len(p) > 7 and p[6].lstrip("-").isdigit() and p[7].lstrip("-").isdigit():
                self.scan_dmin, self.scan_dmax = int(p[6]), int(p[7])
            self.scan_pos, self.scan_off = [], []
            self.scan_cross, self.scan_lo, self.scan_hi = [], [], []
            self.scan_active, self.scan_done = True, False
            if self.wiz_step == 1:
                self._setup_wizard_plot(1)          # fresh axes/artists
            self._scan_status()
            return
        if kind == "end":
            self.scan_active = False
            self._scan_finish()
            return
        try:                                        # K,index,pos,cross,lo,hi
            pos = int(p[2]); cross = int(p[3])
        except (ValueError, IndexError):
            return
        lo = int(p[4]) if len(p) > 4 and p[4].lstrip("-").isdigit() else 0
        hi = int(p[5]) if len(p) > 5 and p[5].lstrip("-").isdigit() else 0
        center = self.scan_center if self.scan_center is not None else pos
        self.scan_pos.append(pos); self.scan_off.append(pos - center)
        self.scan_cross.append(cross); self.scan_lo.append(lo); self.scan_hi.append(hi)
        self._scan_status()

    def _handle_a(self, p, line):
        # A,begin | A,pt,<i>,<pos>,<cross>,<lo>,<hi> | A,thr,<T>,<cmin>,<pmin>,<depth>
        # | A,edge,<which>,<pos> | A,done,<ok>,<home>,<lead>,<trail>,<switch>,<T>,<depth>,"<msg>"
        if len(p) < 2:
            return
        kind = p[1]
        if kind == "begin":
            self._ah_reset()
            self.autohome_active, self.autohome_done = True, False
            if self.wiz_step == 2:
                self._setup_wizard_plot(2)          # fresh axes/artists
            self.ah_lbl.config(text="characterising the dip...")
            return
        if kind == "pt":
            try:
                pos = int(p[3]); cross = int(p[4])
            except (ValueError, IndexError):
                return
            self.ah_char_pos.append(pos)
            self.ah_char_cross.append(cross if cross >= 0 else float("nan"))
            self.ah_lbl.config(text=f"characterising... {len(self.ah_char_pos)} pts")
            return
        if kind == "thr":
            try:
                self.ah_thr = int(p[2]); self.ah_cmin = int(p[3]); self.ah_pmin = int(p[4])
                self.ah_dipdepth = int(p[5]) if len(p) > 5 and p[5].lstrip("-").isdigit() else None
            except (ValueError, IndexError):
                return
            self.ah_lbl.config(text=f"threshold {self.ah_thr} -> two-edge homing...")
            return
        if kind == "edge":
            try:
                self.ah_edges.append((p[2], int(p[3])))
            except (ValueError, IndexError):
                return
            return
        if kind == "done":
            self.autohome_active = False
            try:
                ok = int(p[2]); home = int(p[3]); lead = int(p[4])
                trail = int(p[5]); sw = int(p[6]); thr = int(p[7])
            except (ValueError, IndexError):
                ok, home, lead, trail, sw = 0, 0, 0, 0, 0
                thr = self.ah_thr or 0
            if len(p) > 8 and p[8].lstrip("-").isdigit():
                self.ah_dipdepth = int(p[8])
            msg = line.split('"')[1] if '"' in line else ""
            self.ah_home = home if ok else None
            self.ah_thr = thr
            self.autohome_done = ok == 1
            self.ah_result = dict(ok=ok, home=home, lead=lead, trail=trail,
                                  switch=sw, thr=thr, msg=msg)
            self._finish_autohome(ok, home, lead, trail, sw, thr, msg)
            return

    def _handle_home(self, line, p):
        try:
            ok = int(p[1]); home = int(p[2]); sw = int(p[3])
            lead = int(p[4]); trail = int(p[5])
        except (ValueError, IndexError):
            return
        msg = line.split('"')[1] if '"' in line else ""
        d = lambda s: 360.0 * s / self.usteps_per_rev
        self.last_home = dict(ok=ok, home=home, switchSize=sw, lead=lead, trail=trail, msg=msg)
        if ok:
            txt = (f"homed ok - switch {d(sw):.2f} deg\n"
                   f"edges {d(lead - home):+.2f} / {d(trail - home):+.2f} deg")
            if hasattr(self, "wiz_step") and self.wiz_step == 3:
                self._draw_home_edges(d(lead - home), d(trail - home))
        else:
            txt = f"home FAILED: {msg}"
        self.home_lbl.config(text=txt)
        self._repeat_step(ok, home)

    def _finish_repeat(self):
        lbl = getattr(self, "repeat_result_lbl", self.repeat_lbl)
        if not self.repeat_homes:
            lbl.config(text="repeatability: no successful homes")
            return
        homes = self.repeat_homes
        spread = max(homes) - min(homes)
        deg = 360.0 * spread / self.usteps_per_rev
        lbl.config(
            text=f"repeatability n={len(homes)}: spread {spread} usteps ({deg:.3f} deg)")

    # ====================================================================
    # Rendering
    # ====================================================================
    def _update_statusbar(self):
        s = self.status
        if s["level"]:
            self.flag_lbl.config(text="FLAG PRESENT", background="#2e7d32", foreground="white")
        else:
            self.flag_lbl.config(text="absent", background="#cccccc", foreground="black")
        self.pos_lbl.config(text=f"pos {s['pos']}  ({s['deg']:.2f} deg)")
        self.thr_stat_lbl.config(text=f"thr {s['thr']}")
        st = []
        st.append("RUN" if s["running"] else "idle")
        if s["enabled"]:
            st.append("EN")
        if s["homed"]:
            st.append("homed")
        self.motor_lbl.config(text="  ".join(st))
        if s["fault"]:
            self.fault_lbl.config(background="#c62828", foreground="white")
        else:
            self.fault_lbl.config(background="#cccccc", foreground="black")
        if hasattr(self, "test_thr_lbl"):
            self.test_thr_lbl.config(text=str(s["thr"]))
        if hasattr(self, "enable_var") and s["enabled"] != self.enable_var.get():
            self.enable_var.set(s["enabled"])

    def _draw_level_bar(self, cross):
        if getattr(self, "wiz_step", -1) != 1:
            return
        ax = self.wiz_ax
        ax.clear()
        ax.set_xlim(0, 255); ax.set_ylim(0, 1); ax.set_yticks([])
        ax.set_xlabel("crossing duty (signal level)")
        ax.grid(True, axis="x", alpha=0.3)
        if cross >= 0:
            ax.barh([0.5], [cross], height=0.5, color="tab:green")
            ax.text(min(cross + 4, 250), 0.5, str(cross), va="center")
        self.wiz_canvas.draw_idle()

    def _draw_home_edges(self, lead_deg, trail_deg):
        if not hasattr(self, "wiz_ax") or getattr(self, "wiz_step", -1) != 3:
            return
        for ln in getattr(self, "wiz_edges", []):
            try:
                ln.remove()
            except Exception:
                pass
        self.wiz_edges = [
            self.wiz_ax.axhline(0.0, color="green", ls="--", lw=0.8, alpha=0.7),
            self.wiz_ax.axhline(lead_deg, color="red", ls=":", lw=0.8, alpha=0.7),
            self.wiz_ax.axhline(trail_deg, color="red", ls=":", lw=0.8, alpha=0.7),
        ]

    def _render_scan(self):
        if not hasattr(self, "scan_line") or not self.scan_off:
            return
        order = sorted(range(len(self.scan_off)), key=lambda i: self.scan_off[i])
        xs = [self.scan_off[i] for i in order]
        cross = [self.scan_cross[i] for i in order]
        hi = [self.scan_hi[i] for i in order]
        # connected line: cross>=0 as y, cross<0 as a NaN GAP (dependency-free)
        ys = [c if c >= 0 else float("nan") for c in cross]
        self.scan_line.set_data(xs, ys)
        self.scan_pts.set_data(xs, ys)
        # saturated plateau: real reading near the top OR stuck-HIGH rail -> red @255
        sat = [(xs[i], cross[i] if cross[i] >= 0 else 255)
               for i in range(len(xs))
               if cross[i] >= SCAN_SAT_DUTY or (cross[i] < 0 and hi[i])]
        self.scan_sat.set_data([q[0] for q in sat], [q[1] for q in sat])
        # no-signal: cross<0 and NOT stuck-high (off flag / stuck LOW) -> grey x @floor
        gap = [xs[i] for i in range(len(xs)) if cross[i] < 0 and not hi[i]]
        self.scan_gap.set_data(gap, [self.scan_dmin - 2] * len(gap))
        if xs:
            self.scan_ax.set_xlim(min(xs) - 5, max(xs) + 5)
        self.wiz_canvas.draw_idle()

    def _render_autohome(self):
        if not hasattr(self, "ah_line"):
            return
        if self.ah_char_pos:
            order = sorted(range(len(self.ah_char_pos)), key=lambda i: self.ah_char_pos[i])
            xs = [self.ah_char_pos[i] for i in order]
            ys = [self.ah_char_cross[i] for i in order]     # already NaN-gapped
            self.ah_line.set_data(xs, ys)
            self.wiz_ax.relim(); self.wiz_ax.autoscale_view()
        if self.ah_thr is not None:                          # chosen threshold level
            if self.ah_thr_line is None:
                self.ah_thr_line = self.wiz_ax.axhline(
                    self.ah_thr, color="tab:blue", ls="--", lw=0.9, alpha=0.7)
            else:
                self.ah_thr_line.set_ydata([self.ah_thr, self.ah_thr])
        if self.ah_pmin is not None:                         # coarse dip centre
            self.ah_min_line.set_xdata([self.ah_pmin, self.ah_pmin])
            self.ah_min_line.set_alpha(0.5)
        for which, pos in self.ah_edges:                     # detected edges
            ln = self.ah_lead_line if str(which) in ("lead", "0") else self.ah_trail_line
            ln.set_xdata([pos, pos]); ln.set_alpha(0.7)
        if self.ah_home is not None:                         # final home centre
            self.ah_home_line.set_xdata([self.ah_home, self.ah_home])
            self.ah_home_line.set_alpha(0.9)
        self.wiz_canvas.draw_idle()

    def _redraw(self):
        # advanced plots
        if self.live_t:
            self.adv_live_fig.axes[0].lines[0].set_data(self.live_t, self.live_deg)
            self.adv_live_fig.axes[1].lines[0].set_data(self.live_t, self.live_level)
            self._autoscale_live(self.adv_live_fig)
            self.adv_live_canvas.draw_idle()
        if self.sweep_t:
            self.adv_sweep_line.set_data(self.sweep_t, self.sweep_duty)
            xmax = self.sweep_t[-1]
            self.adv_sweep_ax.set_xlim(max(0, xmax - SWEEP_WINDOW_S), max(SWEEP_WINDOW_S, xmax))
            self.adv_sweep_canvas.draw_idle()

        # wizard plot for the current step
        step = getattr(self, "wiz_step", 0)
        if step in (0, 3) and self.live_t and hasattr(self, "wiz_ld"):
            self.wiz_ld.set_data(self.live_t, self.live_deg)
            self.wiz_ll.set_data(self.live_t, self.live_level)
            self._autoscale_live(self.wiz_fig)
            self.wiz_canvas.draw_idle()
        elif step == 2 and hasattr(self, "ah_line"):
            if self.autohome_active or self.ah_char_pos:
                self._render_autohome()
        elif step == 1 and hasattr(self, "scan_line"):
            self._render_scan()

    def _autoscale_live(self, fig):
        ax = fig.axes[0]
        if not self.live_t:
            return
        xmax = self.live_t[-1]
        ax.set_xlim(max(0, xmax - LIVE_WINDOW_S), max(LIVE_WINDOW_S, xmax))
        ax.relim(); ax.autoscale_view(scalex=False, scaley=True)

    def on_close(self):
        try:
            self.client.send("C 0")
        except Exception:
            pass
        self.client.close()
        self.root.destroy()


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", default=None, help="serial port (auto-detect if omitted)")
    ap.add_argument("--baud", type=int, default=BAUD_DEFAULT)
    args = ap.parse_args()
    port = args.port or autodetect_port()
    root = tk.Tk()
    BenchGUI(root, port, args.baud)
    root.mainloop()


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Live monitor for the optical home-switch threshold sweep firmware.

Reads the debug-UART CSV stream emitted by HomeSwitchTest firmware:

    D,<millis>,<A_duty>,<B_duty>,<A_sigma_x10>,<B_sigma_x10>

(-1 in a duty field means "no crossing found this sweep".) Plots each axis's
crossing duty (and the equivalent threshold voltage) over time in a live
matplotlib window, with the rolling sigma shown in the legend.

Usage:
    python home_switch_monitor.py                 # auto-detect the port
    python home_switch_monitor.py --port /dev/cu.usbmodem2103
    python home_switch_monitor.py --csv log.csv   # also append raw rows to CSV
"""

import argparse
import csv
import glob
import sys
import time
from collections import deque

import serial
import matplotlib.pyplot as plt
from matplotlib.animation import FuncAnimation

VREF = 3.3
WINDOW_SECONDS = 60.0      # how much history to show
BAUD_DEFAULT = 115200


import serial.tools.list_ports

def autodetect_port():
    """Finds the most likely serial port for the ST-Link VCP."""
    ports = sorted(serial.tools.list_ports.comports())
    for p in ports:
        # Nucleo/ST-Link VCP usually mentions "STMicroelectronics" or "ST-Link".
        if "ST-LINK" in p.description.upper() or "STMICROELECTRONICS" in p.description.upper():
            return p.device
    # Fallback to anything that looks like a USB modem or ACM device.
    for p in ports:
        if "usbmodem" in p.device or "ACM" in p.device:
            return p.device
    return ports[0].device if ports else None


def duty_to_volts(duty):
    return VREF * duty / 255.0


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", default=None, help="serial port (auto-detect if omitted)")
    ap.add_argument("--baud", type=int, default=BAUD_DEFAULT)
    ap.add_argument("--window", type=float, default=WINDOW_SECONDS,
                    help="seconds of history to display")
    ap.add_argument("--csv", default=None, help="append parsed rows to this CSV file")
    args = ap.parse_args()

    port = args.port or autodetect_port()
    if not port:
        sys.exit("No serial port found. Plug in the board or pass --port.")

    # dtr/rts False so opening the port doesn't reset the board.
    ser = serial.Serial()
    ser.port = port
    ser.baudrate = args.baud
    ser.timeout = 0.1
    ser.dtr = False
    ser.rts = False
    ser.open()
    print(f"Listening on {port} @ {args.baud}")

    csv_writer = None
    if args.csv:
        csv_file = open(args.csv, "a", newline="")
        csv_writer = csv.writer(csv_file)
        if csv_file.tell() == 0:
            csv_writer.writerow(["host_time", "millis", "A_duty", "B_duty",
                                 "A_sigma", "B_sigma"])

    # Time-bounded history (host wall-clock seconds as x).
    t0 = time.monotonic()
    t_hist = deque()
    a_hist = deque()
    b_hist = deque()
    last = {"a": None, "b": None, "asig": 0.0, "bsig": 0.0,
            "astuck": None, "bstuck": None}

    fig, ax = plt.subplots(figsize=(10, 5))
    ax.set_title("Optical home-switch threshold (live)")
    ax.set_xlabel("time (s)")
    ax.set_ylabel("crossing duty (0-255)")
    ax.set_ylim(-5, 260)
    ax.grid(True, alpha=0.3)

    # Right axis in volts, locked to the duty axis.
    axv = ax.twinx()
    axv.set_ylabel("threshold (V)")
    axv.set_ylim(duty_to_volts(-5), duty_to_volts(260))

    (line_a,) = ax.plot([], [], "o-", ms=3, color="tab:blue", label="A")
    (line_b,) = ax.plot([], [], "s-", ms=3, color="tab:orange", label="B")
    legend = ax.legend(loc="upper left")

    def read_serial():
        """Drain whatever lines are waiting; update history."""
        while ser.in_waiting:
            raw = ser.readline().decode("utf-8", "replace").strip()
            if not raw or raw.startswith("#"):
                if raw:
                    print(raw)
                continue
            if not raw.startswith("D,"):
                continue
            parts = raw.split(",")
            if len(parts) < 6:
                continue
            try:
                millis = int(parts[1])
                a = int(parts[2])
                b = int(parts[3])
                asig = int(parts[4]) / 10.0
                bsig = int(parts[5]) / 10.0
            except ValueError:
                continue
            # Optional trailing comparator state: A_lo,A_hi,B_lo,B_hi (1=HIGH).
            a_hi = b_hi = None
            if len(parts) >= 10:
                try:
                    a_hi = int(parts[7])
                    b_hi = int(parts[9])
                except ValueError:
                    pass

            # Always plot a datapoint and keep the line connected: when there is
            # no crossing, pin to the rail the comparator is stuck on
            # (stuck HIGH -> 255 / 100%, stuck LOW -> 0 / 0%).
            now = time.monotonic() - t0
            t_hist.append(now)
            a_hist.append(a if a >= 0 else (255 if a_hi else 0))
            b_hist.append(b if b >= 0 else (255 if b_hi else 0))
            last.update(a=(a if a >= 0 else None),
                        b=(b if b >= 0 else None),
                        asig=asig, bsig=bsig,
                        astuck=a_hi, bstuck=b_hi)

            if csv_writer:
                csv_writer.writerow([f"{now:.3f}", millis, a, b, asig, bsig])
                csv_file.flush()

        # Trim to the display window.
        cutoff = (time.monotonic() - t0) - args.window
        while t_hist and t_hist[0] < cutoff:
            t_hist.popleft(); a_hist.popleft(); b_hist.popleft()

    def update(_frame):
        read_serial()
        line_a.set_data(t_hist, a_hist)
        line_b.set_data(t_hist, b_hist)
        if t_hist:
            ax.set_xlim(max(0, t_hist[-1] - args.window), max(args.window, t_hist[-1]))

        def lbl(name, duty, sig, stuck):
            if duty is None:
                rail = "?" if stuck is None else ("HIGH" if stuck else "LOW")
                return f"{name}: -- stuck {rail}"
            return f"{name}: {duty}  {duty_to_volts(duty):.2f}V  sigma {sig:.1f}"

        line_a.set_label(lbl("A", last["a"], last["asig"], last["astuck"]))
        line_b.set_label(lbl("B", last["b"], last["bsig"], last["bstuck"]))
        ax.legend(loc="upper left")
        return line_a, line_b

    # cache_frame_data=False: this is an unbounded live stream.
    _anim = FuncAnimation(fig, update, interval=100, blit=False,
                          cache_frame_data=False)
    try:
        plt.show()
    finally:
        ser.close()
        if csv_writer:
            csv_file.close()


if __name__ == "__main__":
    main()

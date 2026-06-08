#!/usr/bin/env python3
"""Live logic-analyzer view for the fixed-threshold home-switch tracker firmware.

The firmware (env: home_switch_track) streams ONE byte per frame at up to 60 Hz:

    bit7 = 1   (sync marker, always set)
    bit0 = A level     bit1 = B level
    bit2 = A edge      bit3 = B edge   (changed since the previous frame)

This reads that stream and shows A and B as two scrolling digital traces, with
the edge bits drawn as tick markers, plus the live state and measured frame rate.

Usage:
    python home_switch_tracker.py                 # auto-detect the port
    python home_switch_tracker.py --port /dev/cu.usbmodem2103
    python home_switch_tracker.py --window 8      # seconds of history
"""

import argparse
import glob
import sys
import time
from collections import deque

import serial
import matplotlib.pyplot as plt
from matplotlib.animation import FuncAnimation

BAUD_DEFAULT = 115200
WINDOW_SECONDS = 6.0


def autodetect_port():
    candidates = sorted(glob.glob("/dev/cu.usbmodem*") + glob.glob("/dev/ttyACM*"))
    return candidates[0] if candidates else None


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--port", default=None, help="serial port (auto-detect if omitted)")
    ap.add_argument("--baud", type=int, default=BAUD_DEFAULT)
    ap.add_argument("--window", type=float, default=WINDOW_SECONDS,
                    help="seconds of history to display")
    args = ap.parse_args()

    port = args.port or autodetect_port()
    if not port:
        sys.exit("No serial port found. Plug in the board or pass --port.")

    # dtr/rts False so opening the port doesn't reset the board.
    ser = serial.Serial()
    ser.port = port
    ser.baudrate = args.baud
    ser.timeout = 0
    ser.dtr = False
    ser.rts = False
    ser.open()
    print(f"Listening on {port} @ {args.baud}")

    t0 = time.monotonic()
    t_hist = deque()       # frame arrival time (s)
    a_hist = deque()       # 0/1
    b_hist = deque()
    a_edges = deque()      # arrival times where A edged
    b_edges = deque()
    rate = {"fps": 0.0, "n": 0, "t": time.monotonic()}
    last = {"a": 0, "b": 0}

    fig, ax = plt.subplots(figsize=(10, 4))
    ax.set_title("Home-switch tracker (live)")
    ax.set_xlabel("time (s)")
    ax.set_yticks([0.5, 2.5])
    ax.set_yticklabels(["B", "A"])
    ax.set_ylim(-0.4, 3.4)
    ax.grid(True, axis="x", alpha=0.3)

    # A trace drawn around y=2..3, B trace around y=0..1 (step lines).
    (line_a,) = ax.step([], [], where="post", color="tab:blue", lw=1.6)
    (line_b,) = ax.step([], [], where="post", color="tab:orange", lw=1.6)
    edge_a = ax.scatter([], [], marker="|", s=200, color="tab:blue")
    edge_b = ax.scatter([], [], marker="|", s=200, color="tab:orange")

    def read_serial():
        data = ser.read(4096)
        now = time.monotonic() - t0
        for byte in data:
            if not (byte & 0x80):
                continue                      # ASCII banner / unsynced byte
            a = byte & 0x01
            b = (byte >> 1) & 0x01
            ae = (byte >> 2) & 0x01
            be = (byte >> 3) & 0x01
            t_hist.append(now)
            a_hist.append(a)
            b_hist.append(b)
            if ae:
                a_edges.append(now)
            if be:
                b_edges.append(now)
            last["a"], last["b"] = a, b
            rate["n"] += 1

        # Trim to the display window.
        cutoff = (time.monotonic() - t0) - args.window
        while t_hist and t_hist[0] < cutoff:
            t_hist.popleft(); a_hist.popleft(); b_hist.popleft()
        while a_edges and a_edges[0] < cutoff:
            a_edges.popleft()
        while b_edges and b_edges[0] < cutoff:
            b_edges.popleft()

        # Measured frame rate (refresh ~ once a second).
        tnow = time.monotonic()
        if tnow - rate["t"] >= 1.0:
            rate["fps"] = rate["n"] / (tnow - rate["t"])
            rate["n"] = 0
            rate["t"] = tnow

    def update(_frame):
        read_serial()
        # Offset A to 2..3, B to 0..1 so both traces are visible.
        line_a.set_data(t_hist, [2 + v for v in a_hist])
        line_b.set_data(t_hist, [0 + v for v in b_hist])
        edge_a.set_offsets([[t, 2.5] for t in a_edges] or [[None, None]])
        edge_b.set_offsets([[t, 0.5] for t in b_edges] or [[None, None]])
        if t_hist:
            ax.set_xlim(max(0, t_hist[-1] - args.window), max(args.window, t_hist[-1]))
        ax.set_title(f"Home-switch tracker (live)   "
                     f"A={last['a']} B={last['b']}   {rate['fps']:.0f} fps")
        return line_a, line_b, edge_a, edge_b

    _anim = FuncAnimation(fig, update, interval=16, blit=False,
                          cache_frame_data=False)
    try:
        plt.show()
    finally:
        ser.close()


if __name__ == "__main__":
    main()

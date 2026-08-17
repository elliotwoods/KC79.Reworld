#!/usr/bin/env python3
"""Decode a KC79 Reworld wire frame (COBS + msgpack) for cross-checking the
Rust encoder. Usage:

    python crosscheck.py "03 93 FF 08 81 A4 70 6F 6C 6C C0 00"
    python crosscheck.py golden-frames/position-report.hex

Requires: pip install cobs msgpack
"""
import sys
from pathlib import Path

from cobs import cobs
import msgpack


def main() -> None:
    arg = sys.argv[1]
    if Path(arg).exists():
        arg = Path(arg).read_text()
    data = bytes.fromhex(arg.replace(" ", "").replace("\n", ""))

    # Split on 0x00 delimiters; decode each frame
    for i, frame in enumerate(f for f in data.split(b"\x00") if f):
        decoded = cobs.decode(frame)
        message = msgpack.unpackb(decoded, strict_map_key=False)
        print(f"frame {i}: msgpack bytes = {decoded.hex(' ')}")
        print(f"frame {i}: decoded       = {message!r}")
        assert isinstance(message, list) and len(message) >= 3, "not an envelope"
        target, source, body = message[0], message[1], message[2]
        print(f"frame {i}: target={target} source={source} body={body!r}")


if __name__ == "__main__":
    main()

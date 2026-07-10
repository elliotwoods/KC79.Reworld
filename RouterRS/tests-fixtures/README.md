# Test fixtures

Shared golden fixtures used by the Rust crates (and, for NDJSON, by the
RouterReports Node.js viewer tests).

- `golden-frames/position-report.hex` — a real captured wire frame from
  `IPython/2024-11-23 - COBS issues/cobsissue.py`: firmware position report
  `[0, 1, {"p": [94848, 0, 94848, 0]}]` (COBS-encoded, `0x00`-delimited).
  Note the frame uses forced `0xD0` int8 addresses and `0xD2` int32 positions
  (msgpack-arduino style), while the Router's msgpack11 TX path uses minimal
  encodings — both are valid on this wire.
- `crosscheck.py` — independent verification of Rust-encoded frames using the
  Python `cobs` and `msgpack` packages (the same ones used for the original
  on-site debugging). Run `pip install cobs msgpack` then
  `python crosscheck.py <hex-string-or-file>` to decode any frame.
- `config.sample.json` — a representative production-style `config.json`
  exercised by the router-core config tests (added in Phase 2).
- `pilot-vectors.csv` — kinematics golden table (added in Phase 2).

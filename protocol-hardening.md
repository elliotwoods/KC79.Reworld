# RS485 Protocol Hardening — Recommendations

**Status:** proposal only. No code changes are committed. Apply on a test bench
following the procedures below.

**Date:** 2026-05-29
**Scope:** the RS485 link between the Router (openFrameworks app) and the Portal
firmware (STM32G070), plus the firmware's ID daisy-chain.

---

## 1. Background — how the protocol works today

- **Transport:** RS485 half-duplex, 115200 8N1. DE pin (PA1) toggles TX/RX.
- **Framing:** COBS (zero-delimited).
- **Body:** MessagePack.
- **Frame:** a 3-element array `[target_i8, source_i8, body]`.
  - `target`: `0` = router/host, `1..127` = devices, `-1` = broadcast.
  - `body`: `nil` (ping), a `string5` magic word (`"FW"` → reboot to bootloader),
    a map (command), or a bare `bool` (ACK).
- **Reliability:** the router waits up to `responseWindow_ms` (300 ms) for a reply
  after each ACK-needing send.
- **IDs:** assigned over a separate UART daisy-chain (PB8/PB9); each device adds 1
  and forwards. A 4-bit DIP (PD0–PD3) provides a fallback ID.

Key source files:

| Concern | File |
| --- | --- |
| Router framing / send / receive | `Router/src/Modules/Hardware/RS485.cpp` `.h` |
| Router per-portal routing | `Router/src/Modules/Hardware/Column.cpp`, `Portal.cpp` |
| Firmware framing / dispatch | `PortalFW/src/Modules/RS485.cpp` `.h` |
| Firmware command handlers | `PortalFW/src/Modules/App.cpp` |
| Firmware ID daisy-chain | `PortalFW/src/Modules/ID.cpp` `.h` |
| MessagePack + COBS stream (submodule) | `PortalFW/lib/msgpack-arduino/src/msgpack/` |
| COBS codec (router) | `Router/src/cobs-c/` |

> Note: `PortalFW/lib/msgpack-arduino` is a **git submodule**. Changes to
> `COBSRWStream.*` are committed in that repo, then the submodule pointer is
> bumped in the root repo. Plan for two commits.

---

## 2. Findings, in priority order

### Finding 1 — ID daisy-chain checksum is silently disabled (correctness bug)

`PortalFW/src/Modules/ID.cpp` (`readIncomingID`):

```cpp
if(targetID ^ 'C' == this->incomingBytes[1]
    && targetID ^ 'R' == this->incomingBytes[2]
    && targetID ^ 'C' == this->incomingBytes[3]) {
```

In C++ `==` binds tighter than `^`, so this parses as
`targetID ^ ('C' == incomingBytes[1])`, i.e. `targetID ^ (0 or 1)` — never the
intended checksum comparison. The checksum effectively always "passes", so a
single noise byte on the ID line can be accepted as a real ID and, because each
device forwards `received + 1`, shift the addressing of every downstream device.

Severity: **high** (addressing integrity), effort: **trivial**.

### Finding 2 — No payload integrity (corruption risk)

COBS only frames; it does not detect corruption. MessagePack only catches
*structurally* invalid data. A bit-flip inside a valid `int32` (e.g. a motion
target) decodes cleanly and can command a wild move. The UART is 8N1 — **no
parity** — and parity is not viable as a primary mechanism because it cannot be
carried across the TCP transport (`Router/src/SerialDevices/TCP.h`) and is not
exposed by `ofSerial`/`SerialDevices::IDevice`.

Recommendation: append a **CRC-16/CCITT-FALSE** over each frame.

Severity: **high** (safety on a motor bus), effort: **medium**.

### Finding 3 — ACK is cosmetic; no correlation; no retransmission

On receive the router records *any* frame from the target as "the reply"
(`serialThreadReceive` pushes `json[1]` for any `json.size() >= 3`) and matches
purely on source ID (`waitForReceive`). Consequences:

- The ACK's success/failure `bool` (`json[2]`) is **never read**, so the
  firmware's `sendACK(false)` is indistinguishable from `sendACK(true)`.
- A status or position frame from a device counts as an ACK.
- There is no request/ACK correlation (no sequence number), so a late reply to
  command *N* can be accepted as the ACK for *N+1*.
- On timeout the router only logs — it never retransmits. Delivery is best-effort
  despite looking reliable.

Recommendation: sequence numbers + an unambiguous ACK body + retransmission.

Severity: **medium/high** (reliability), effort: **medium**.

### Finding 4 — Unauthenticated reboot-to-bootloader (robustness)

Any frame whose body is a `string5` starting `"FW"` triggers `NVIC_SystemReset()`
into the bootloader (`PortalFW/src/Modules/RS485.cpp`). With no CRC (Finding 2),
a corrupted frame that happens to decode as `"FW…"` can bounce a device mid-move.
Fix is folded into Finding 2: **validate the CRC before rebooting**.

### Finding 5 — ID receive buffer never cleared (robustness)

`readIncomingID` keeps a sliding 4-byte window and never resets on a terminator,
so bytes from one frame can combine with the next. Fix is folded into Finding 1:
clear the buffer on each `0` terminator.

### What is already good (keep)

COBS + MessagePack is compact and extensible; the `arraySize < 3` tolerance lets
us add trailing fields without breaking old firmware; collation sheds stale
motion commands; half-duplex DE timing is handled; early-ACK for long routines is
the right pattern.

---

## 3. Proposed end-state wire format

```
[target_i8, source_i8, body, seq_u8, crc16_u16]
```

- `seq` and `crc16` are appended as MessagePack array elements. This is
  **backward compatible both ways** (verified in code):
  - Firmware reads elements 0–2 then `nextIncomingPacket()` discards the rest.
  - Router consume path (`Column::processIncoming`) reads only `json[0..2]`.
- `crc16` covers the serialized prefix `[header, target, source, body, seq]`
  (everything before the CRC field). Because the CRC is always encoded as a
  3-byte MessagePack `uint16` (`0xcd hi lo`), the receiver can CRC "all decoded
  bytes except the final 3".
- The ACK body changes from a bare `bool` to `{"ack": <bool>}` so it can never be
  confused with status (`{...}`) or position (`{"p":[...]}`) frames.

### CRC algorithm (must be byte-identical on both ends)

CRC-16/CCITT-FALSE: poly `0x1021`, init `0xFFFF`, no reflection, xorout `0x0000`.

```c
uint16_t crc = 0xFFFF;
for each byte b:
    crc ^= (uint16_t)b << 8;
    for (int i = 0; i < 8; i++)
        crc = (crc & 0x8000) ? (crc << 1) ^ 0x1021 : (crc << 1);
```

- **Router:** `boost::crc_ccitt_false_t` is already vendored
  (`Router/src/boost/crc.hpp`) and matches this definition. To remove any doubt,
  prefer a small hand-rolled header `Router/src/crc16ccitt.h` with the loop above
  and a unit test that compares it to Boost (see Test 0).
- **Firmware:** inline the identical loop inside `COBSRWStream` so the submodule
  stays self-contained.

---

## 4. How to implement (per stage)

Each stage is independently landable. Land Stage 1 first; Stage 2 must be on all
nodes (in "emit, don't verify" mode) before Stage 3 is enabled.

### Stage 1 — ID checksum (Findings 1 & 5)

File: `PortalFW/src/Modules/ID.cpp`, `readIncomingID`.

- Parenthesise each XOR: `((targetID ^ 'C') == this->incomingBytes[1]) && …`.
- After the terminator-processing block (inside `if (byte == 0)`), add
  `this->incomingBytes.clear();`.

No wire change; no router change. Smallest, highest-confidence fix.

### Stage 2 — payload CRC (Findings 2 & 4)

Add a running CRC at the COBS layer and emit/verify the trailing CRC.

**Submodule `COBSRWStream` (`PortalFW/lib/msgpack-arduino/src/msgpack/COBSRWStream.*`):**

- Add `uint16_t runningCRC` to both the `receive` and `transmit` structs (init
  `0xFFFF`).
- In `read()`: fold each returned byte into `receive.runningCRC`.
- In `write(uint8_t)`: fold **every** logical byte (including zeros) into
  `transmit.runningCRC` *before* the COBS zero-handling branch.
- Reset `receive.runningCRC = 0xFFFF` where a new packet begins (the
  `outgoingStreamIsAtStartOfNextPacket = true` line in `decodeIncoming`).
- Reset `transmit.runningCRC = 0xFFFF` at the end of `writeEOP()`.
- Expose `getRxRunningCRC()` / `getTxRunningCRC()`.

> Why `read()`/`write()`: every reader (`readInt`, `readString`, …) ultimately
> funnels through the virtual `read()`/`readBytes`, and every writer through
> `write()`. `peek()` does not consume, so it must not accumulate.
> `decodeIncoming()` reads the *underlying* serial, not `COBSRWStream::read()`,
> so there is no double counting.

**Firmware `PortalFW/src/Modules/RS485.cpp`:**

- Add a `finishFrame(uint8_t seq)` helper: write `seq` (`writeIntU8`), snapshot
  `getTxRunningCRC()`, write it (`writeIntU16`), then `endTransmission()`.
- Change the three senders (`sendStatusReport`, `sendPositions`, `sendACK`) to
  declare array size **5** and call `finishFrame(this->lastRxSeq)` instead of
  `endTransmission()`. Change `sendACK`'s body to the map `{"ack": success}`.
- In `processCOBSPacket`: keep `arraySize` in scope; after the body is consumed,
  if `arraySize >= 4` read `seq` into `lastRxSeq`; if `arraySize >= 5` snapshot
  `getRxRunningCRC()` **before** reading the CRC, then read it and (when
  `verifyCRC`) reject on mismatch.
- **Defer the `"FW"` reboot** until after the CRC check (set a local flag, reboot
  at the end of the function) so a corrupted frame cannot trigger it.
- Add members `uint8_t lastRxSeq = 0;` and `bool verifyCRC = false;`.

**Router `Router/src/Modules/Hardware/RS485.cpp` `.h`:**

- `serialThreadSend`: after `packet.render()`, build a local `frame` copy of
  `packet.msgpackBinary`; if append-CRC is enabled and `frame[0]` is a fixarray
  (`0x9?`), bump the element count by 2, append `seq` (`0xcc, value`), compute the
  CRC over the bytes so far, append it (`0xcd, hi, lo`), and COBS-encode `frame`.
- `serialThreadReceive`: if verify is enabled and `json.size() >= 5`, recompute
  the CRC over `binaryMessagePack` minus the last 3 bytes and compare to
  `json[4]`; drop and notify on mismatch.
- Add a `Reliability` parameter group: `appendCRC` (default **on**), `verifyCRC`
  (default **off**), plus the Stage 3 flags below.

Rollout: emit always, verify only behind a flag. A new router talking to old
firmware (and vice-versa) keeps working because the extra elements are ignored.

### Stage 3 — real ACKs (Finding 3)

**Router:**

- Add `uint8_t seq` to `Packet`; add a per-target `uint8_t txSeq[128]`.
- Stamp `packet.seq = txSeq[target]++` **at send time** (not enqueue, so collation
  cannot carry a stale seq). Skip for broadcast (`-1`).
- Replace/augment `repliesSeenFrom` with an `AckRecord { source, seq, success }`
  list. In `serialThreadReceive`, only push an `AckRecord` when `json[2]` is an
  object containing `"ack"`.
- In `serialThreadSend`, when strict mode is on: match on
  `(source == target && seq == packet.seq)`, capture `success`, and wrap
  send+wait in a bounded retry loop (`maxRetries`); resend the **same** seq on
  NACK/timeout.
- Add parameters `strictACK` (default **off**) and `maxRetries` (default 3).

**Firmware:** already covered by Stage 2 (`lastRxSeq` echo, `{"ack": …}` body).

Interactions to preserve: broadcast (`-1`) sets `disableACK` and consumes no seq;
early-ACK (`init`/`calibrate`/`home`) emits a real `{"ack":true}` the matcher
catches, while the later status frame is correctly ignored; retransmitted
`m`/routine commands are idempotent (the `isInsideRoutine` guard).

### Default flags (safe rollout)

| Flag (Router ▸ RS485 ▸ Reliability) | Default | Effect |
| --- | --- | --- |
| `Append seq+CRC` | **on** | emit 5-element frames (backward compatible) |
| `Verify RX CRC` (router + firmware `verifyCRC`) | **off** | reject on CRC mismatch |
| `Strict ACK + retransmit` | **off** | seq-matched ACKs + retries |

With defaults, behaviour is unchanged except for extra trailing bytes that old
firmware already ignores. Turn verification on only after Test 1 passes.

---

## 5. Bench test plan

### Equipment

- 1× Router host (the machine running the openFrameworks app) with the USB↔RS485
  adapter normally used (or the TCP↔RS485 gateway if testing that path).
- At least **2× Portal boards** (STM32G070) on the same RS485 bus, each with a
  known DIP-switch ID, wired A/B/GND in a daisy-chain with proper termination at
  the bus ends.
- The ST-Link (or USB DFU) used to flash firmware.
- A logic analyzer or USB-serial sniffer on the RS485 A/B pair (e.g. Saleae or a
  second adapter in listen-only mode). Strongly recommended for Tests 1–3.
- For Test 4: jumper wires to make the ID daisy-chain (PB8 TX → next board PB9 RX).

> Safety: keep motor current low or mechanically free the prisms during move
> tests so an unexpected command cannot damage hardware.

### Test 0 — CRC parity unit test (no hardware)

**Goal:** prove the firmware-style CRC and `boost::crc_ccitt_false_t` produce
identical values, so the two ends will agree.

**Setup:** a small host-side C++ test (can live under `PortalFW/test/` for the
PlatformIO native env, or a throwaway `main`). Generate N random byte vectors,
compute the CRC with the hand-rolled loop and with Boost, assert equality. Add a
known vector check: CRC-16/CCITT-FALSE of ASCII `"123456789"` is `0x29B1`.

**Pass:** all vectors match and `"123456789"` → `0x29B1`.

### Test 1 — CRC agreement on the wire (1 device)

**Goal:** confirm both ends compute the same CRC over real frames.

**Physical setup:** one Portal board on the bus, flashed with Stage 1+2 firmware.
Router with `Append seq+CRC` **on**, `Verify RX CRC` **off** to start.

**Steps:**
1. With verify off, run normal traffic (poll + a few small moves). Confirm comms
   are healthy (status/positions update, pings ACK). This proves the extra bytes
   don't break framing.
2. On the sniffer, capture one Router→device frame and one device→Router frame.
   Manually verify the last 3 bytes are `0xcd hi lo` and that
   CRC(prefix) == `hi:lo`.
3. Set firmware `verifyCRC = true` and Router `Verify RX CRC` **on**. Repeat the
   traffic.

**Pass:** with verify on, traffic is still healthy (ACKs and status keep
flowing). **Fail:** comms stop when verify is enabled → the two CRC
implementations disagree; turn verify back off and diff them (byte order,
init value, which bytes are covered).

### Test 2 — Corruption is detected (1 device)

**Goal:** confirm a corrupted frame is rejected, not acted upon.

**Physical setup:** as Test 1 with verify **on**.

**Steps (pick one):**
- *Software injection:* add a temporary debug toggle in `serialThreadSend` that
  flips one bit of `binaryCOBS` before transmit. Send a move.
- *Hardware injection:* briefly induce a glitch (e.g. tap noise onto the A/B pair)
  while spamming moves — less precise but realistic.

**Observe:** the device must **not** move to a wrong position; the firmware should
log a format/CRC error and (for a unicast) return `{"ack":false}`. For a
device→Router status frame, flip a bit on that path and confirm the Router's
"Rx Error" indicator fires and the frame is dropped.

**Pass:** corrupted frames never produce motion or bad state; errors are flagged.

### Test 3 — Real ACK + retransmission (1 device)

**Goal:** confirm seq-matched ACKs and retransmission.

**Physical setup:** one device, verify **on**, `Strict ACK + retransmit` **on**,
`maxRetries = 3`. Enable `Print ACK time`.

**Steps:**
1. Send a single `m` move. Confirm exactly one ACK is logged with the correct seq
   and `ok`, and that the trailing positions frame does **not** get mistaken for
   the next command's ACK.
2. Force a NACK: send a deliberately malformed body (or, with verify on, a
   bit-flipped frame). Confirm the Router logs `{"ack":false}` and retransmits up
   to `maxRetries`, then logs failure.
3. Drop an ACK: temporarily make the firmware skip every Nth ACK. Confirm the
   Router retransmits the **same** seq and the device does not double-apply
   (moves are idempotent; routines are guarded by `isInsideRoutine`).

**Pass:** one positive ACK per delivered command; retransmission on NACK/timeout;
no duplicate side-effects.

### Test 4 — ID daisy-chain fix (2 devices)

**Goal:** confirm the Stage 1 checksum fix lets IDs propagate.

**Physical setup:** two boards chained on the ID UART: board A `PB8 (TX)` →
board B `PB9 (RX)`, common ground. Set board A's DIP to ID `N`.

**Steps:**
1. Flash Stage 1 firmware to both. Power up.
2. Watch board B's debug log (`ID` module) for `New ID : N+1`.
3. *Regression check:* flash the **old** firmware to board B and confirm it does
   **not** reliably pick up `N+1` from the chain (it only uses its own DIP) —
   demonstrating the bug the fix addresses.

**Loopback alternative (1 board):** wire a board's `PB8 (TX)` to its own
`PB9 (RX)` and confirm it reads back `value` and advances to `value + 1`.

**Pass:** with the fix, the downstream ID tracks `upstream + 1`; without it, it
does not.

### Test 5 — Mixed-fleet compatibility (≥2 devices)

**Goal:** confirm rollout is incremental (no flag-day).

**Physical setup:** one board on **new** firmware, one on **old** firmware, same
bus. Router with `Append seq+CRC` **on**, `Verify RX CRC` **off**,
`Strict ACK` **off**.

**Steps:** run normal traffic addressed to both devices; ping/poll/move each.

**Pass:** both devices respond correctly; the old device ignores the extra
trailing bytes; the new device accepts legacy-shaped replies. Only after the whole
fleet is updated should `Verify RX CRC` and `Strict ACK` be enabled.

---

## 6. Suggested landing order

1. Stage 1 (ID fix) — land and flash fleet-wide.
2. Stage 2 firmware + router in **emit-only** mode (verify off) — Test 1, Test 5.
3. Enable `Verify RX CRC` once Test 1 passes fleet-wide — Test 2.
4. Stage 3 + enable `Strict ACK + retransmit` — Test 3.

Remember the submodule two-step for any `COBSRWStream` change: commit in
`msgpack-arduino`, then bump the submodule pointer in the root repo.

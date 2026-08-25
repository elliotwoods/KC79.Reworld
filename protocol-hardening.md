# RS485 Protocol Hardening — Recommendations

**Status:** partly implemented. This document is kept as the analysis it always was, not as a
to-do list — several of its findings have since shipped, and saying so here is cheaper than
finding out by reading five projects.

| Finding | State |
| --- | --- |
| 1 — ID daisy-chain checksum silently disabled | **fixed** (`PortalFW/src/Modules/ID.cpp`) |
| 2 — no payload integrity | **partly**: the CRC-16 trailer exists and PortalFW verifies it at commit time, but verification is off by default and the Router does not append one to ordinary commands. It *is* enforced end to end on the bootloader control plane |
| 3 — ACK is cosmetic, no correlation, no retransmission | **partly**: replies carry a sequence number, and a firmware upload to a v6 bootloader now finds and repairs its own losses. Ordinary command traffic is unchanged |
| 4 — unauthenticated reboot-to-bootloader | **fixed** (`"FW!KC79"`) |
| 5 — ID receive buffer never cleared | **fixed** |
| §7 — the bootloader | **superseded**; see the note at §7 and `Protocol.md` §10 |

The stage plan and bench procedures below remain the right way to land the rest.

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
| Router firmware upload | `Router/src/Modules/Hardware/FWUpdate.cpp`, `MassFWUdpdate.cpp` |
| RS485 bootloader (out-of-repo, see §7) | `…Dropbox…/KC79 - SBAU/Engineering/STM32CubeWorkspace/BootloaderRS485/` |

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

A complication: the receive path is **streaming** — `COBSRWStream::available()`
decodes on the fly and `RS485::processIncoming()` starts parsing as soon as the
start of a packet is readable, so the app can be acting on a body while the tail
of the frame (where a CRC would live) is still on the wire. A trailing CRC can
therefore only gate side effects if the packet is **buffered to completion
first**.

Recommendation (agreed): append a **CRC-16/CCITT-FALSE** over each frame, and
add a *complete-packet gate* inside `COBSRWStream`: an incoming packet is not
exposed to the reader until its EOP delimiter has been seen and the CRC has
verified. The decoded ring buffer already exists (256 bytes,
`MSGPACK_COBSRWSTREAM_BUFFER_SIZE`), so this costs no new memory — only latency,
which at 115200 baud is ~87 µs/byte (< 5 ms even for a 50-byte frame, against a
300 ms response window). Crucially the user-side API (`readArraySize`,
`readInt`, …) is unchanged: callers still stream out of the buffer exactly as
today.

Alternatives considered and rejected:

- *Per-payload CRC inside message schemas* — only protects the payload it is
  attached to (a bit-flip in the target-address byte misroutes the whole frame
  undetected) and leaks CRC handling into every schema.
- *Parse-then-commit in handlers* — keeps streaming but requires every handler
  in `App.cpp` to stage values and apply them only after verification; one
  missed handler silently reintroduces the hole.

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
The interface's ACK indicators are kept and become *more* informative: instead
of lighting up for any frame from the target, they reflect actual
`{"ack": true/false}` results plus retry count ("delivered ok / delivered after
2 retries / failed").

Severity: **medium/high** (reliability), effort: **medium**.

### Finding 4 — Unauthenticated reboot-to-bootloader (robustness)

Any frame whose body is a `string5` starting `"FW"` triggers `NVIC_SystemReset()`
into the bootloader (`PortalFW/src/Modules/RS485.cpp`). With no CRC (Finding 2),
a corrupted frame that happens to decode as `"FW…"` can bounce a device mid-move.

Agreed fix: a full CRC is not needed for this specific issue — **lengthen the
magic word** (e.g. `"FW!KC79"`, 6–8 bytes) so an accidental match is
astronomically unlikely. One line on each end, landable independently of
Stage 2 (see Stage 1.5). Once the Stage 2 complete-packet gate lands, corrupted
frames are rejected before dispatch anyway, so this becomes belt-and-braces.

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
  bytes except the final 3". Verification happens at the **framing layer**
  (`COBSRWStream`), on the complete buffered packet, before any byte is exposed
  to the parser — see Stage 2.
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

### Stage 1.5 — lengthen the reboot magic word (Finding 4)

Files: `PortalFW/src/Modules/RS485.cpp` (`processCOBSPacket`) and the router
side that emits the announce frame.

- Replace the 2-byte `"FW"` check with a longer improbable token (e.g.
  `"FW!KC79"`) compared in full.
- Landable independently of Stage 2; one line on each end. Once Stage 2's
  complete-packet gate is verifying CRCs this is redundant-but-harmless.

> **Bootloader interaction (important):** the same `"FW"` broadcast serves two
> consumers — the *application* (reboots into the bootloader) and the
> *bootloader* (treats it as the firmware-announce that resets `writePosition`).
> The fielded bootloader (`BootloaderRS485`, see §7) parses the announce with a
> 3-byte buffer (`FWUpdateApp::processIncoming` — `readString5(…, allocatedSize
> = 3, …)`), so a 7-byte `"FW!KC79"` would be **rejected as a format error** by
> bootloaders already burned into devices, breaking remote update. Since the
> bootloader can only be replaced by physical flashing (or a BootloaderCoup-style
> app), the rollout must be:
>
> 1. The **application's** reboot word becomes `"FW!KC79"`.
> 2. The router's update sequence sends the *long* word first (reboots all
>    apps), then continues announcing with the legacy 2-byte `"FW"` for the
>    bootloader. Apps that are already in the bootloader don't see the long
>    word; new-firmware apps ignore the short one. `"ER"`/`"RU"` are only ever
>    parsed by the bootloader and stay 2 bytes.

### Stage 2 — frame CRC with a complete-packet gate (Findings 2 & 4)

Two halves. **TX** keeps a running CRC folded in as bytes are written (the
sender knows the CRC by the time it writes the trailing field, so transmit can
stay fully streaming). **RX** buffers each packet to completion inside
`COBSRWStream` and verifies the CRC *before exposing any byte to the reader* —
so all user-side code (`RS485.cpp`, `App.cpp` handlers) keeps its current
streaming style unchanged, and no handler can act on an unverified frame.

**Submodule `COBSRWStream` (`PortalFW/lib/msgpack-arduino/src/msgpack/COBSRWStream.*`):**

*TX (running CRC, as before):*

- Add `uint16_t runningCRC = 0xFFFF` to the `transmit` struct.
- In `write(uint8_t)`: fold **every** logical byte (including zeros) into
  `transmit.runningCRC` *before* the COBS zero-handling branch. (Every writer
  funnels through `write()`.)
- Reset `transmit.runningCRC = 0xFFFF` at the end of `writeEOP()`.
- Expose `getTxRunningCRC()`.

*RX (complete-packet gate):*

- Add a `bool gateOnCompletePacket` mode flag (default **off** = today's
  streaming behaviour, so old callers and the router-side use of this class are
  untouched) and a `bool verifyCRC` flag.
- When gating is on, `available()` / `isStartOfIncomingPacket()` report data
  only once `decodeIncoming()` has seen the packet's EOP zero (the existing
  `receive.incomingStreamIsAtStartOfNextPacket` state). Until then the packet
  accumulates in the existing 256-byte decoded ring buffer
  (`MSGPACK_COBSRWSTREAM_BUFFER_SIZE`) — no new memory.
- At EOP, if `verifyCRC` is set: compute CRC-16 over all decoded bytes except
  the final 3 (`0xcd hi lo`), compare with those bytes, and on mismatch drop
  the whole packet (skip to next) and raise a counter/flag the app can report.
  Frames shorter than 4 decoded bytes or not ending in `0xcd …` are treated as
  legacy (no CRC) and passed through — this is what keeps mixed fleets working.
- A packet larger than the ring buffer cannot be gated; on overflow, drop the
  packet and flag an error (current command frames are far below 256 bytes;
  revisit the buffer size if a larger command is ever added).

**Firmware `PortalFW/src/Modules/RS485.cpp`:**

- Enable `gateOnCompletePacket` on the RX stream; `verifyCRC` stays behind the
  existing flag. **No changes to the parsing code** in `processCOBSPacket` /
  `App::processIncoming` beyond the two below:
- Add a `finishFrame(uint8_t seq)` helper: write `seq` (`writeIntU8`), snapshot
  `getTxRunningCRC()`, write it (`writeIntU16`), then `endTransmission()`.
- Change the three senders (`sendStatusReport`, `sendPositions`, `sendACK`) to
  declare array size **5** and call `finishFrame(this->lastRxSeq)` instead of
  `endTransmission()`. Change `sendACK`'s body to the map `{"ack": success}`.
- In `processCOBSPacket`: keep `arraySize` in scope; after the body is consumed,
  if `arraySize >= 4` read `seq` into `lastRxSeq`. (The CRC element needs no
  handling here — it was already verified and can be left for
  `nextIncomingPacket()` to discard.)
- Add members `uint8_t lastRxSeq = 0;` and `bool verifyCRC = false;`.
- The `"FW"` reboot needs no special deferral: with the gate on, a frame only
  reaches dispatch after its CRC has verified.

> Latency note: gating delays parsing until the full frame has arrived —
> ~87 µs/byte at 115200, i.e. < 5 ms for typical frames, negligible against the
> 300 ms response window.

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

**Observe:** the device must **not** move to a wrong position; the frame is
dropped at the framing layer (a corrupted frame's target address cannot be
trusted, so **no ACK is sent** — the Router sees a timeout, and with Stage 3 on,
a retransmission). The firmware should raise its CRC-error counter/flag. For a
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
2. Force a NACK/timeout: send a deliberately malformed body (firmware replies
   `{"ack":false}`) and, separately, a bit-flipped frame (dropped at the framing
   layer → timeout, no reply). Confirm the Router retransmits up to
   `maxRetries` in both cases, then logs failure.
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

### Test 6 — Remote firmware update regression (1 device)

**Goal:** confirm the RS485 bootloader update path survives every hardening
stage — especially the Stage 1.5 magic-word change and Stage 2 extra elements.

**Physical setup:** one board carrying the fielded `BootloaderRS485` (do **not**
reflash the bootloader — the point is compatibility with what's in the field),
app firmware at the stage under test.

**Steps:**
1. From the Router, run a full FW update cycle (announce → erase → upload →
   run). With Stage 1.5 firmware, confirm the long-word reboot + legacy `"FW"`
   announce sequence gets the device into and through the update.
2. Repeat with `Append seq+CRC` **on**: the bootloader reads elements 0–2 and
   never touches the trailing seq/CRC, so upload frames must still be accepted.
   (Upload frames are broadcast, so the router must not wait for ACKs on them —
   unchanged behaviour.)
3. Confirm the updated app boots and responds normally afterwards.

**Pass:** update completes and the device runs the new app at every stage.
**Fail** at step 2 likely means an upload frame overflowed the bootloader's
64-byte decoded buffer — reduce the router's upload frame size (it streams, so
this is only a risk if a gate is ever added bootloader-side).

---

## 6. Suggested landing order

1. Stage 1 (ID fix) — land and flash fleet-wide.
1.5. Stage 1.5 (longer magic word) — land alongside Stage 1 (router and firmware
   must update together, or the router keeps sending the old word).
2. Stage 2 firmware + router in **emit-only** mode (verify off) — Test 1, Test 5.
3. Enable `Verify RX CRC` once Test 1 passes fleet-wide — Test 2.
4. Stage 3 + enable `Strict ACK + retransmit` — Test 3.
5. Bootloader import into this repo (§7) — can proceed in parallel with 2–4;
   run Test 6 after each protocol-affecting stage regardless.

Remember the submodule two-step for any `COBSRWStream` change: commit in
`msgpack-arduino`, then bump the submodule pointer in the root repo.

---

## 7. The RS485 bootloader

> **Superseded for v6.** Everything below describes the fielded v4/v5 bootloader and the plan for
> importing it, which happened. The bootloader was then rewritten: it no longer erases the durable
> pages, it answers, it accepts frames in any order, and it is 16 kB rather than 24 kB. The wire
> protocol it speaks is documented in [`Protocol.md` §10](./Protocol.md#10-firmware-update--bootloader-protocol),
> which is the contract; this section is kept because a fleet still contains v4/v5 boards and
> because §7.4's findings are what the rewrite was built on.
>
> Two specific corrections to what follows. The budget in §7.3 ("1,868 bytes; the current image is
> 22,708 of 24,576") is now 16,384 with the image at 14,796. And §7.1's conclusion that the
> protocol is "effectively frozen" was right about v4/v5 and is what the compatibility mode in
> §10.1 preserves — but replacing a bootloader no longer requires a debug probe, because the
> application can now install one (§10.5).

### 7.1 Which bootloader is in use

The `STM32CubeWorkspace` folder (Dropbox, `KC79 - SBAU/Engineering`) contains
three bootloader-ish projects. **`BootloaderRS485` is the one compatible with
the current protocol** — verified by matching its parser against the Router:

| Protocol feature | Router (`FWUpdate.cpp` / `MassFWUdpdate.cpp`) | `BootloaderRS485` (`FWUpdateApp.cpp`, `RS485.cpp`) |
| --- | --- | --- |
| Frame | 3-element array `[-1, 0, body]`, broadcast only | requires `arraySize >= 3`, only accepts target `-1` |
| Magic words | `"FW"` announce, `"ER"` erase, `"RU"` run | `"FW"` / `"ER"` / `"RU"` |
| Upload body | map `{frameOffset → bin(checksum ‖ data)}` | same, checksum-first |
| Packet checksum | XOR of 16-bit words (`Utils::calcCheckSum`) | identical `calcCheckSum` |
| Memory map | app flashed with `board_upload.offset_address = 0x08006000` | `BOOTLOADER_SIZE = 24 kB`, `APP_FLASH_ADDRESS = 0x08006000` |

The other two projects:

- **`BootloaderCoup`** — not a bootloader but an *application* that carries a
  bin2c-embedded bootloader image (compiled 2023-07-06) and writes it to
  sector 0: the mechanism for replacing the bootloader on fielded devices over
  RS485 (upload the "coup" app via the existing bootloader, run it once). Keep;
  it is the only remote path for ever changing the fielded bootloader.
- **`Bootloader2`** — minimal CubeMX skeleton (bare `main.c`, no COBS/msgpack);
  an earlier iteration, not protocol-compatible. Ignore.

Two properties of `BootloaderRS485` matter for this plan:

- It carries a **snapshot copy** of msgpack-arduino (`Core/msgpack-arduino/`),
  not the submodule — and it has already drifted: its `COBSRWStream` is an
  older double-buffer implementation with a **64-byte** decoded buffer, vs. the
  submodule's lwrb ring buffer at 256 bytes. Any Stage 2 change to the
  submodule does *not* reach the bootloader until this is unified.
- It is burned into fielded devices and only replaceable via ST-Link or the
  BootloaderCoup path — so the protocol it speaks (2-byte magic words, XOR16
  packet checksums, no ACKs) is effectively **frozen** and everything above
  must stay compatible with it (see Stage 1.5 note and Test 6).

### 7.2 Proposal — bring the bootloader into this repo under PlatformIO

Currently the bootloader lives outside version control (Dropbox) as an
STM32CubeIDE project. Proposal: import it as a sibling PlatformIO project,
`PortalBootloader/`, so bootloader, firmware, and router evolve in one history.

**Layout:**

```
PortalBootloader/
  platformio.ini
  STM32G070RBTX_FLASH.ld      # from the CubeIDE project, FLASH LENGTH → 24K
  BootloaderRS485.ioc         # keep for future CubeMX regeneration
  src/                        # Core/Src — main.cpp, FWUpdateApp, RS485, flash, …
  include/                    # Core/Inc
```

**`platformio.ini`:**

```ini
[env:bootloader]
platform = ststm32
board = nucleo_g070rb
board_build.mcu = stm32g070rbt6
framework = stm32cube            ; the bootloader is HAL-based, NOT Arduino
board_build.ldscript = STM32G070RBTX_FLASH.ld
upload_protocol = stlink
build_flags =
    -Os
    -D MSGPACK_COBSRWSTREAM_BUFFER_SIZE=64   ; preserve current RAM footprint
lib_deps = msgpack-arduino
lib_extra_dirs = ../PortalFW/lib             ; reuse the existing submodule
extra_scripts = post:check_size.py           ; assert .bin ≤ 24 kB (see below)
```

Key points, in order of importance:

1. **Delete the msgpack snapshot; use the submodule.** The library already
   supports non-Arduino targets (`NotArduino.cpp` / `Platform.hpp` — the
   snapshot itself uses that path), so `lib_extra_dirs = ../PortalFW/lib`
   points PlatformIO at the same checkout PortalFW builds against. One source
   of truth: a Stage 2 `COBSRWStream` change lands once and both images get it.
   The snapshot's drift (old double-buffer stream) must be reconciled first —
   build against the submodule and re-run Test 6 before trusting it.
2. **Linker script is the contract.** Copy `STM32G070RBTX_FLASH.ld` and set
   `FLASH LENGTH = 24K`. Add a `post:` script that fails the build if the
   `.bin` exceeds `24 * 1024` bytes — today CubeIDE size limits are implicit;
   in-repo they should be enforced.
3. **`framework = stm32cube`, not `arduino`.** The bootloader must stay small
   and must not inherit the Arduino core's startup/IWDG behaviour. PlatformIO
   supports mixed frameworks per-env, so it coexists fine next to `PortalFW`.
4. **Keep the `.ioc`** so peripheral config can be regenerated with CubeMX if
   pins ever change; generated HAL code lives in `src/` like any other source.
5. **Bit-for-bit verification before switching.** Build the last CubeIDE
   Release once more, then the PlatformIO build with matching flags, and
   compare `arm-none-eabi-objdump`/size output (identical `-Os` GCC major
   versions should be near-identical; at minimum, run Tests 1–6 against the
   PlatformIO-built bootloader on the bench before it becomes canonical).
6. **CI symmetry.** Whatever builds `PortalFW` (the two-env matrix) also builds
   `PortalBootloader` so protocol changes that break the bootloader are caught
   at compile time, not on the bench.
7. **Archive, don't fork.** Once imported and verified, mark the Dropbox
   project read-only/renamed (`_ARCHIVED_see_KC79.Reworld`) so fixes can't land
   in the wrong copy. Import `BootloaderCoup` the same way later if a
   bootloader replacement campaign is ever needed (it would then embed the
   PlatformIO-built `.bin` via bin2c as part of its build).

### 7.3 Corrections to §7.2, found by reading the CubeIDE project

Four things in the proposal above are wrong or unattainable. They were found while
planning the import, before it was attempted.

1. **`STM32G070RBTX_FLASH.ld` line 48 already says `LENGTH = 28K`, not 24K.**
   The bank is 24 kB. A bootloader between 24,576 and 28,672 bytes therefore
   links cleanly today and silently overlaps the application at `0x08006000`.
   (The CubeIDE *Debug* build is 36,988 bytes — it cannot coexist with an
   application at all.) Tightening `LENGTH` moves no bytes, so it does not
   affect the comparison in point 5.
2. **`-D MSGPACK_COBSRWSTREAM_BUFFER_SIZE=64` is inert.** That macro is an
   unconditional `#define … 256` at `COBSRWStream.hpp:4`, not `#ifndef`-guarded,
   so a command-line `-D` warns and loses. Accept 256 (≈264 B more `.bss` out of
   36 kB, and it raises the real maximum frame body from ~49 to ~240 bytes), or
   do the submodule two-step to add a guard.
3. **`board_build.stm32cube.custom_config_header = yes` is required**, or
   PlatformIO installs its own `stm32g0xx_hal_conf.h`, which enables far more HAL
   modules than this project's ten and grows `.text` past the budget. **The
   budget is 1,868 bytes**: the current image is 22,708 of 24,576.
4. **Bit-for-bit comparison (point 5) is unattainable.** CubeIDE built with GCC
   10.3-2021.10 and CMSIS 1.4.3; `ststm32@19.6.0` offers 6.3.1/8.2.1/9.2.1/12.3.1
   only — there is no 10.3. Read point 5 as *behavioural equivalence,
   structurally diffed*: section table, `.isr_vector` byte-compare, symbol-name
   sets, `size`, string table, and disassembly of `flash_erase` / `flash_write` /
   `run_application` / `FWUpdateApp::processIncoming`.

Also: the msgpack snapshot has drifted **less** than §7.1 implies. A full `diff -r`
shows `deserialize.*`, `serialize.*`, `Serializer.*`, `NotArduino.*`, `constants.h`
and `Platform.hpp` are byte-identical — the whole protocol parse path is unchanged.
Only `COBSRWStream` (64-byte double buffer → 256-byte lwrb ring), `Messaging`
(virtual + `MSGPACK_DISABLE_ERROR_REPORT`) and an additive `isInt()` differ.

### 7.4 The "16-bit address space limits file size" question, answered

**There is no 16-bit truncation of `frameOffset` anywhere in the path.** Every hop
was traced and every one is 32-bit: `dump_uint` (Rust) and `msgpack_pack_uint32`
(C++) both emit `0xCE` above 65535; `getNextDataType` maps it; `readInt<uint32_t>`
is explicitly instantiated and identical in the snapshot and the submodule;
`FWUpdateApp::writePosition` is `uint32_t`; `flash_write` takes `uint32_t`; and
`flash_erase` derives `NbPages` from the device's `FLASH_SIZE` register, giving
`Page = 12, NbPages = 52` — the full 104 kB bank.

This is now gated rather than argued. `PortalBootloader/test-native/` compiles the
**real** msgpack sources on the non-Arduino path the bootloader uses and replays
`FWUpdateApp::processIncoming`'s exact parse sequence over a real `COBSRWStream`
for all **3,328** frame offsets of a completely full image, plus every msgpack
width boundary, plus a deliberately 16-bit-narrowed key that must read back
truncated (which is what keeps the other assertions from being vacuous).
`RouterRS`'s `fw::tests::fw_frame_offset_spans_full_application_bank` pins the
host's wire bytes for offset 106,464 as `CE 00 01 9F E0`. Run:

```powershell
powershell -File PortalBootloader\test-native\run.ps1
cd RouterRS; cargo test -p router-proto fw::
```

**What the real constraints turned out to be:**

- **`Router/src/Modules/Hardware/FWUpdate.cpp` was wired up wrong** — and this is
  the most plausible origin of the folklore. It advanced `dataPosition` and
  `frameOffset` by the GUI parameter `frameSize` but passed the hardcoded
  `FW_FRAME_SIZE` (32) as the payload length. Left at the default the two agree;
  raise "Frame size" — the obvious thing to try when a large image uploads
  slowly — and every frame carries 32 bytes while the offset strides by N, so the
  bootloader's continuity check fires ("FW : Write position is ahead of ours") and
  the upload dies. It fails loudly rather than corrupting, but the failure has
  nothing to do with file size and only shows up on large images, because that is
  when anyone touches the setting. **Fixed.** `RouterRS`'s
  `fw_update.rs:90` never had this bug.
- **The COBS decode buffer, not the address, caps the frame size.** With the
  fielded 64-byte buffer the usable body is ~49 bytes. `constants.h:7`'s
  `FW_FRAME_SIZE 128` is **dead code** — nothing reads it — and is an invitation
  to set the host to 128 and hit both problems at once. Delete it at import.
- **`flash_write` rounds up to double-words** (`flash.cpp:65-87`). A final frame
  whose length is not a multiple of 8 reads past the buffer into uninitialised
  stack and programs it, so the image tail is non-deterministic — and the "Truncate
  trailing 0xFF" option makes the final length arbitrary. Pad the tail to 8 bytes
  with `0xFF`. This changes fielded behaviour, so it needs its own bench pass.
- **`FWUpdateApp.cpp:110` allocates a VLA sized from a 16-bit wire field before
  validating anything**, on a 1 kB stack. A corrupt `bin16` header claiming 60,000
  bytes smashes the stack of a bootloader that can only be replaced by ST-Link.
  An `if (size > 250) return MessageFormatError();` costs ~8 bytes of flash.

The remaining acceptance test is still on the bench, because only it covers the
fielded 64-byte buffer and the real flash: upload a deliberately >64 kB
application over RS485, then read the bank back over SWD and byte-compare, with a
<64 kB image as the negative control.

#ifdef ARDUINO

#include <Arduino.h>
#include <HardwareSerial.h>
#include <atomic>
#include <driver/gpio.h>
#include <esp_core_dump.h>
#include <esp_mac.h>
#include <esp_system.h>

#include "BridgeCore.h"
#include "ControlPlane.h"
#include "EspOtaTarget.h"
#include "OtaSession.h"
#include "Persistence.h"
#include "SnapshotEngine.h"
#include "Wire.h"

#ifndef REPEATER_BUILD_ID
#define REPEATER_BUILD_ID "unknown"
#endif

#ifndef REPEATER_SIDE1_INVERT
#define REPEATER_SIDE1_INVERT 0
#endif

#ifndef REPEATER_SIDE2_INVERT
#define REPEATER_SIDE2_INVERT 0
#endif

namespace {

constexpr uint8_t PIN_MAX1_RO = 20;
constexpr uint8_t PIN_MAX1_DI = 21;
constexpr uint8_t PIN_MAX1_DERE = 7;
constexpr uint8_t PIN_MAX2_RO = 6;
constexpr uint8_t PIN_MAX2_DI = 4;
constexpr uint8_t PIN_MAX2_DERE = 5;
constexpr uint8_t PIN_LED = 10;

constexpr uint32_t RS485_BAUD = 115200;

// The frame idle timeout lives in BridgeCore.h, with the measurement that set it.
constexpr size_t RX_BUFFER_BYTES = 12288;
constexpr size_t TX_BUFFER_BYTES = 4096;
constexpr size_t COPY_CHUNK_BYTES = 128;
constexpr uint32_t HEARTBEAT_HALF_PERIOD_MS = 500;
constexpr uint32_t DATA_LED_HOLD_MS = 100;
constexpr bool SIDE1_INVERTED = REPEATER_SIDE1_INVERT != 0;
constexpr bool SIDE2_INVERTED = REPEATER_SIDE2_INVERT != 0;

/// How long the application must run before it is considered to have started
/// successfully, clearing the unhealthy-boot counter.
constexpr uint32_t HEALTHY_UPTIME_MS = 30000;

/// Identity and range bookkeeping is slow-moving; there is no reason to touch it
/// on every pass of a loop that also services two UARTs.
constexpr uint32_t IDENTITY_SERVICE_PERIOD_MS = 1000;

repeater::FrameRouter router(repeater::FRAME_IDLE_TIMEOUT_US);
std::atomic<uint32_t> uartErrors[2] = {0, 0};
std::atomic<uint32_t> turnaroundEvents[2] = {0, 0};
std::atomic<bool> transmitting[2] = {false, false};
uint32_t lastActivityMs = 0;
uint32_t lastHeartbeatMs = 0;
bool heartbeatState = false;
bool bootRecordPending = true;
String commandLine;

// millis() wraps every 49.7 days and this installation runs for months, so uptime
// is accumulated into 64 bits rather than reported raw.
uint64_t uptimeBaseMs = 0;
uint32_t lastUptimeSampleMs = 0;
uint32_t lastIdentityServiceMs = 0;
bool healthyRecorded = false;

int8_t repeaterIndex = repeater::persistence::INDEX_UNSET;
uint8_t persistedRangeStart = 0;
esp_reset_reason_t bootResetReason = ESP_RST_UNKNOWN;
bool coreDumpPresent = false;
uint8_t macAddress[repeater::MAC_BYTES] = {};

repeater::ControlPlane controlPlane;
repeater::EspOtaTarget otaTarget;
repeater::OtaSession ota(otaTarget);
repeater::SnapshotEngine snapshot;

/// How many of the stored snapshot replies have been queued toward the host during
/// a `snap-read`. The originate queue is only four deep, so nine frames plus the
/// completion have to be fed in over several loop passes.
uint8_t snapshotReadCursor = 0;
bool snapshotReadActive = false;

/// An image that has been made bootable but not yet rebooted into.
bool otaRebootPending = false;

/// True while the running image still has to prove itself. Resolved locally: the
/// host may accelerate it with `ota-confirm`, but must never be required to.
bool otaPendingVerify = false;
bool otaVerifyResolved = false;

/// How many `ota-data` frames reached the dispatcher, how many of those the payload
/// parser rejected, and the payload size of the most recent one. `ota-data` is the one
/// verb that never answers, so without these a lost chunk is indistinguishable from a
/// chunk that arrived and was thrown away.
uint32_t otaDataFrames = 0;
uint32_t otaDataParseFailures = 0;
uint32_t otaDataPayloadBytes = 0;

/// Where a control frame stopped: seen by the control plane at all, rejected as
/// unparseable, or parsed but addressed elsewhere. Together with `data_frames` these
/// pin a missing chunk to one hop.
uint32_t controlFramesSeen = 0;
uint32_t controlFramesInvalid = 0;
uint32_t controlFramesNotOurs = 0;
uint32_t controlLastFrameBytes = 0;

/// Set by a control-plane `reboot`, acted on from the service loop. Rebooting
/// inside frame ingest would strand a half-parsed frame and the reply that has
/// not been transmitted yet.
uint32_t rebootAtMs = 0;

/// Bumped whenever the repeater changes state in a way the host would want to know
/// about between polls, so a host that only samples occasionally can tell it missed
/// something rather than silently seeing a steady picture.
uint32_t eventSeq = 0;

/// `tx-test` state. Frames are emitted one per loop pass rather than queued in a burst,
/// so a long run cannot overflow the four-slot originate queue and report drops that are
/// an artefact of the test rather than of the bus.
uint32_t txTestRemaining = 0;
repeater::Side txTestSide = repeater::Side::None;
uint8_t txTestSequence = 0;
int8_t txTestTarget = 0;
uint32_t txTestLastMs = 0;
repeater::BlockState lastReportedBlockState = repeater::BlockState::Unknown;

uint64_t uptimeMs() {
    const uint32_t now = millis();
    uptimeBaseMs += static_cast<uint32_t>(now - lastUptimeSampleMs);
    lastUptimeSampleMs = now;
    return uptimeBaseMs;
}

const char* resetReasonName(esp_reset_reason_t reason) {
    switch(reason) {
    case ESP_RST_POWERON: return "poweron";
    case ESP_RST_EXT: return "external";
    case ESP_RST_SW: return "software";
    case ESP_RST_PANIC: return "panic";
    case ESP_RST_INT_WDT: return "int_wdt";
    case ESP_RST_TASK_WDT: return "task_wdt";
    case ESP_RST_WDT: return "wdt";
    case ESP_RST_DEEPSLEEP: return "deepsleep";
    case ESP_RST_BROWNOUT: return "brownout";
    case ESP_RST_SDIO: return "sdio";
    default: return "unknown";
    }
}

// Keyed on the destination rather than the source: a locally originated frame
// (a branch poll, a control-plane reply) has no source side at all.
HardwareSerial& uartFor(repeater::Side destination) {
    return destination == repeater::Side::One ? Serial0 : Serial1;
}

void setTransmitting(repeater::Side destination, bool enabled) {
    const bool isTwo = destination == repeater::Side::Two;
    const uint8_t index = isTwo ? 1 : 0;
    const uint8_t derePin = isTwo ? PIN_MAX2_DERE : PIN_MAX1_DERE;
    if(enabled) {
        transmitting[index].store(true);
    }
    digitalWrite(derePin, enabled ? HIGH : LOW);
    if(!enabled) {
        transmitting[index].store(false);
    }
}

void printStatus(const char* type) {
    if(!Serial || Serial.availableForWrite() < 64) {
        return;
    }
    const auto& stats = router.stats();
    Serial.printf(
        "{\"type\":\"%s\",\"version\":\"%s\",\"build\":\"%s\","
        "\"baud\":%lu,\"routing\":{\"mode\":\"%s\",\"range_start\":%u,"
        "\"range_end\":%u,\"relayed_control\":%llu,"
        "\"filtered_host_frames\":%llu,\"parse_errors\":%llu},"
        "\"side1\":{\"inverted\":%s,\"rx_bytes\":%llu,\"received_frames\":%llu,"
        "\"forwarded_bytes\":%llu,\"frames\":%llu,\"incomplete\":%llu,"
        "\"oversized\":%llu,\"queue_depth\":%u,\"queue_high_water\":%u,\"queue_drops\":%llu,"
        "\"uart_errors\":%lu,\"turnaround_events\":%lu},"
        "\"side2\":{\"inverted\":%s,\"rx_bytes\":%llu,\"received_frames\":%llu,"
        "\"forwarded_bytes\":%llu,\"frames\":%llu,\"incomplete\":%llu,"
        "\"oversized\":%llu,\"queue_depth\":%u,\"queue_high_water\":%u,\"queue_drops\":%llu,"
        "\"uart_errors\":%lu,\"turnaround_events\":%lu},"
        "\"tx_errors\":%llu,\"consumed_inner\":%llu,\"originated\":%llu,"
        "\"originate_drops\":%llu,\"last_activity_ms\":%lu,"
        "\"index\":%d,\"mac\":\"%02x:%02x:%02x:%02x:%02x:%02x\",\"event_seq\":%lu,"
        "\"health\":{\"reset_reason\":\"%s\",\"boots\":%lu,\"unhealthy_boots\":%lu,"
        "\"min_free_heap\":%lu,\"uptime_ms\":%llu,\"core_dump\":%s}}\n",
        type,
        REPEATER_VERSION,
        REPEATER_BUILD_ID,
        static_cast<unsigned long>(RS485_BAUD),
        repeater::blockStateName(router.blockState()),
        static_cast<unsigned>(router.localRangeStart()),
        static_cast<unsigned>(router.localRangeEnd()),
        stats.relayedControlFrames,
        stats.filteredHostFrames,
        stats.parseErrors,
        SIDE1_INVERTED ? "true" : "false",
        stats.oneToTwo.rxBytes,
        stats.oneToTwo.receivedFrames,
        stats.oneToTwo.forwardedBytes,
        stats.oneToTwo.forwardedFrames,
        stats.oneToTwo.incompleteFrames,
        stats.oneToTwo.oversizedFrames,
        static_cast<unsigned>(router.queueDepth(repeater::Side::One)),
        static_cast<unsigned>(stats.oneToTwo.queueHighWater),
        stats.oneToTwo.queueDrops,
        static_cast<unsigned long>(uartErrors[0].load()),
        static_cast<unsigned long>(turnaroundEvents[0].load()),
        SIDE2_INVERTED ? "true" : "false",
        stats.twoToOne.rxBytes,
        stats.twoToOne.receivedFrames,
        stats.twoToOne.forwardedBytes,
        stats.twoToOne.forwardedFrames,
        stats.twoToOne.incompleteFrames,
        stats.twoToOne.oversizedFrames,
        static_cast<unsigned>(router.queueDepth(repeater::Side::Two)),
        static_cast<unsigned>(stats.twoToOne.queueHighWater),
        stats.twoToOne.queueDrops,
        static_cast<unsigned long>(uartErrors[1].load()),
        static_cast<unsigned long>(turnaroundEvents[1].load()),
        stats.txErrors,
        stats.consumedInnerFrames,
        stats.originatedFrames,
        stats.originateDrops,
        static_cast<unsigned long>(millis() - lastActivityMs),
        static_cast<int>(repeaterIndex),
        macAddress[0], macAddress[1], macAddress[2],
        macAddress[3], macAddress[4], macAddress[5],
        static_cast<unsigned long>(eventSeq),
        resetReasonName(bootResetReason),
        static_cast<unsigned long>(repeater::persistence::bootCount()),
        static_cast<unsigned long>(repeater::persistence::unhealthyBoots()),
        static_cast<unsigned long>(ESP.getMinFreeHeap()),
        uptimeMs(),
        coreDumpPresent ? "true" : "false");
}

/// The wire form of everything the USB `status` command reports. Keys are short
/// because this crosses a 115200 bus; the host-side decoder names them.
void writeDirectionStats(repeater::wire::MsgpackWriter& out, const repeater::DirectionStats& dir,
    size_t queueDepth, uint32_t uartErrorCount, uint32_t turnaroundCount) {
    out.mapHeader(11);
    out.key("rx"); out.uinteger(dir.rxBytes);
    out.key("rf"); out.uinteger(dir.receivedFrames);
    out.key("fb"); out.uinteger(dir.forwardedBytes);
    out.key("ff"); out.uinteger(dir.forwardedFrames);
    out.key("inc"); out.uinteger(dir.incompleteFrames);
    out.key("ovr"); out.uinteger(dir.oversizedFrames);
    out.key("qd"); out.uinteger(queueDepth);
    out.key("qhw"); out.uinteger(dir.queueHighWater);
    out.key("qdr"); out.uinteger(dir.queueDrops);
    out.key("ue"); out.uinteger(uartErrorCount);
    out.key("te"); out.uinteger(turnaroundCount);
}

void writeStatusPayload(repeater::wire::MsgpackWriter& out) {
    const auto& stats = router.stats();

    out.mapHeader(13);
    out.key("proto"); out.uinteger(repeater::CONTROL_PROTO_VERSION);
    out.key("ver"); out.string(REPEATER_VERSION);
    out.key("build"); out.string(REPEATER_BUILD_ID);
    out.key("mac"); out.binary(macAddress, sizeof(macAddress));
    out.key("idx"); out.integer(repeaterIndex);
    out.key("mode"); out.string(repeater::blockStateName(router.blockState()));
    out.key("range");
    out.arrayHeader(2);
    out.uinteger(router.localRangeStart());
    out.uinteger(router.localRangeEnd());

    out.key("s1");
    writeDirectionStats(out, stats.oneToTwo, router.queueDepth(repeater::Side::One),
        uartErrors[0].load(), turnaroundEvents[0].load());
    out.key("s2");
    writeDirectionStats(out, stats.twoToOne, router.queueDepth(repeater::Side::Two),
        uartErrors[1].load(), turnaroundEvents[1].load());

    out.key("flt");
    out.mapHeader(3);
    out.key("relay"); out.uinteger(stats.relayedControlFrames);
    out.key("host"); out.uinteger(stats.filteredHostFrames);
    out.key("pe"); out.uinteger(stats.parseErrors);

    out.key("plane");
    out.mapHeader(5);
    out.key("tx"); out.uinteger(stats.txErrors);
    out.key("ctl"); out.uinteger(stats.controlFrames);
    out.key("cons"); out.uinteger(stats.consumedInnerFrames);
    out.key("org"); out.uinteger(stats.originatedFrames);
    out.key("odr"); out.uinteger(stats.originateDrops);

    out.key("ev"); out.uinteger(eventSeq);

    out.key("health");
    out.mapHeader(6);
    out.key("rst"); out.string(resetReasonName(bootResetReason));
    out.key("boots"); out.uinteger(repeater::persistence::bootCount());
    out.key("unhealthy"); out.uinteger(repeater::persistence::unhealthyBoots());
    out.key("heap"); out.uinteger(ESP.getMinFreeHeap());
    out.key("up"); out.uinteger(uptimeMs());
    out.key("cd"); out.boolean(coreDumpPresent);
}

void scheduleReboot() {
    rebootAtMs = millis() + 250;
    if(rebootAtMs == 0) rebootAtMs = 1; // 0 is the "not scheduled" sentinel
}

/// `v` for `ota-begin`: `{"size":n,"chunk":n,"session":n,"sha":bin(32)}`.
bool parseOtaBegin(const repeater::ControlRequest& request, repeater::OtaBeginRequest& out) {
    if(request.payload == nullptr) return false;
    repeater::wire::MsgpackCursor cursor(request.payload, request.payloadSize);
    uint32_t fields = 0;
    if(!cursor.readMapSize(fields)) return false;

    bool haveSize = false;
    bool haveChunk = false;
    bool haveSha = false;
    for(uint32_t i = 0; i < fields; ++i) {
        const uint8_t* key = nullptr;
        uint32_t keyLength = 0;
        if(!cursor.readString(key, keyLength)) return false;
        int64_t value = 0;
        if(repeater::wire::stringEquals(key, keyLength, "size")) {
            if(!cursor.readInteger(value) || value <= 0) return false;
            out.imageSize = static_cast<uint32_t>(value);
            haveSize = true;
        }
        else if(repeater::wire::stringEquals(key, keyLength, "chunk")) {
            if(!cursor.readInteger(value) || value <= 0) return false;
            out.chunkBytes = static_cast<uint32_t>(value);
            haveChunk = true;
        }
        else if(repeater::wire::stringEquals(key, keyLength, "session")) {
            if(!cursor.readInteger(value)) return false;
            out.session = static_cast<uint8_t>(value);
        }
        else if(repeater::wire::stringEquals(key, keyLength, "sha")) {
            const uint8_t* sha = nullptr;
            uint32_t shaLength = 0;
            if(!cursor.readBinary(sha, shaLength) || shaLength != repeater::OTA_SHA_BYTES) return false;
            memcpy(out.sha256, sha, repeater::OTA_SHA_BYTES);
            haveSha = true;
        }
        else if(!cursor.skipValue()) return false;
    }
    return haveSize && haveChunk && haveSha;
}

/// `v` for `ota-data`: `[session, index, bin(data), crc16]`. An array rather than a
/// map because this is the only verb sent hundreds of times per update.
bool parseOtaData(const repeater::ControlRequest& request, uint8_t& session, uint32_t& index,
    const uint8_t*& data, uint32_t& size, uint16_t& crc) {
    if(request.payload == nullptr) return false;
    repeater::wire::MsgpackCursor cursor(request.payload, request.payloadSize);
    uint32_t count = 0;
    if(!cursor.readArraySize(count) || count < 4) return false;

    int64_t value = 0;
    if(!cursor.readInteger(value) || value < 0 || value > 255) return false;
    session = static_cast<uint8_t>(value);
    if(!cursor.readInteger(value) || value < 0) return false;
    index = static_cast<uint32_t>(value);
    if(!cursor.readBinary(data, size)) return false;
    if(!cursor.readInteger(value) || value < 0 || value > 0xFFFF) return false;
    crc = static_cast<uint16_t>(value);
    return true;
}

/// Put a known frame on one bus, from the repeater itself.
///
/// Commissioning has a blind spot that costs hours: every other check here is driven by
/// frames arriving from the host, so when nothing arrives there is no way to separate a
/// host adapter that never drives the line from a differential pair that is not landing
/// on this transceiver. Both present as silence with zero UART errors. Driving the line
/// from the middle of the topology splits them in one command -- if the host hears this
/// frame, its receive path and the wiring are good and only its transmitter is suspect.
///
/// The frame is envelope target 0. That is the one class a Portal ignores (it is neither
/// its own ID nor the -1 broadcast) and the one class every repeater refuses to relay, in
/// every routing mode, so it is inert on either bus and cannot leak to a neighbour.
///
/// With a `target` of 0 the frame is that inert marker. Given a Portal ID instead, it is a
/// real `{"poll": nil}` addressed to that one board -- which turns the same command into
/// the other half of the diagnosis: if the branch answers, side 2's transceiver, wiring
/// and receive path are all proven at once, and the fault is confined to side 1. Unicast
/// rather than broadcast, so nine boards do not answer into each other.
void sendTestFrame(repeater::Side destination, uint8_t sequence, int8_t target) {
    uint8_t body[16];
    repeater::wire::MsgpackWriter out(body, sizeof(body));
    out.arrayHeader(3);
    out.integer(target);
    out.integer(target == 0 ? repeaterIndex : 0);
    out.mapHeader(1);
    if(target == 0) {
        out.key("tx");
        out.uinteger(sequence);
    }
    else {
        out.key("poll");
        out.nil();
    }
    if(!out.ok()) return;

    uint8_t framed[repeater::MAX_ORIGINATED_FRAME_BYTES];
    const size_t size =
        repeater::wire::cobsEncodeFrame(out.data(), out.size(), framed, sizeof(framed));
    if(size == 0) return;
    router.originate(destination, framed, size);
}

void serviceTxTest() {
    if(txTestRemaining == 0 || txTestSide == repeater::Side::None) return;
    // Wait for the previous one to actually leave, so the count the operator asked for is
    // the count that reached the wire.
    if(router.originateDepth(txTestSide) > 0) return;
    // A poll needs an answering gap; the inert marker does not.
    if(txTestTarget != 0 && millis() - txTestLastMs < 200) return;
    sendTestFrame(txTestSide, txTestSequence++, txTestTarget);
    txTestRemaining--;
    txTestLastMs = millis();
    lastActivityMs = millis();
}

/// Frames whatever `beginReply` started and queues it toward the host bus.
void sendControlReply() {
    uint8_t framed[repeater::MAX_ORIGINATED_FRAME_BYTES];
    const size_t size = controlPlane.finishReply(framed, sizeof(framed));
    if(size == 0) return;
    router.originate(repeater::Side::One, framed, size);
}

/// Every OTA control reply carries the same picture of where the session is, so a
/// host that missed an earlier reply can always recover the state from the next one.
void replyWithOtaState(repeater::ControlVerb verb, repeater::OtaResult result) {
    auto& out = controlPlane.beginReply(verb, result == repeater::OtaResult::Ok, true);
    out.mapHeader(7);
    out.key("result"); out.string(repeater::otaResultName(result));
    out.key("state"); out.string(repeater::otaStateName(ota.state()));
    out.key("session"); out.uinteger(ota.session());
    out.key("chunks"); out.uinteger(ota.chunkCount());
    out.key("got"); out.uinteger(ota.receivedChunks());
    out.key("slot"); out.string(repeater::EspOtaTarget::runningLabel());
    out.key("pending"); out.boolean(otaRebootPending);
    sendControlReply();
}

void handleControlRequest(const repeater::ControlRequest& request) {
    using repeater::ControlVerb;

    switch(request.verb) {
    case ControlVerb::Status: {
        auto& out = controlPlane.beginReply(request.verb, true, true);
        writeStatusPayload(out);
        sendControlReply();
        break;
    }
    case ControlVerb::Relearn:
        router.clearLocalBlock();
        repeater::persistence::setLearnedRangeStart(0);
        persistedRangeStart = 0;
        eventSeq++;
        controlPlane.beginReply(request.verb, true);
        sendControlReply();
        break;

    case ControlVerb::ResetCounters:
        router.resetStats();
        uartErrors[0].store(0);
        uartErrors[1].store(0);
        turnaroundEvents[0].store(0);
        turnaroundEvents[1].store(0);
        controlPlane.beginReply(request.verb, true);
        sendControlReply();
        break;

    case ControlVerb::SetIndex: {
        int64_t value = 0;
        repeater::wire::MsgpackCursor cursor(request.payload, request.payloadSize);
        const bool ok = request.payload != nullptr && cursor.readInteger(value)
            && value >= 0 && value <= repeater::REPEATER_COUNT
            && repeater::persistence::setRepeaterIndex(static_cast<int8_t>(value));
        if(ok) {
            repeaterIndex = static_cast<int8_t>(value);
            controlPlane.setIdentity(repeaterIndex, macAddress);
            eventSeq++;
        }
        // Reply after adopting the new identity, so the source names where the
        // host should address this unit from now on.
        controlPlane.beginReply(request.verb, ok);
        sendControlReply();
        break;
    }
    case ControlVerb::Reboot:
        // Answer first; the reply has to reach the wire before the reset.
        controlPlane.beginReply(request.verb, true);
        sendControlReply();
        scheduleReboot();
        break;

    case ControlVerb::SnapshotStart: {
        // Broadcast, and answers nothing: all six branches sweep at once, and the
        // host reads them back one at a time afterwards.
        int64_t collectMs = repeater::SNAPSHOT_DEFAULT_COLLECT_MS;
        if(request.payload != nullptr) {
            repeater::wire::MsgpackCursor cursor(request.payload, request.payloadSize);
            int64_t value = 0;
            if(cursor.readInteger(value) && value > 0 && value <= 1000) collectMs = value;
        }
        if(router.blockState() == repeater::BlockState::Assigned) {
            snapshot.begin(router.localRangeStart(), static_cast<uint32_t>(collectMs), millis());
        }
        break;
    }
    case ControlVerb::SnapshotRead: {
        // The nine stored Portal replies are relayed verbatim, then a completion
        // frame. Feeding them starts here and continues from the service loop.
        snapshotReadCursor = 0;
        snapshotReadActive = !snapshot.collecting();
        if(!snapshotReadActive) {
            // Still sweeping: say so rather than answering with a partial set.
            auto& out = controlPlane.beginReply(request.verb, false, true);
            out.mapHeader(1);
            out.key("busy");
            out.boolean(true);
            sendControlReply();
        }
        break;
    }
    case ControlVerb::OtaBegin: {
        repeater::OtaBeginRequest begin;
        const bool parsed = parseOtaBegin(request, begin);
        // The erase inside this call takes hundreds of milliseconds with the cache
        // disabled, so inbound bytes are lost while it runs. Answering only after
        // it returns is what puts that loss inside a window the host is waiting in.
        const repeater::OtaResult result = parsed
            ? ota.begin(begin, millis())
            : repeater::OtaResult::BadRequest;
        if(result == repeater::OtaResult::Ok) router.setForwardingPaused(true);
        eventSeq++;
        replyWithOtaState(request.verb, result);
        break;
    }
    case ControlVerb::OtaData: {
        // Unacknowledged: this is the one high-volume verb, and the received-chunk
        // bitmap is how loss is discovered instead. That also means a chunk that never
        // lands says nothing about *why* on its own, so the two ways it can be lost
        // before `writeChunk` ever sees it -- never dispatched, or dispatched and
        // unparseable -- are counted separately and reported by `ota-state`.
        otaDataFrames++;
        uint8_t session = 0;
        uint32_t index = 0;
        const uint8_t* data = nullptr;
        uint32_t size = 0;
        uint16_t crc = 0;
        if(parseOtaData(request, session, index, data, size, crc)) {
            otaDataPayloadBytes = size;
            ota.writeChunk(session, index, data, size, crc, millis());
        }
        else {
            otaDataParseFailures++;
            otaDataPayloadBytes = request.payloadSize;
        }
        break;
    }
    case ControlVerb::OtaMap: {
        auto& out = controlPlane.beginReply(request.verb, true, true);
        out.mapHeader(5);
        out.key("state"); out.string(repeater::otaStateName(ota.state()));
        out.key("session"); out.uinteger(ota.session());
        out.key("chunks"); out.uinteger(ota.chunkCount());
        out.key("got"); out.uinteger(ota.receivedChunks());
        // The raw bitmap, not run lengths: fixed size, no worst case, and the host
        // is the side with a CPU to spare.
        out.key("map"); out.binary(ota.bitmap(), ota.bitmapBytes());
        sendControlReply();
        break;
    }
    case ControlVerb::OtaEnd: {
        const repeater::OtaResult result = ota.finish(millis());
        if(result == repeater::OtaResult::Ok) {
            otaRebootPending = true;
            router.setForwardingPaused(false);
        }
        else if(result != repeater::OtaResult::Incomplete) {
            // A verification or commit failure ends the session; only an incomplete
            // image is worth keeping open for a repair pass.
            router.setForwardingPaused(false);
        }
        eventSeq++;
        replyWithOtaState(request.verb, result);
        break;
    }
    case ControlVerb::OtaBoot:
        if(otaRebootPending) scheduleReboot();
        break;

    case ControlVerb::OtaConfirm:
        // An accelerator only. If the host never sends this, the image still
        // resolves on its own evidence within HEALTHY_UPTIME_MS.
        if(otaPendingVerify && !otaVerifyResolved) {
            otaVerifyResolved = repeater::EspOtaTarget::markValid();
            otaPendingVerify = !otaVerifyResolved;
            eventSeq++;
        }
        controlPlane.beginReply(request.verb, !otaPendingVerify);
        sendControlReply();
        break;

    case ControlVerb::OtaAbort:
        ota.abort();
        router.setForwardingPaused(false);
        otaRebootPending = false;
        eventSeq++;
        break;

    default:
        // Recognised as control-plane traffic but not implemented in this build.
        // Still answered, so the host can tell "not supported" from "not there".
        controlPlane.beginReply(request.verb, false);
        sendControlReply();
        break;
    }
}

class ControlDispatcher : public repeater::ControlFrameConsumer {
public:
    repeater::ControlDisposition consumeControlFrame(const uint8_t* frame, size_t size) override {
        const repeater::ControlRequest request = controlPlane.parse(frame, size);
        controlFramesSeen++;
        controlLastFrameBytes = size;
        if(!request.valid) {
            controlFramesInvalid++;
            return repeater::ControlDisposition::NotControl;
        }
        if(!request.addressedToUs) controlFramesNotOurs++;
        if(request.addressedToUs) handleControlRequest(request);
        // A broadcast is for the whole chain, so it is acted on here and passed on. A
        // unicast for somebody else is not ours to answer, but this panel is the only
        // road to the panels below it -- dropping it is what made them unreachable.
        if(request.broadcast) {
            return request.addressedToUs
                ? repeater::ControlDisposition::ConsumedAndRelay
                : repeater::ControlDisposition::Relay;
        }
        return request.addressedToUs
            ? repeater::ControlDisposition::Consumed
            : repeater::ControlDisposition::Relay;
    }
};

ControlDispatcher controlDispatcher;

/// Slow-moving bookkeeping: persist a newly learned range, and declare the boot
/// healthy once the application has stayed up long enough to prove it.
void serviceIdentity() {
    const uint32_t now = millis();
    if(static_cast<uint32_t>(now - lastIdentityServiceMs) < IDENTITY_SERVICE_PERIOD_MS) return;
    lastIdentityServiceMs = now;

    const repeater::BlockState state = router.blockState();
    if(state != lastReportedBlockState) {
        lastReportedBlockState = state;
        eventSeq++;
    }

    // The block follows the index, and nothing else. It used to be inferred from the
    // first branch reply, which in a chain is whichever panel answered first -- a panel
    // could and did adopt the block belonging to one below it.
    const uint8_t rangeStart = repeaterIndex == repeater::persistence::INDEX_UNSET
        ? 0
        : static_cast<uint8_t>((repeaterIndex - 1) * 9 + 1);
    if(rangeStart != persistedRangeStart) {
        if(rangeStart == 0) router.clearLocalBlock();
        else router.setLocalBlock(rangeStart);
        repeater::persistence::setLearnedRangeStart(rangeStart);
        persistedRangeStart = rangeStart;
        eventSeq++;
    }

    if(!healthyRecorded && uptimeMs() >= HEALTHY_UPTIME_MS) {
        repeater::persistence::noteHealthy();
        healthyRecorded = true;
    }

    if(rebootAtMs != 0 && static_cast<int32_t>(now - rebootAtMs) >= 0) {
        ESP.restart();
    }
}

/// Drives a branch sweep and, afterwards, feeds the collected replies back toward
/// the host. Both are paced by the originate queue rather than by a timer, so they
/// interleave with ordinary relaying instead of blocking it.
void serviceSnapshot() {
    const uint32_t now = millis();
    snapshot.service(now);

    if(snapshot.collecting()) {
        uint8_t poll[64];
        const size_t size = snapshot.nextPoll(now, poll, sizeof(poll));
        if(size > 0) router.originate(repeater::Side::Two, poll, size);
        return;
    }

    if(!snapshotReadActive) return;

    // Leave a slot free so the completion frame cannot be refused by a full queue.
    while(snapshotReadCursor < snapshot.storedCount()
        && router.originateDepth(repeater::Side::One) + 1 < repeater::ORIGINATE_QUEUE_DEPTH) {
        size_t size = 0;
        const uint8_t* stored = snapshot.storedFrame(snapshotReadCursor, size);
        if(stored == nullptr || size == 0) break;
        if(!router.originate(repeater::Side::One, stored, size)) break;
        snapshotReadCursor++;
    }

    if(snapshotReadCursor < snapshot.storedCount()) return;

    auto& out = controlPlane.beginReply(repeater::ControlVerb::SnapshotRead, true, true);
    out.mapHeader(4);
    out.key("start"); out.uinteger(snapshot.rangeStart());
    out.key("count"); out.uinteger(snapshot.storedCount());
    // Bit i set means `start + i` answered. The host reconciles this against the
    // relayed frames, and tolerates a late reply arriving outside the sweep.
    out.key("mask"); out.uinteger(snapshot.receivedMask());
    out.key("ms"); out.uinteger(snapshot.lastSweepMs());
    sendControlReply();
    snapshotReadActive = false;
}

/// Decides whether a freshly installed image keeps running or reverts.
///
/// The criterion is local evidence of malfunction, never absence of evidence of
/// health. A gate that required the host to check in would revert every morning
/// the rack powered up before the show PC, and because `esp_ota_begin` refuses to
/// run while an image is still pending verification, it would also lock out the
/// very update that fixed the problem.
void serviceOtaVerify() {
    if(!otaPendingVerify || otaVerifyResolved) return;

    // An image that will not stay up. The counter is cleared once the application
    // has run for HEALTHY_UPTIME_MS, so only repeated short-lived boots reach 3.
    if(repeater::persistence::unhealthyBoots() >= 3) {
        otaVerifyResolved = true;
        eventSeq++;
        if(!repeater::EspOtaTarget::rollBackAndReboot()) {
            // No valid image in the other slot. Nothing to revert to, so keep
            // running and let the host see this in the status.
            otaPendingVerify = false;
        }
        return;
    }

    // Positive evidence: frames arrived on the shared bus and decoded. That
    // exercises the UART, the transceiver, the DE line, COBS and the parser.
    const auto& stats = router.stats();
    const bool proven = stats.oneToTwo.receivedFrames > 0
        && stats.parseErrors < stats.oneToTwo.receivedFrames;

    // Silence is benign. An idle installation is not a broken image.
    const bool longEnough = uptimeMs() >= HEALTHY_UPTIME_MS;

    if(proven || longEnough) {
        otaVerifyResolved = repeater::EspOtaTarget::markValid();
        otaPendingVerify = !otaVerifyResolved;
        eventSeq++;
    }
}

void serviceUsbDiagnostics() {
    if(bootRecordPending && Serial && Serial.availableForWrite() >= 64) {
        Serial.printf(
            "{\"type\":\"boot\",\"version\":\"%s\",\"build\":\"%s\","
            "\"chip\":\"ESP32-C3\",\"baud\":%lu,"
            "\"side1_inverted\":%s,\"side2_inverted\":%s,"
            "\"pins\":{\"side1_rx\":20,\"side1_tx\":21,\"side1_de\":7,"
            "\"side2_rx\":6,\"side2_tx\":4,\"side2_de\":5}}\n",
            REPEATER_VERSION,
            REPEATER_BUILD_ID,
            static_cast<unsigned long>(RS485_BAUD),
            SIDE1_INVERTED ? "true" : "false",
            SIDE2_INVERTED ? "true" : "false");
        bootRecordPending = false;
    }

    while(Serial.available() > 0) {
        const char c = static_cast<char>(Serial.read());
        if(c == '\r' || c == '\n') {
            commandLine.trim();
            if(commandLine == "status") {
                printStatus("status");
            }
            else if(commandLine == "version") {
                printStatus("version");
            }
            else if(commandLine == "reset-counters") {
                router.resetStats();
                uartErrors[0].store(0);
                uartErrors[1].store(0);
                turnaroundEvents[0].store(0);
                turnaroundEvents[1].store(0);
                printStatus("counters-reset");
            }
            else if(commandLine == "relearn") {
                router.clearLocalBlock();
                repeater::persistence::setLearnedRangeStart(0);
                persistedRangeStart = 0;
                eventSeq++;
                printStatus("relearn");
            }
            else if(commandLine.startsWith("set-index")) {
                // `set-index N` where N is 1..6, or `set-index 0` to unprovision.
                const long value = commandLine.substring(9).toInt();
                if(value >= 0 && value <= 6 && repeater::persistence::setRepeaterIndex(static_cast<int8_t>(value))) {
                    repeaterIndex = static_cast<int8_t>(value);
                    eventSeq++;
                    printStatus("index");
                }
                else if(Serial.availableForWrite() >= 64) {
                    Serial.printf("{\"type\":\"error\",\"message\":\"index must be 0-6\"}\n");
                }
            }
            else if(commandLine == "index") {
                printStatus("index");
            }
            else if(commandLine.startsWith("idle-timeout")) {
                // `idle-timeout <us>`; bare, it reports. Adjustable at run time because the
                // value that matters is a property of how the Portals transmit, which is
                // measured on a branch rather than known in advance.
                String args = commandLine.substring(12);
                args.trim();
                if(args.length() > 0) {
                    const long value = args.toInt();
                    if(value >= 500 && value <= 200000) {
                        router.setIdleTimeoutUs(static_cast<uint32_t>(value));
                    }
                    else if(Serial.availableForWrite() >= 80) {
                        Serial.printf(
                            "{\"type\":\"error\",\"message\":\"idle-timeout 500..200000 us\"}\n");
                        commandLine = "";
                        continue;
                    }
                }
                if(Serial.availableForWrite() >= 64) {
                    Serial.printf("{\"type\":\"idle-timeout\",\"us\":%lu}\n",
                        static_cast<unsigned long>(router.idleTimeoutUs()));
                }
            }
            else if(commandLine.startsWith("tx-test")) {
                // `tx-test <1|2> [count]` -- emit inert frames on one bus so each
                // segment can be proven without a working host adapter.
                String args = commandLine.substring(7);
                args.trim();
                const int space = args.indexOf(' ');
                const long side = (space < 0 ? args : args.substring(0, space)).toInt();
                String rest = space < 0 ? String("") : args.substring(space + 1);
                rest.trim();
                const int second = rest.indexOf(' ');
                long count = rest.length() == 0 ? 5 : (second < 0 ? rest : rest.substring(0, second)).toInt();
                const long target = second < 0 ? 0 : rest.substring(second + 1).toInt();
                if(count < 1) count = 5;
                if(count > 200) count = 200;
                if((side == 1 || side == 2) && target >= 0 && target <= 127) {
                    txTestSide = side == 1 ? repeater::Side::One : repeater::Side::Two;
                    txTestRemaining = static_cast<uint32_t>(count);
                    txTestTarget = static_cast<int8_t>(target);
                    txTestLastMs = 0;
                    if(Serial.availableForWrite() >= 128) {
                        Serial.printf(
                            "{\"type\":\"tx-test\",\"side\":%ld,\"count\":%ld,\"target\":%ld,"
                            "\"body\":\"%s\"}\n",
                            side, count, target, target == 0 ? "inert marker" : "poll");
                    }
                }
                else if(Serial.availableForWrite() >= 80) {
                    Serial.printf(
                        "{\"type\":\"error\",\"message\":\"tx-test <1|2> [count] [portal-id]\"}\n");
                }
            }
            else if(commandLine == "ota-state" && Serial.availableForWrite() >= 160) {
                Serial.printf(
                    "{\"type\":\"ota-state\",\"slot\":\"%s\",\"state\":\"%s\","
                    "\"session\":%u,\"chunks\":%lu,\"got\":%lu,\"last\":\"%s\","

                    "\"reboot_pending\":%s,\"verify_pending\":%s,\"paused\":%s}\n",
                    repeater::EspOtaTarget::runningLabel(),
                    repeater::otaStateName(ota.state()),
                    static_cast<unsigned>(ota.session()),
                    static_cast<unsigned long>(ota.chunkCount()),
                    static_cast<unsigned long>(ota.receivedChunks()),
                    repeater::otaResultName(ota.lastError()),
                    otaRebootPending ? "true" : "false",
                    otaPendingVerify ? "true" : "false",
                    router.forwardingPaused() ? "true" : "false");
                Serial.flush();
                Serial.printf(
                    "{\"type\":\"ota-diag\",\"data_frames\":%lu,\"data_parse_fails\":%lu,"
                    "\"data_last_bytes\":%lu,\"ctrl_seen\":%lu,\"ctrl_invalid\":%lu,"
                    "\"ctrl_not_ours\":%lu,\"ctrl_last_bytes\":%lu}\n",
                    static_cast<unsigned long>(otaDataFrames),
                    static_cast<unsigned long>(otaDataParseFailures),
                    static_cast<unsigned long>(otaDataPayloadBytes),
                    static_cast<unsigned long>(controlFramesSeen),
                    static_cast<unsigned long>(controlFramesInvalid),
                    static_cast<unsigned long>(controlFramesNotOurs),
                    static_cast<unsigned long>(controlLastFrameBytes));
            }
            else if(commandLine == "rollback") {
                // Deliberate manual revert. Fails when the other slot holds no
                // valid image, which is what an aborted update leaves behind.
                const bool started = repeater::EspOtaTarget::rollBackAndReboot();
                if(!started && Serial.availableForWrite() >= 80) {
                    Serial.printf(
                        "{\"type\":\"error\",\"message\":\"no valid image to roll back to\"}\n");
                }
            }
            else if(commandLine.length() > 0 && Serial.availableForWrite() >= 32) {
                Serial.printf("{\"type\":\"error\",\"message\":\"unknown command\"}\n");
            }
            commandLine = "";
        }
        else if(commandLine.length() < 63) {
            commandLine += c;
        }
    }
}

void receiveAvailable(repeater::Side side, HardwareSerial& serial) {
    uint8_t buffer[COPY_CHUNK_BYTES];
    size_t count = 0;
    while(count < sizeof(buffer) && serial.available() > 0) {
        const int value = serial.read();
        if(value < 0) {
            break;
        }
        buffer[count++] = static_cast<uint8_t>(value);
    }
    if(count > 0) {
        router.ingest(side, buffer, count, micros());
        lastActivityMs = millis();
    }
}

void serviceBridge() {
    receiveAvailable(repeater::Side::One, Serial0);
    receiveAvailable(repeater::Side::Two, Serial1);
    router.expireIncomplete(micros());

    repeater::FrameView frame;
    if(!router.nextFrame(frame)) return;
    auto& destination = uartFor(frame.destination);
    setTransmitting(frame.destination, true);
    const size_t written = destination.write(frame.data, frame.size);
    destination.flush(true);
    setTransmitting(frame.destination, false);
    const bool ok = written == frame.size;
    if(frame.source == repeater::Side::None) router.completeOriginated(frame.destination, ok);
    else router.completeTransmission(frame.source, ok);
    lastActivityMs = millis();
}

void serviceLed() {
    const uint32_t now = millis();
    if(static_cast<uint32_t>(now - lastActivityMs) < DATA_LED_HOLD_MS) {
        digitalWrite(PIN_LED, HIGH);
        lastHeartbeatMs = now;
        heartbeatState = false;
        return;
    }
    if(static_cast<uint32_t>(now - lastHeartbeatMs) >= HEARTBEAT_HALF_PERIOD_MS) {
        lastHeartbeatMs = now;
        heartbeatState = !heartbeatState;
        digitalWrite(PIN_LED, heartbeatState ? HIGH : LOW);
    }
}

} // namespace

void setup() {
    bootResetReason = esp_reset_reason();
    coreDumpPresent = esp_core_dump_image_check() == ESP_OK;
    lastUptimeSampleMs = millis();

    repeater::persistence::begin();
    repeater::persistence::noteBootAttempt();
    repeaterIndex = repeater::persistence::repeaterIndex();
    persistedRangeStart = repeater::persistence::learnedRangeStart();
    router.setLocalBlock(persistedRangeStart);
    lastReportedBlockState = router.blockState();

    esp_read_mac(macAddress, ESP_MAC_WIFI_STA);
    controlPlane.setIdentity(repeaterIndex, macAddress);
    router.setControlFrameConsumer(&controlDispatcher);
    otaPendingVerify = repeater::EspOtaTarget::pendingVerify();
    router.setInnerReplyConsumer(&snapshot);

    pinMode(PIN_MAX1_DERE, OUTPUT);
    pinMode(PIN_MAX2_DERE, OUTPUT);
    digitalWrite(PIN_MAX1_DERE, LOW);
    digitalWrite(PIN_MAX2_DERE, LOW);

    pinMode(PIN_LED, OUTPUT);
    digitalWrite(PIN_LED, LOW);

    // RE# and DE are tied together on each MAX3362. While that transceiver is
    // transmitting, RE# is high and RO is high-impedance; bias the ESP RX input
    // to its physical idle level so the floating pin cannot create break/frame
    // events. An inverted UART idles low at the pin, a normal UART idles high.
    Serial0.setRxBufferSize(RX_BUFFER_BYTES);
    Serial0.setTxBufferSize(TX_BUFFER_BYTES);
    Serial1.setRxBufferSize(RX_BUFFER_BYTES);
    Serial1.setTxBufferSize(TX_BUFFER_BYTES);
    Serial0.onReceiveError([](hardwareSerial_error_t) {
        (transmitting[0].load() ? turnaroundEvents[0] : uartErrors[0]).fetch_add(1);
    });
    Serial1.onReceiveError([](hardwareSerial_error_t) {
        (transmitting[1].load() ? turnaroundEvents[1] : uartErrors[1]).fetch_add(1);
    });
    Serial0.begin(
        RS485_BAUD, SERIAL_8N1, PIN_MAX1_RO, PIN_MAX1_DI, SIDE1_INVERTED);
    Serial1.begin(
        RS485_BAUD, SERIAL_8N1, PIN_MAX2_RO, PIN_MAX2_DI, SIDE2_INVERTED);

    // HardwareSerial::begin() configures the GPIO matrix and clears the pulls,
    // so apply the idle bias after each UART has claimed its RX pin.
    gpio_set_pull_mode(
        static_cast<gpio_num_t>(PIN_MAX1_RO),
        SIDE1_INVERTED ? GPIO_PULLDOWN_ONLY : GPIO_PULLUP_ONLY);
    gpio_set_pull_mode(
        static_cast<gpio_num_t>(PIN_MAX2_RO),
        SIDE2_INVERTED ? GPIO_PULLDOWN_ONLY : GPIO_PULLUP_ONLY);

    Serial.begin(115200);
    commandLine.reserve(64);

    // The Arduino core leaves the loop task unsubscribed from the 5 s task watchdog.
    // A frame write blocks the loop with the driver enabled for up to 178 ms at
    // MAX_FRAME_BYTES, well inside that budget, so a trip means a genuine hang.
    enableLoopWDT();
}

void loop() {
    serviceBridge();
    serviceUsbDiagnostics();
    serviceTxTest();
    serviceIdentity();
    serviceSnapshot();
    serviceOtaVerify();
    ota.service(millis());
    if(!ota.busy() && router.forwardingPaused() && !otaRebootPending) {
        // The session timed out. Relaying has to come back on its own, or an
        // abandoned update leaves nine Portals dark until someone visits.
        router.setForwardingPaused(false);
        eventSeq++;
    }
    serviceLed();
    delay(0);
}

/// Arduino resolves a pending-verify image inside `initArduino()` by default,
/// before `setup()` has run and long before the bus has said anything. Deferring
/// it hands the decision to `serviceOtaVerify()`.
///
/// `extern "C"` is load-bearing: the weak symbol lives in `esp32-hal-misc.c`, so a
/// C++-mangled definition would link cleanly and simply never override anything.
extern "C" bool verifyRollbackLater() {
    return true;
}

#endif

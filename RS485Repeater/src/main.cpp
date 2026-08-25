#ifdef ARDUINO

#include <Arduino.h>
#include <HardwareSerial.h>
#include <atomic>
#include <driver/gpio.h>

#include "BridgeCore.h"

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
constexpr uint32_t FRAME_IDLE_TIMEOUT_US = 2000;
constexpr size_t RX_BUFFER_BYTES = 12288;
constexpr size_t TX_BUFFER_BYTES = 4096;
constexpr size_t COPY_CHUNK_BYTES = 128;
constexpr uint32_t HEARTBEAT_HALF_PERIOD_MS = 500;
constexpr uint32_t DATA_LED_HOLD_MS = 100;
constexpr bool SIDE1_INVERTED = REPEATER_SIDE1_INVERT != 0;
constexpr bool SIDE2_INVERTED = REPEATER_SIDE2_INVERT != 0;

repeater::FrameRouter router(FRAME_IDLE_TIMEOUT_US);
std::atomic<uint32_t> uartErrors[2] = {0, 0};
std::atomic<uint32_t> turnaroundEvents[2] = {0, 0};
std::atomic<bool> transmitting[2] = {false, false};
uint32_t lastActivityMs = 0;
uint32_t lastHeartbeatMs = 0;
bool heartbeatState = false;
bool bootRecordPending = true;
String commandLine;

HardwareSerial& destinationFor(repeater::Side side) {
    return side == repeater::Side::One ? Serial1 : Serial0;
}

void setTransmitFor(repeater::Side source, bool enabled) {
    const bool destinationIsTwo = source == repeater::Side::One;
    const uint8_t destinationIndex = destinationIsTwo ? 1 : 0;
    const uint8_t destinationDere = destinationIsTwo ? PIN_MAX2_DERE : PIN_MAX1_DERE;
    if(enabled) {
        transmitting[destinationIndex].store(true);
    }
    digitalWrite(destinationDere, enabled ? HIGH : LOW);
    if(!enabled) {
        transmitting[destinationIndex].store(false);
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
        "\"range_end\":%u,\"filtered_unicasts\":%llu,\"filtered_keyframes\":%llu,"
        "\"filtered_host_frames\":%llu,\"parse_errors\":%llu,\"conflicts\":%llu},"
        "\"side1\":{\"inverted\":%s,\"rx_bytes\":%llu,\"received_frames\":%llu,"
        "\"forwarded_bytes\":%llu,\"frames\":%llu,\"incomplete\":%llu,"
        "\"oversized\":%llu,\"queue_depth\":%u,\"queue_high_water\":%u,\"queue_drops\":%llu,"
        "\"uart_errors\":%lu,\"turnaround_events\":%lu},"
        "\"side2\":{\"inverted\":%s,\"rx_bytes\":%llu,\"received_frames\":%llu,"
        "\"forwarded_bytes\":%llu,\"frames\":%llu,\"incomplete\":%llu,"
        "\"oversized\":%llu,\"queue_depth\":%u,\"queue_high_water\":%u,\"queue_drops\":%llu,"
        "\"uart_errors\":%lu,\"turnaround_events\":%lu},"
        "\"tx_errors\":%llu,\"last_activity_ms\":%lu}\n",
        type,
        REPEATER_VERSION,
        REPEATER_BUILD_ID,
        static_cast<unsigned long>(RS485_BAUD),
        repeater::routingModeName(router.routingMode()),
        static_cast<unsigned>(router.localRangeStart()),
        static_cast<unsigned>(router.localRangeEnd()),
        stats.filteredUnicasts,
        stats.filteredKeyframes,
        stats.filteredHostFrames,
        stats.parseErrors,
        stats.topologyConflicts,
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
        static_cast<unsigned long>(millis() - lastActivityMs));
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
                router.relearn();
                printStatus("relearn");
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
    auto& destination = destinationFor(frame.source);
    setTransmitFor(frame.source, true);
    const size_t written = destination.write(frame.data, frame.size);
    destination.flush(true);
    setTransmitFor(frame.source, false);
    router.completeTransmission(frame.source, written == frame.size);
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
}

void loop() {
    serviceBridge();
    serviceUsbDiagnostics();
    serviceLed();
    delay(0);
}

#endif

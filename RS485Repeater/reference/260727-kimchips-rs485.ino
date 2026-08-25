/**
 * 260727-kimchips-rs485 Firmware v2.0
 * Bidirectional RS485 Repeater/Bridge using ESP32-C3-WROOM-02U-N4
 *
 * Author: Wonseok Choi, Retrix
 * Description:
 *   Monitors both MAX3362 RS485 transceivers simultaneously.
 *   - Data received from the first MAX3362 is relayed through the second MAX3362.
 *   - Data received from the second MAX3362 is relayed through the first MAX3362.
 *   Both directions operate in pseudo-full-duplex mode: each MAX3362 stays in
 *   receive mode by default and briefly switches to transmit mode only when
 *   forwarding data to the opposite side.
 *
 * Hardware Connections:
 *   ESP32-C3 <-> First MAX3362:
 *     GPIO20 (UART0 RX) <-> RO  (Receiver Output)
 *     GPIO21 (UART0 TX) <-> DI  (Driver Input)
 *     GPIO7             <-> RE# & DE (shared, LOW = receive, HIGH = transmit)
 *
 *   ESP32-C3 <-> Second MAX3362:
 *     GPIO6  (UART1 RX) <-> RO  (Receiver Output)
 *     GPIO4  (UART1 TX) <-> DI  (Driver Input)
 *     GPIO5             <-> RE# & DE (shared, LOW = receive, HIGH = transmit)
 *
 *   ESP32-C3 <-> Status LED:
 *     GPIO10 <-> LED (heartbeat blink / data activity indicator)
 */

#include <HardwareSerial.h>

// ============================================================================
// Pin Definitions
// ============================================================================
#define PIN_MAX1_RO      20   // First MAX3362  - Receiver Output   (UART0 RX)
#define PIN_MAX1_DI      21   // First MAX3362  - Driver Input      (UART0 TX)
#define PIN_MAX1_DERE    7    // First MAX3362  - RE# and DE control (shared)
#define PIN_MAX2_RO      6    // Second MAX3362 - Receiver Output   (UART1 RX)
#define PIN_MAX2_DI      4    // Second MAX3362 - Driver Input      (UART1 TX)
#define PIN_MAX2_DERE    5    // Second MAX3362 - RE# and DE control (shared)
#define PIN_LED          10   // Status LED

// ============================================================================
// HardwareSerial instances for UART0 and UART1
// On ESP32-C3, Serial1 is pre-defined but Serial0 is not.
// We explicitly create both for clarity and cross-platform compatibility.
// ============================================================================
HardwareSerial Serial0(0);  // UART0 -> first MAX3362

// ============================================================================
// User-Configurable RS485 Baud Rate
// Default: 115,200 bps
// Modify this variable to change the RS485 communication speed.
// ============================================================================
unsigned long rs485BaudRate = 115200;  // 115.2 kbps

// ============================================================================
// LED Timing Constants
// ============================================================================
const unsigned long HEARTBEAT_INTERVAL_MS = 1000;   // 1-second heartbeat cycle
const unsigned long DATA_ACTIVE_TIMEOUT_MS = 100;   // LED stays lit for 100ms after last data byte

// ============================================================================
// State Variables
// ============================================================================
unsigned long lastHeartbeatToggle = 0;
unsigned long lastDataTimestamp    = 0;
bool          heartbeatLedState    = false;

// ============================================================================
// Helper: Set MAX3362 mode
//   LOW  = receive mode  (RE# = LOW,  DE = LOW)
//   HIGH = transmit mode (RE# = HIGH, DE = HIGH)
// ============================================================================
static inline void max1_set_mode(uint8_t mode) {
  digitalWrite(PIN_MAX1_DERE, mode);
}

static inline void max2_set_mode(uint8_t mode) {
  digitalWrite(PIN_MAX2_DERE, mode);
}

// ============================================================================
// setup()
// ============================================================================
void setup() {
  // --- MAX3362 Control Pins ---
  pinMode(PIN_MAX1_DERE, OUTPUT);
  pinMode(PIN_MAX2_DERE, OUTPUT);

  // Both MAX3362 start in receive mode (RE# = LOW, DE = LOW)
  max1_set_mode(LOW);
  max2_set_mode(LOW);

  // --- Status LED ---
  pinMode(PIN_LED, OUTPUT);
  digitalWrite(PIN_LED, LOW);

  // --- UART Initialization (Full Duplex) ---
  // UART0: connected to first MAX3362  (RX = GPIO20, TX = GPIO21)
  Serial0.begin(rs485BaudRate, SERIAL_8N1, PIN_MAX1_RO, PIN_MAX1_DI);

  // UART1: connected to second MAX3362 (RX = GPIO6,  TX = GPIO4)
  Serial1.begin(rs485BaudRate, SERIAL_8N1, PIN_MAX2_RO, PIN_MAX2_DI);

  // NOTE: USB CDC (Serial) is intentionally NOT initialized per requirements.
}

// ============================================================================
// loop()
// ============================================================================
void loop() {
  bool dataReceived = false;

  // ---------------------------------------------------------------
  // Direction A: First MAX3362 (Serial0) -> Second MAX3362 (Serial1)
  // ---------------------------------------------------------------
  while (Serial0.available() > 0) {
    uint8_t byteValue = Serial0.read();

    // Switch MAX2 to transmit mode, send the byte, then return to receive mode
    max2_set_mode(HIGH);
    Serial1.write(byteValue);
    Serial1.flush();        // wait until TX FIFO is empty before switching back
    max2_set_mode(LOW);

    dataReceived = true;
  }

  // ---------------------------------------------------------------
  // Direction B: Second MAX3362 (Serial1) -> First MAX3362 (Serial0)
  // ---------------------------------------------------------------
  while (Serial1.available() > 0) {
    uint8_t byteValue = Serial1.read();

    // Switch MAX1 to transmit mode, send the byte, then return to receive mode
    max1_set_mode(HIGH);
    Serial0.write(byteValue);
    Serial0.flush();        // wait until TX FIFO is empty before switching back
    max1_set_mode(LOW);

    dataReceived = true;
  }

  // ---------------------------------------------------------------
  // LED Control
  // ---------------------------------------------------------------
  unsigned long currentMillis = millis();

  // Track the last time data was actively relayed
  if (dataReceived) {
    lastDataTimestamp = currentMillis;
  }

  // Determine if data activity window is still open
  bool isDataActive = (currentMillis - lastDataTimestamp) < DATA_ACTIVE_TIMEOUT_MS;

  if (isDataActive) {
    // Solid ON while data is being actively relayed
    digitalWrite(PIN_LED, HIGH);
    // Reset heartbeat phase so the blink cycle restarts cleanly after data stops
    lastHeartbeatToggle = currentMillis;
    heartbeatLedState   = false;
  } else {
    // Heartbeat: toggle every half-interval (500 ms ON, 500 ms OFF -> 1 Hz blink)
    if (currentMillis - lastHeartbeatToggle >= (HEARTBEAT_INTERVAL_MS / 2)) {
      lastHeartbeatToggle = currentMillis;
      heartbeatLedState   = !heartbeatLedState;
      digitalWrite(PIN_LED, heartbeatLedState ? HIGH : LOW);
    }
  }
}

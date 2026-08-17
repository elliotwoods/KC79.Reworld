// Minimal Logger stub for the HomeSwitchTest `home_switch_bench` build.
//
// MotorDriver.cpp (reused verbatim from PortalFW) #includes Logger.h and calls
// the free log() function inside its testRoutine()/testTimer() helpers. The
// bench firmware never calls those helpers, but the log() symbol must still
// resolve at link time. The real Logger.cpp cannot be pulled in here: it owns
// the interactive debug menu and is tightly coupled to App::X() (which would
// drag in the whole PortalFW application). So we satisfy just the free log()
// overloads declared in Logger.h as no-ops.
//
// Bench progress / status is reported over the wire by bench_main.cpp as
// `L,...` / `S,...` telemetry lines, not through this path.

#include "Logger.h"

void log(const LogLevel&, const char*, const char*, bool) {}
void log(const LogMessage&) {}
void log(const Exception&) {}

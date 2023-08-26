
#include <Arduino.h>

HardwareSerial serial(PB7, PB6);

extern "C" {
	#include "logger.h"
	RAM_FUNC
	void logInit() {
		serial.begin(115200);
	}

	RAM_FUNC
	void logPrintln(const char * text) {
		serial.println(text);
		serial.flush();
	}

	RAM_FUNC
	void logPrint(const char * text) {
		serial.print(text);
		serial.flush();
	}
}
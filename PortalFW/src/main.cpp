#include <Arduino.h>
#include "Modules/App.h"

Modules::App app;

void setup() {
	// LED's
	pinMode(PB3, OUTPUT);
	pinMode(PB4, OUTPUT);

	app.setup();
}

void loop() {
	// Cycle LED's
	analogWrite(PB3, (millis() % 1000) / 64);
	
	app.update();
}
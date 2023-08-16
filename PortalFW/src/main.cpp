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
	analogWrite(PB4, (millis() % 1000) / 64);
	
	app.update();

	// We need some delay to allow for bigger numbers in MotionControl (otherwise can't accelerate)
	HAL_Delay(1);
}
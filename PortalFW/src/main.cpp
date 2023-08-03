#include <Arduino.h>
#include "Modules/App.h"

Modules::App app;
uint32_t frame = 0;

void setup() {
	// LED's
	pinMode(PB3, OUTPUT);
	pinMode(PB4, OUTPUT);

	app.setup();
}

void loop() {
	// Cycle LED's
	analogWrite(PB3, frame % 32);
	
	app.update();

	frame++;
}
#include "LEDs.h"
#include "Arduino.h"
#include "App.h"

namespace Modules {
	//----------
	const char *
	LEDs::getTypeName() const
	{
		return "LEDs";
	}

	//----------
	void
	LEDs::update()
	{
		auto app = &App::X();

		// Heartbeat LED
		{
			const auto & healthStatusA = app->motionControlA->getHealthStatus();
			const auto & healthStatusB = app->motionControlB->getHealthStatus();

			const auto allOK = healthStatusA.allOK() && healthStatusB.allOK();

			if(allOK) {
				// Slow heartbeat
				analogWrite(LED_HEARTBEAT, (millis() % 2000) / 128);
			} else {
				// Write a debug sequence out

				int lengthOfSequence = 16;
				int positionInSequence = (millis() / 200) % lengthOfSequence;
				
				bool value = false;

				switch(positionInSequence) {
					// one flash for axis A
					case 0:
					value = !healthStatusA.allOK();
					break;

					// two flashes for axis B
					case 4:
					case 6:
					value = !healthStatusB.allOK();
					break;
				}

				digitalWrite(LED_HEARTBEAT, value);
			}
		}

		// Motor indicator LED
		if(this->motorIndicatorEnabled) {
			digitalWrite(LED_INDICATOR, app->motorDriverA->getEnabled() || app->motorDriverB->getEnabled());
		}
	}

	//----------
	void
	LEDs::setMotorIndicatorEnabled(bool enabled)
	{
		this->motorIndicatorEnabled = enabled;
	}
}
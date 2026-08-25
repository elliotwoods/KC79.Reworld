#include "HomeSwitchOptical.h"
#include "Arduino.h"
#include "HardwareTimer.h"

namespace Modules {
	// Shared comparator threshold (PC15). One software-PWM signal feeds both axes.
	const uint32_t pinThreshold = PC15;

	// 8-bit software PWM at ~80Hz -> ISR at 80 * 256 = 20480Hz. The RC filter
	// (100k / 1uF, corner ~1.6Hz) smooths this to clean DC on the comparator +IN.
	const uint32_t thresholdPwmFrequency = 80;

	volatile uint8_t HomeSwitchOptical_thresholdDuty = HOMESWITCHOPTICAL_DEFAULT_THRESHOLD;
	HardwareTimer * HomeSwitchOptical_thresholdTimer = nullptr;

#pragma mark HomeSwitchOptical
	//----------
	std::set<HomeSwitchOptical*> HomeSwitchOptical::allHomeSwitches;

	//----------
	HomeSwitchOptical::Config
	HomeSwitchOptical::Config::A()
	{
		Config config;
		{
			config.pinSensor = PC13;
		}

		return config;
	}

	//----------
	HomeSwitchOptical::Config
	HomeSwitchOptical::Config::B()
	{
		Config config;
		{
			config.pinSensor = PC14;
		}

		return config;
	}

#pragma mark HomeSwitchOptical
	//----------
	HomeSwitchOptical::HomeSwitchOptical(const Config& config)
	: config(config)
	{
		// Comparator output is push-pull, so no pull-up needed.
		pinMode(this->config.pinSensor, INPUT);

		// Resolve the port and mask once, for getRawActive()'s use inside the step ISR.
		{
			const PinName pinName = digitalPinToPinName(this->config.pinSensor);
			this->sensorPort = get_GPIO_Port(STM_PORT(pinName));
			this->sensorPinMask = STM_LL_GPIO_PIN(pinName);
		}

		HomeSwitchOptical::allHomeSwitches.insert(this);
	}

	//----------
	const char *
	HomeSwitchOptical::getTypeName() const
	{
		return "HomeSwitchOptical";
	}

	//----------
	void
	HomeSwitchOptical::setup()
	{
		// PC15 is in the VBAT/OSC32 domain and has no timer alternate-function,
		// so analogWrite cannot drive it. Generate the threshold with software
		// PWM from a free basic timer (TIM6) update interrupt.
		// Guarded so it only runs once, even though both axes call setup().
		static bool thresholdStarted = false;
		if (thresholdStarted) {
			return;
		}
		thresholdStarted = true;

		pinMode(pinThreshold, OUTPUT);
		digitalWrite(pinThreshold, LOW);

		HomeSwitchOptical_thresholdTimer = new HardwareTimer(TIM6);
		HomeSwitchOptical_thresholdTimer->setOverflow(thresholdPwmFrequency * 256
			, HERTZ_FORMAT);
		HomeSwitchOptical_thresholdTimer->attachInterrupt([]() {
			static uint8_t count = 0;
			digitalWrite(pinThreshold
				, count < HomeSwitchOptical_thresholdDuty ? HIGH : LOW);
			count++;
		});
		HomeSwitchOptical_thresholdTimer->resume();
	}

	//-----------
	bool
	HomeSwitchOptical::getForwardsActive() const
	{
		return this->getRawActive();
	}

	//-----------
	bool
	HomeSwitchOptical::getBackwardsActive() const
	{
		return this->getRawActive();
	}

	//----------
	void
	HomeSwitchOptical::setThreshold(uint8_t duty)
	{
		HomeSwitchOptical_thresholdDuty = duty;
	}

	//----------
	uint8_t
	HomeSwitchOptical::getThreshold()
	{
		return HomeSwitchOptical_thresholdDuty;
	}
}

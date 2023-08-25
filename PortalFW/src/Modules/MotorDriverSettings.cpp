#include "MotorDriverSettings.h"
#include "Arduino.h"

enum MState {
	High,
	Low,
	High_Z
};

void setMState(uint32_t pin, MState mState)
{
	switch(mState) {
		case(MState::High):
		{
			pinMode(pin, OUTPUT);
			digitalWrite(pin, HIGH);
			break;
		}
		case(MState::Low):
		{
			pinMode(pin, OUTPUT);
			digitalWrite(pin, LOW);
			break;
		}
		case(MState::High_Z):
		{
			pinMode(pin, INPUT);
			break;
		}
		default:
			break;
	}
}

namespace Modules {
	//----------
	MotorDriverSettings::MotorDriverSettings(const Config& config)
	: config(config)
	{
		pinMode(config.pinVREF, OUTPUT);
		pinMode(config.pinSleep, OUTPUT);

		this->pushState();
	}

	//----------
	const char *
	MotorDriverSettings::getTypeName() const
	{
		return "MotorDriverSettings";
	}

	//----------
	void
	MotorDriverSettings::setMicrostepResolution(MicrostepResolution value)
	{
		this->state.microStepResolution = value;
		this->pushMicrostepResoltuion();
	}

	//----------
	MotorDriverSettings::MicrostepResolution
	MotorDriverSettings::getMicrostepResolution() const
	{
		return this->state.microStepResolution;
	}

	//----------
	Steps
	MotorDriverSettings::getMicrostepsPerStep() const
	{
		return Steps(1 << (uint32_t) (uint8_t) this->getMicrostepResolution());
	}

	//----------
	void
	MotorDriverSettings::setSleep(bool value)
	{
		this->state.sleep = value;
		this->pushSleep();
	}

	//----------
	bool
	MotorDriverSettings::getSleep() const
	{
		return this->state.sleep;
	}

	//----------
	void
	MotorDriverSettings::setCurrent(float value)
	{
		this->state.current = value;
		this->pushCurrent();
	}

	//----------
	float
	MotorDriverSettings::getCurrent() const
	{
		return this->state.current;
	}

	//----------
	void
	MotorDriverSettings::pushState()
	{
		this->pushMicrostepResoltuion();
		this->pushSleep();
		this->pushCurrent();
	}

	//----------
	void
	MotorDriverSettings::pushMicrostepResoltuion()
	{
		switch(this->state.microStepResolution) {
			case MicrostepResolution::_1:
				setMState(this->config.pinM0, MState::Low);
				setMState(this->config.pinM1, MState::Low);
				break;
			case MicrostepResolution::_2:
				setMState(this->config.pinM0, MState::High_Z);
				setMState(this->config.pinM1, MState::Low);
				break;
			case MicrostepResolution::_4:
				setMState(this->config.pinM0, MState::Low);
				setMState(this->config.pinM1, MState::High);
				break;
			case MicrostepResolution::_8:
				setMState(this->config.pinM0, MState::High);
				setMState(this->config.pinM1, MState::High);
				break;
			case MicrostepResolution::_16:
				setMState(this->config.pinM0, MState::High_Z);
				setMState(this->config.pinM1, MState::High);
				break;
			case MicrostepResolution::_32:
				setMState(this->config.pinM0, MState::Low);
				setMState(this->config.pinM1, MState::High_Z);
				break;
			case MicrostepResolution::_128:
				setMState(this->config.pinM0, MState::High_Z);
				setMState(this->config.pinM1, MState::High_Z);
				break;
			case MicrostepResolution::_256:
				setMState(this->config.pinM0, MState::High);
				setMState(this->config.pinM1, MState::High_Z);
				break;
		}
	}

	//----------
	void
	MotorDriverSettings::pushSleep()
	{
		digitalWrite(this->config.pinSleep, this->state.sleep ? LOW : HIGH);
	}

	//----------
	void
	MotorDriverSettings::pushCurrent()
	{
		if(this->state.current > MOTORDRIVERSETTINGS_MAX_CURRENT) {
			this->state.current = MOTORDRIVERSETTINGS_MAX_CURRENT;
		}

		const float voltageAtMotorDriver = this->state.current * 3.0f;
		const float voltageBeforeDivider = voltageAtMotorDriver / this->config.vrefRatio;
		float pwmRatio = (voltageBeforeDivider / 3.3f);

		// Cap the ratio to 100%
		if(pwmRatio > 1.0f) {
			pwmRatio = 1.0f;
		}

		analogWrite(this->config.pinVREF, (uint32_t) (255.0f * pwmRatio));
	}

	//----------
	bool
	MotorDriverSettings::processIncomingByKey(const char * key, Stream & stream)
	{
		if(strcmp(key, "setCurrent") == 0) {
			Amps value;
			if(!msgpack::readFloat(stream, value)) {
				return false;
			}

			this->setCurrent(value);
			return true;
		}
		else if(strcmp(key, "setMicrostepResolution") == 0) {
			int32_t value;

			// we use int32_t since we already use this symbol elsewhere
			// (to save space in flash we use the same one again here)
			if(!msgpack::readInt<int32_t>(stream, value)) {
				return false;
			}

			this->setMicrostepResolution((MicrostepResolution) value);
			return true;
		}

		return false;
	}
}
#pragma once

#include "Base.h"

#include <stdint.h>
#include <stddef.h>
#include <set>

namespace Modules {
	class HomeSwitchMechanical : public Base {
	public:
		struct Config
		{
			uint32_t pinBackwardsSwitch;
			uint32_t pinForwardsSwitch;

			static Config A();
			static Config B();
		};

		HomeSwitchMechanical(const Config& = Config());
		const char * getTypeName() const;

		static std::set<HomeSwitchMechanical*> allHomeSwitches;

		bool getForwardsActive() const;
		bool getBackwardsActive() const;

		// Both latch inputs in one call, for the step ISR. Two distinct switches here, so this
		// is two reads -- unlike the optical switch, where it is one. See HomeSwitchOptical.h.
		struct RawState {
			bool forwards;
			bool backwards;
		};
		RawState getRawState() const {
			return RawState { this->getForwardsActive(), this->getBackwardsActive() };
		}
	protected:
		const Config config;
	};
}

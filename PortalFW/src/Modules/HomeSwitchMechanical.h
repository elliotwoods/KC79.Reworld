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
	protected:
		const Config config;
	};
}

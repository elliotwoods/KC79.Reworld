#pragma once

#include "Base.h"

#include <stdint.h>
#include <stddef.h>
#include <set>

namespace Modules {
	class HomeSwitch : public Base {
	public:
		struct Config
		{
			uint32_t pinLeftSwitch;
			uint32_t pinRightSwitch;

			static Config A();
			static Config B();
		};
		
		HomeSwitch(const Config& = Config());
		const char * getTypeName() const;

		static std::set<HomeSwitch*> allHomeSwitches;
		void handleInterrupt();
	protected:
		const Config config;
	};
}
#pragma once

#include "Base.h"
#include <stdint.h>
#include <stddef.h>

namespace Modules {
	class MotorDriver : public Base {
	public:
		struct Config {
			uint32_t Fault;
			uint32_t Enable;
			uint32_t Step;
			uint32_t Direction;

			static Config MotorA();
			static Config MotorB();
		};

		MotorDriver(const Config&);

		void setEnabled(bool);
		bool getEnabled() const;

		void setDirection(bool);
		bool getDirection() const;

		void testSteps(size_t stepCount, uint32_t delayBetweenSteps);
		void testRoutine();

	protected:
		void pushState();
		void pushEnabled();
		void pushDirection();

		const Config config;

		struct {
			bool enabled = false;
			bool direction = false;
		} state;
	};
}

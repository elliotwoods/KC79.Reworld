#include "Base.h"

namespace Modules {
	class Routines : public Base {
	public:
		Routines(App * app);

		// Run all routines in sequence
		void startup()

		bool unjam();
		bool tuneCurrent();
		bool calibrate();
		bool calibrateRoutine(uint8_t tryCount);
	};
}
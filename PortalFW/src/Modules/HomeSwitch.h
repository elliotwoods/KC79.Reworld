#pragma once

// Selects the home-switch implementation. The rest of the codebase only ever
// names "Modules::HomeSwitch"; this typedef points it at the right class.
//   - default: HomeSwitchOptical (reflective IR sensor + comparator, active-high)
//   - HOME_SWITCH_LEGACY: HomeSwitchMechanical (rev-1 mechanical switches, active-low)
#ifdef HOME_SWITCH_LEGACY
	#include "HomeSwitchMechanical.h"
	namespace Modules { typedef HomeSwitchMechanical HomeSwitch; }
#else
	#include "HomeSwitchOptical.h"
	namespace Modules { typedef HomeSwitchOptical HomeSwitch; }
#endif

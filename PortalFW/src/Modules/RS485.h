#pragma once
#include "Base.h"
#include "Exception.h"

namespace Modules {
	class App;

	class RS485 : public Base {
	public:
		RS485(App *);
		void setup() override;
		void update() override;
	protected:
		App * app;
		void processIncoming();
		Exception processCOBSPacket();

		void beginTransmission();
		void endTransmission();
		
		void transmitOutbox();
	};
}
#pragma once
#include "Base.h"
#include "Exception.h"

namespace Modules {
	class App;

	class RS485 : public Base {
	public:
		RS485(App *);
		const char * getTypeName() const;
		void setup() override;
		void update() override;
		
		void sendStatusReport();

		// Use this function if you want to manually send an ACK
		// e.g. if the message starts a routine which takes time (init/home/etc)
		static void sendACKEarly(bool success);

		// Use this in case where your message response will be sent manually
		// e.g. for poll requests
		static void noACKRequired();
	protected:
		App * app;
		static RS485 * instance;

		void processIncoming();
		Exception processCOBSPacket();

		void beginTransmission();
		void endTransmission();
		
		void sendACK(bool success);

		bool disableACK = false;
		bool sentACKEarly = false;
	};
}
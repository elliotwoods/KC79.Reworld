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
		void sendPositions();

		// Use this function if you want to manually send an ACK
		// e.g. if the message starts a routine which takes time (init/home/etc)
		static void sendACKEarly(bool success);

		// Use this to set disableACK to true for this receive frame
		// e.g. for when you manually give a response
		// Returns false if already no ACK was required
		static void noACKRequired();

		// Are you allowed to reply to a message?
		static bool replyAllowed();

		bool hasAnySignalBeenReceived() const;
	protected:
		App * app;
		static RS485 * instance;

		void processIncoming();
		Exception processCOBSPacket(bool & isForUs);

		void beginTransmission();
		void endTransmission();
		
		void sendACK(bool success);

		bool disableACK = false;
		bool sentACKEarly = false;

		bool anySignalReceived = false;
	};
}
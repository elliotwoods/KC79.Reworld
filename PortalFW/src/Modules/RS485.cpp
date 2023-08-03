#include "RS485.h"
#include <Arduino.h>

#include <msgpack.hpp>

#include "App.h"
#include "Log.h"
#include "Exception.h"

#include "Data/MessageType.h"

HardwareSerial serialRS485(PA3, PA2);
msgpack::COBSRWStream cobsStream(serialRS485);

#define PIN_DE PA1
namespace Modules {
	//---------
	RS485::RS485(App * app)
	: app(app)
	{

	}

	//---------
	void
	RS485::setup()
	{
		serialRS485.begin(115200);

		// Setup the DE pin
		pinMode(PIN_DE, OUTPUT);
		digitalWrite(PIN_DE, LOW);
	}

	//---------
	void
	RS485::update()
	{
		this->processIncoming();

	 	digitalWrite(PIN_DE, HIGH);
		cobsStream.print(".");
		cobsStream.flush();
		digitalWrite(PIN_DE, LOW);
	}

	//---------
	void
	RS485::processIncoming()
	{
		const auto ourID = this->app->id->get();

		// Skip any partial packets
		if(!cobsStream.isStartOfIncomingPacket()) {
			cobsStream.nextIncomingPacket();
		}

		while(cobsStream.isStartOfIncomingPacket() && cobsStream.available()) {
			bool needsReply = false;

			try
			{
				MessageType messageType;

				// Check it's a message for us
				bool weShouldProcess = false;
				{
					MessageType messageType;
					
					// First part is the message type
					if(!msgpack::readInt(cobsStream, (uint8_t&) messageType)) {
						throw Exception("Message type invalid");
					};

					if(messageType == MessageType::ServerBroadcast) {
						weShouldProcess = true;
					}
					else if(messageType == MessageType::ServerUnicast) {
						// For Unicast, the next part is the ID for the target
						uint8_t targetID;
						if(!msgpack::readInt(cobsStream, targetID)) {
							throw(Exception("Messge format error"));
						}

						if(targetID == ourID) {
							weShouldProcess = true;
						}
					}

					if(weShouldProcess) {
						// We should process this packet
						auto success = app->processIncoming(cobsStream);
						if(!success) {
							log(LogLevel::Error, "Failed to process packet");
						}
					}
				}
			}
			catch(const Exception & e)
			{
				log(LogLevel::Error, e.what());
			}

			cobsStream.nextIncomingPacket();
		}
	}
}
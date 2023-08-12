#include "ID.h"
#include <Arduino.h>
#include "Logger.h"

#define ID_RX_PIN PB9
#define ID_TX_PIN PB8

HardwareSerial serialID(ID_RX_PIN, ID_TX_PIN);

namespace Modules {
	//----------
	const uint32_t ID::binaryPins[4] {PD0, PD1, PD2, PD3};

	//----------
	ID::ID()
	{

	}

	//----------
	const char *
	ID::getTypeName() const
	{
		return "ID";
	}

	//---------
	void
	ID::setup()
	{
		// set the pin modes
		for(size_t i=0; i<4; i++) {
			pinMode(ID::binaryPins[i], INPUT_PULLUP);
		}

		// initialise the serial device
		serialID.begin(115200);

		// init from the board
		this->initFromBoard();

		// flush the incoming serial buffer (we often get a 0 byte on startup)
		while(serialID.available()) {
			serialID.read();
		}
	}

	//----------
	void
	ID::initFromBoard()
	{
		// accumulate values from the pins
		int readValue = 0;
		for(int i=0; i<4; i++) {
			readValue += (digitalRead(ID::binaryPins[i]) == HIGH ? 0 : 1) << i;
		}

		// offset by 1 (0 is the host)
		readValue += 1;

		{
			char message[64];
			sprintf(message, "Board ID : %d", (int) readValue);
			log(LogLevel::Status, message);
		}

		this->value = (Value) readValue;
	}
	
	//---------
	void
	ID::update()
	{
		// For now we presume 1 byte values
		serialID.write(this->value);

		// Check for incoming IDs 
		while(serialID.available()) {
			uint8_t previousID;
		
			auto bytesRead = serialID.readBytes(&previousID, 1);
			
			// check we received a valid ID
			// (otherwise might be a spurious packet, especially `\0`)
			if(previousID >= ID_PORTAL_MIN && previousID < ID_PORTAL_MAX) {
				this->value = previousID + 1;
				this->markNewID = true;

				{
					char message[64];
					sprintf(message, "New ID : %d", (int) this->value);
					log(LogLevel::Status, message);
				}
			}

		}
		
		// New ID this frame flag
		{
			this->isIDNewThisFrame = this->markNewID;
			this->markNewID = false;
		}
	}

	//---------
	ID::Value
	ID::get() const
	{
		return this->value;
	}

	//---------
	bool
	ID::getIsIDNewThisFrame() const
	{
		return this->isIDNewThisFrame;
	}
}

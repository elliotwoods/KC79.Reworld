#include "ID.h"
#include <Arduino.h>
#include "Log.h"

HardwareSerial serialID(PB9, PB8);

namespace Modules {
	//----------
	ID::ID(const Config& config)
	: config(config) 
	{
		this->value = config.defaultValue;
	}

	//---------
	void
	ID::setup()
	{
		// set the pin modes
		for(size_t i=0; i<4; i++) {
			pinMode(config.binaryPins[i], INPUT_PULLUP);
		}

		// initialise the serial device
		serialID.begin(115200);

		// init from the board
		this->initFromBoard();
	}

	//----------
	void
	ID::initFromBoard()
	{
		// accumulate values from the pins
		int readValue = 0;
		for(int i=0; i<4; i++) {
			readValue += (digitalRead(this->config.binaryPins[i]) == HIGH ? 0 : 1) << i;
		}

		{
			char message[64];
			sprintf(message, "Board ID : %d", readValue);
			log(message);
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
		
			serialID.readBytes(&previousID, 1);
			this->value = previousID + 1;
			this->markNewID = true;

			{
				char message[64];
				sprintf(message, "New ID : %d", this->value);
				log(message);
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

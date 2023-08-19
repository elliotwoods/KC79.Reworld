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
		// Check for incoming IDs
		this->readIncomingID();
		
		// New ID this frame flag
		{
			this->isIDNewThisFrame = this->markNewID;
			this->markNewID = false;
		}

		// Print if new
		if(this->isIDNewThisFrame) {
			char message[64];
			sprintf(message, "New ID : %d", (int) this->value);
			log(LogLevel::Status, message);
		}

		// If new ID, then send ours out
		if(this->isIDNewThisFrame || millis() - this->lastSend > 1000) {
			this->sendIDToNext();
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

	//---------
	void
	ID::readIncomingID()
	{
		while(serialID.available()) {
			auto byte = serialID.read();
			if(byte == 0) {
				// try to process current buffer
				if(this->incomingBytes.size() == 4) {
					auto targetID = this->incomingBytes[0];
					if(targetID ^ 'C' == this->incomingBytes[1]
						&& targetID ^ 'R' == this->incomingBytes[2]
						&& targetID ^ 'C' == this->incomingBytes[3]) {
						
						this->value = targetID + 1;
						this->markNewID = true;
					}
				}
			}
			else {
				// add the current buffer
				this->incomingBytes.push_back(byte);

				// limit to 4 elements in the buffer
				while(this->incomingBytes.size() > 4) {
					this->incomingBytes.pop_front();
				}
			}
		}
	}

	//---------
	void
	ID::sendIDToNext()
	{
		this->lastSend = millis();
		serialID.write(this->value);
		serialID.write('C' ^ this->value);
		serialID.write('R' ^ this->value);
		serialID.write('C' ^ this->value);
	}
}

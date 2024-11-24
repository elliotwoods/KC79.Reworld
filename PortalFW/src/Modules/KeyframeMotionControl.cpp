#include "KeyframeMotionControl.h"
#include "App.h"

namespace Modules {
	//----------
	KeyframeMotionControl::KeyframeMotionControl()
	{

	}

	//----------
	const char *
	KeyframeMotionControl::getTypeName() const
	{
		return "KeyframeMotionControl";
	}

	//----------
	void
	KeyframeMotionControl::update()
	{
		auto now = millis();
		auto timeSinceLastKeyframe = now - this->lastTimestamp;

		// Check if data is stale
		{
			if(timeSinceLastKeyframe > this->keyframeLifetime) {
				this->active = false;
			}
		}

		if(!this->active) {
			// Not active, i.e. either:
			// * Not started yet / no keyframe message arrived
			// * Other motion control message arrived
			// * Data is stale
			return;
		}

		// Data is OK, apply filtered motion...
		for(uint8_t i=0; i<2; i++) {
			auto targetPosition = (int32_t) ((int64_t) this->keyframes[i].position + ((int64_t) this->keyframes[i].velocity * (int64_t)  timeSinceLastKeyframe) / (int64_t) 1000);
			Modules::App::X().getMotionControl(i)->setTargetPosition(targetPosition);
		}

		// Calling MotionControl::setTargetPosition(..) clears the keyframe active flag
		// so we set it again here.
		this->active = true;
	}

	//----------
	void
	KeyframeMotionControl::clear()
	{
		this->active = false;
	}

//#define DEBUG_KEYFRAME_RX
	//----------
	bool
	KeyframeMotionControl::processIncoming(Stream & stream)
	{
		// The incoming message is format:
		/*
		{
			"startIndex" : 1..N,
		 	"values" : [
				posA, posB, velA, velB
			]
		}
		*/

		auto ourID = Modules::App::X().id->get();

		// read map size
		{
			size_t mapSize;
			if(!msgpack::readMapSize(stream, mapSize)) {
#ifdef DEBUG_KEYFRAME_RX
				log(LogLevel::Error, this->getName(), "Can't read outer map");
#endif
				return false;
			}
			if(mapSize != 2) {
#ifdef DEBUG_KEYFRAME_RX
				log(LogLevel::Error, this->getName(), "Map size is wrong");
#endif
				return false;
			}
		}

		// read start index
		Modules::ID::Value blockStartIndex;
		{
			// "startIndex"
			{
				char data[100];
				size_t length;
				if(!msgpack::readString(stream, data, 100, length)) {
					return false;
				}
			}

			{
				if(!msgpack::readInt(stream, blockStartIndex)) {
#ifdef DEBUG_KEYFRAME_RX
					log(LogLevel::Error, this->getName(), "Failed to load block start index");
#endif
					return false;
				}
			}
		}

		// read size of block
		size_t blockSize;
		{
			// "values"
			{
				char data[100];
				size_t length;
				if(!msgpack::readString(stream, data, 100, length)) {
#ifdef DEBUG_KEYFRAME_RX
					log(LogLevel::Error, this->getName(), "Failed to load key");
#endif
					return false;
				}
			}

			// array header
			{
				if(!msgpack::readArraySize(stream, blockSize)) {
#ifdef DEBUG_KEYFRAME_RX
					log(LogLevel::Error, this->getName(), "Failed to load block size");
#endif
					return false;
				}
			}
		}

		auto blockEndIndex = blockStartIndex + blockSize;
#ifdef DEBUG_KEYFRAME_RX
		{
			char data[100];
			sprintf(data, "Block %d -> %d", blockStartIndex, blockEndIndex);
			log(LogLevel::Status, this->getName(), data);
		}
#endif

		if(ourID < blockStartIndex || ourID >= blockEndIndex) {
			// This data block is not for us. Jump to next packet
			return true;
		}

#ifdef DEBUG_KEYFRAME_RX
		log(LogLevel::Status, this->getName(), "Reading block");
#endif

		// We have to read the entire block
		for(size_t i=0; i<blockSize; i++) {
			// Read the inner array size
			size_t innerArraySize;
			if(!msgpack::readArraySize(stream, innerArraySize)) {
#ifdef DEBUG_KEYFRAME_RX
				log(LogLevel::Error, this->getName(), "Can't read inner array");
#endif
				return false;
			}

			Steps positionA, positionB;
			StepsPerSecond velocityA = 0, velocityB = 0;

			// Read the values out
			{
				if(innerArraySize >= 2) {
					// Read the positions
					if(!msgpack::readInt(stream, positionA)) {
#ifdef DEBUG_KEYFRAME_RX
						log(LogLevel::Error, this->getName(), "Can't read value 1");
#endif
						return false;
					}
					if(!msgpack::readInt(stream, positionB)) {
#ifdef DEBUG_KEYFRAME_RX
						log(LogLevel::Error, this->getName(), "Can't read value 2");
#endif
						return false;
					}

#ifdef DEBUG_KEYFRAME_RX
					{
						char message[100];
						sprintf(message, "%d, %d", positionA, positionB);
					}
#endif
				}

				if(innerArraySize == 4) {
					// Read the velocities
					if(!msgpack::readInt(stream, velocityA)) {
#ifdef DEBUG_KEYFRAME_RX
						log(LogLevel::Error, this->getName(), "Can't read value 3");
#endif
						return false;
					}
					if(!msgpack::readInt(stream, velocityB)) {
#ifdef DEBUG_KEYFRAME_RX
						log(LogLevel::Error, this->getName(), "Can't read value 4");
#endif
						return false;
					}

					// Enable velocity interpolation
					this->active = true;
				}
			}
			
			// Apply the values if the ID matches
			if(i + blockStartIndex == ourID) {
				this->keyframes[0].position = positionA;
				this->keyframes[1].position = positionB;
				this->keyframes[0].velocity = velocityA;
				this->keyframes[1].velocity = velocityB;

				auto wasActive = this->active;
				App::X().motionControlA->setTargetPosition(positionA);
				App::X().motionControlB->setTargetPosition(positionB);

				if(wasActive) {
					// This flag is cleared by setTargetPosition on the motionControl
					this->active = true;
				}
				this->lastTimestamp = millis();

				return true;
			}
		}

		return true;
	}
}
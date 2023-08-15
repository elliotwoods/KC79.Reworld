#include "pch_App.h"
#include "ModuleControl.h"

namespace Modules {
	//---------
	ModuleControl::ModuleControl(shared_ptr<RS485> rs485)
		: rs485(rs485)
	{

	}

	//---------
	string
		ModuleControl::getTypeName() const
	{
		return "ModuleControl";
	}

	//---------
	void
		ModuleControl::init()
	{
		this->onPopulateInspector += [this](ofxCvGui::InspectArguments& args) {
			this->populateInspector(args);
		};
	}

	//---------
	void
		ModuleControl::update()
	{
		// Update values if local cache of what has been sent is different from parameters
		{
			if (this->debug.cachedValues.motorDriverSettings.current
				!= this->parameters.debug.motorDriverSettings.current) {
				this->setCurrent(this->parameters.debug.targetID.get()
					, this->parameters.debug.motorDriverSettings.current.get());
			}

			if (this->debug.cachedValues.motorDriverSettings.microstepResolution
				!= this->parameters.debug.motorDriverSettings.microstepResolution) {
				this->setMicrostepResolution(this->parameters.debug.targetID.get()
					, this->parameters.debug.motorDriverSettings.microstepResolution.get());
			}
		}

		// Continuous motion
		{
			// Update range for continuous motion
			{
				if (this->parameters.debug.motionControl.continuousMotion.range.get()
					!= this->parameters.debug.motionControl.continuousMotion.position.getMax()) {
					this->parameters.debug.motionControl.continuousMotion.position.setMax(
						this->parameters.debug.motionControl.continuousMotion.range.get()
					);
				}
			}

			if (this->parameters.debug.motionControl.continuousMotion.enabled) {
				// Make it an int
				this->parameters.debug.motionControl.continuousMotion.position =
					floor(this->parameters.debug.motionControl.continuousMotion.position);

				if (this->debug.cachedValues.motionControl.continuousMove.position != this->parameters.debug.motionControl.continuousMotion.position) {
					this->move(this->parameters.debug.targetID
						, Axis::A
						, this->parameters.debug.motionControl.continuousMotion.position
						, this->parameters.debug.motionControl.maxVelocity
						, this->parameters.debug.motionControl.acceleration
						, this->parameters.debug.motionControl.minVelocity);

					this->debug.cachedValues.motionControl.continuousMove.position = this->parameters.debug.motionControl.continuousMotion.position;
				}
			}
		}
		
	}

	//---------
	void
		ModuleControl::populateInspector(ofxCvGui::InspectArguments& args)
	{
		auto inspector = args.inspector;

		inspector->addParameterGroup(this->parameters);

		inspector->addButton("zeroCurrentPosition A", [this]() {
			this->zeroCurrentPosition(this->parameters.debug.targetID.get(), Axis::A);
			});

		inspector->addButton("zeroCurrentPosition B", [this]() {
			this->zeroCurrentPosition(this->parameters.debug.targetID.get(), Axis::B);
			});

		inspector->addButton("testRoutine A", [this]() {
			this->runTestRoutine(this->parameters.debug.targetID.get(), Axis::A);
			});

		inspector->addButton("testRoutine B", [this]() {
			this->runTestRoutine(this->parameters.debug.targetID.get(), Axis::B);
			});

		inspector->addButton("testTimer A", [this]() {
			this->runTestTimer(this->parameters.debug.targetID.get(), Axis::A);
			});

		inspector->addButton("testTimer B", [this]() {
			this->runTestTimer(this->parameters.debug.targetID.get(), Axis::B);
			});

		inspector->addButton("move A", [this]() {
			auto targetPosition = this->parameters.debug.motionControl.targetPosition;
			if (this->parameters.debug.motionControl.relativeMove) {
				targetPosition += this->parameters.debug.motionControl.movement;
				this->parameters.debug.motionControl.targetPosition.set(targetPosition);
			}

			this->move(this->parameters.debug.targetID.get()
				, Axis::A
				, this->parameters.debug.motionControl.targetPosition
				, this->parameters.debug.motionControl.maxVelocity
				, this->parameters.debug.motionControl.acceleration
				, this->parameters.debug.motionControl.minVelocity);
			});

		inspector->addButton("move B", [this]() {
			auto targetPosition = this->parameters.debug.motionControl.targetPosition;
			if (this->parameters.debug.motionControl.relativeMove) {
				targetPosition += this->parameters.debug.motionControl.movement;
				this->parameters.debug.motionControl.targetPosition.set(targetPosition);
			}

			this->move(this->parameters.debug.targetID.get()
				, Axis::B
				, this->parameters.debug.motionControl.targetPosition
				, this->parameters.debug.motionControl.maxVelocity
				, this->parameters.debug.motionControl.acceleration
				, this->parameters.debug.motionControl.minVelocity);
			});

		inspector->addButton("Measure backlash A", [this]() {
			this->measureBacklash(this->parameters.debug.targetID.get()
				, Axis::A);
			});

		inspector->addButton("Measure backlash B", [this]() {
			this->measureBacklash(this->parameters.debug.targetID.get()
				, Axis::B);
			});
	}

	//---------
	void
		ModuleControl::setCurrent(RS485::Target target, float value)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key = "motorDriverSettings";
				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "setCurrent";
					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_float(&packer, value);
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);

		this->debug.cachedValues.motorDriverSettings.current = value;
	}

	//---------
	void
		ModuleControl::setMicrostepResolution(RS485::Target target, int value)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key = "motorDriverSettings";
				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "setMicrostepResolution";
					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_uint8(&packer, log2(value));
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);

		this->debug.cachedValues.motorDriverSettings.microstepResolution = value;
	}

	//---------
	void
		ModuleControl::zeroCurrentPosition(RS485::Target target, Axis axis)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key;
				switch (axis) {
				case Axis::A:
					key = "motionControlA";
					break;
				case Axis::B:
					key = "motionControlB";
					break;
				default:
					break;
				}

				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "zeroCurrentPosition";
					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_nil(&packer);
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::runTestRoutine(RS485::Target target, Axis axis)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key;
				switch (axis) {
				case Axis::A:
					key = "motorDriverA";
					break;
				case Axis::B:
					key = "motorDriverB";
					break;
				default:
					break;
				}

				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "testRoutine";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_nil(&packer);
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::runTestTimer(RS485::Target target, Axis axis)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		auto period = this->parameters.debug.testTimer.period.get();
		auto count = this->parameters.debug.testTimer.count.get();

		if (this->parameters.debug.testTimer.normaliseParameters.get()) {
			auto microstepResolution = this->parameters.debug.motorDriverSettings.microstepResolution.get();
			period /= microstepResolution;
			count *= microstepResolution;
		}

		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key;
				switch (axis) {
				case Axis::A:
					key = "motorDriverA";
					break;
				case Axis::B:
					key = "motorDriverB";
					break;
				default:
					break;
				}

				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "testTimer";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					// Expecting an array [period_us, total_count]
					msgpack_pack_array(&packer, 2);
					msgpack_pack_uint32(&packer, period);
					msgpack_pack_uint32(&packer, count);
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::move(RS485::Target target
			, Axis axis
			, int32_t targetPosition
			, int32_t maxVelocity
			, int32_t acceleration
			, int32_t minVelocity)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		auto period = this->parameters.debug.testTimer.period.get();
		auto count = this->parameters.debug.testTimer.count.get();

		if (this->parameters.debug.testTimer.normaliseParameters.get()) {
			auto microstepResolution = this->parameters.debug.motorDriverSettings.microstepResolution.get();
			period /= microstepResolution;
			count *= microstepResolution;
		}

		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key;
				switch (axis) {
				case Axis::A:
					key = "motionControlA";
					break;
				case Axis::B:
					key = "motionControlB";
					break;
				default:
					break;
				}

				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "move";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					// Expecting an array [targetPosition, maxVelocity, acceleration, minVelocity]
					msgpack_pack_array(&packer, 4);
					msgpack_pack_int32(&packer, targetPosition);
					msgpack_pack_int32(&packer, maxVelocity);
					msgpack_pack_int32(&packer, acceleration);
					msgpack_pack_int32(&packer, minVelocity);
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::measureBacklash(RS485::Target target
			, Axis axis)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		auto timeout_s = this->parameters.debug.motionControl.measureBacklash.timeout_s.get();
		auto fastSpeed = this->parameters.debug.motionControl.measureBacklash.fastSpeed.get();
		auto slowSpeed = this->parameters.debug.motionControl.measureBacklash.slowSpeed.get();
		auto backOffDistance = this->parameters.debug.motionControl.measureBacklash.backOffDistance.get();
		auto debounceDistance = this->parameters.debug.motionControl.measureBacklash.debounceDistance.get();

		msgpack_sbuffer messageBuffer;
		msgpack_packer packer;
		msgpack_sbuffer_init(&messageBuffer);
		msgpack_packer_init(&packer
			, &messageBuffer
			, msgpack_sbuffer_write);

		{
			msgpack_pack_map(&packer, 1);

			// (0) - Key
			{
				string key;
				switch (axis) {
				case Axis::A:
					key = "motionControlA";
					break;
				case Axis::B:
					key = "motionControlB";
					break;
				default:
					break;
				}

				msgpack_pack_str(&packer, key.size());
				msgpack_pack_str_body(&packer, key.c_str(), key.size());
			}

			// (0) - Value
			{
				msgpack_pack_map(&packer, 1);

				// (1) - Key
				{
					string key = "measureBacklash";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					// Expecting an array [timeout_s, stepHalfCycleTime_us]
					msgpack_pack_array(&packer, 5);
					msgpack_pack_uint8(&packer, timeout_s);
					msgpack_pack_int32(&packer, fastSpeed);
					msgpack_pack_int32(&packer, slowSpeed);
					msgpack_pack_int32(&packer, backOffDistance);
					msgpack_pack_int32(&packer, debounceDistance);
				}
			}
		}

		auto header = rs485->makeHeader(target);

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}
}

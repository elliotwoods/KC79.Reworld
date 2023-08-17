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
			if (this->cachedValues.motorDriverSettings.current
				!= this->parameters.motorDriverSettings.current) {
				this->setCurrent(this->parameters.motorDriverSettings.current.get());
			}

			if (this->cachedValues.motorDriverSettings.microstepResolution
				!= this->parameters.motorDriverSettings.microstepResolution) {
				this->setMicrostepResolution(this->parameters.motorDriverSettings.microstepResolution.get());
			}
		}

		// Continuous motion
		{
			// Update range for continuous motion
			{
				if (this->parameters.motionControl.continuousMotion.range.get()
					!= this->parameters.motionControl.continuousMotion.position.getMax()) {
					this->parameters.motionControl.continuousMotion.position.setMax(
						this->parameters.motionControl.continuousMotion.range.get()
					);
				}
			}

			if (this->parameters.motionControl.continuousMotion.enabled) {
				// Make it an int
				this->parameters.motionControl.continuousMotion.position =
					floor(this->parameters.motionControl.continuousMotion.position);

				if (this->cachedValues.motionControl.continuousMove.position != this->parameters.motionControl.continuousMotion.position) {
					this->move(Axis::A
						, this->parameters.motionControl.continuousMotion.position
						, this->parameters.motionControl.maxVelocity
						, this->parameters.motionControl.acceleration
						, this->parameters.motionControl.minVelocity);

					this->cachedValues.motionControl.continuousMove.position = this->parameters.motionControl.continuousMotion.position;
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
			this->zeroCurrentPosition(Axis::A);
			});

		inspector->addButton("zeroCurrentPosition B", [this]() {
			this->zeroCurrentPosition(Axis::B);
			});

		inspector->addButton("testRoutine A", [this]() {
			this->runTestRoutine(Axis::A);
			});

		inspector->addButton("testRoutine B", [this]() {
			this->runTestRoutine(Axis::B);
			});

		inspector->addButton("testTimer A", [this]() {
			this->runTestTimer(Axis::A);
			});

		inspector->addButton("testTimer B", [this]() {
			this->runTestTimer(Axis::B);
			});

		inspector->addButton("move A", [this]() {
			auto targetPosition = this->parameters.motionControl.targetPosition;
			if (this->parameters.motionControl.relativeMove) {
				targetPosition += this->parameters.motionControl.movement;
				this->parameters.motionControl.targetPosition.set(targetPosition);
			}

			this->move(Axis::A
				, this->parameters.motionControl.targetPosition
				, this->parameters.motionControl.maxVelocity
				, this->parameters.motionControl.acceleration
				, this->parameters.motionControl.minVelocity);
			});

		inspector->addButton("move B", [this]() {
			auto targetPosition = this->parameters.motionControl.targetPosition;
			if (this->parameters.motionControl.relativeMove) {
				targetPosition += this->parameters.motionControl.movement;
				this->parameters.motionControl.targetPosition.set(targetPosition);
			}

			this->move(Axis::B
				, this->parameters.motionControl.targetPosition
				, this->parameters.motionControl.maxVelocity
				, this->parameters.motionControl.acceleration
				, this->parameters.motionControl.minVelocity);
			});

		inspector->addButton("Measure backlash A", [this]() {
			this->measureBacklash(Axis::A);
			});

		inspector->addButton("Measure backlash B", [this]() {
			this->measureBacklash(Axis::B);
			});

		inspector->addButton("Home A", [this]() {
			this->home(Axis::A);
			});

		inspector->addButton("Home B", [this]() {
			this->home(Axis::B);
			});
	}

	//---------
	void
		ModuleControl::setCurrent(float value)
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

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);

		this->cachedValues.motorDriverSettings.current = value;
	}

	//---------
	void
		ModuleControl::setMicrostepResolution(int value)
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

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);

		this->cachedValues.motorDriverSettings.microstepResolution = value;
	}

	//---------
	void
		ModuleControl::zeroCurrentPosition(Axis axis)
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

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::runTestRoutine(Axis axis)
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

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::runTestTimer(Axis axis)
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		auto period = this->parameters.testTimer.period.get();
		auto count = this->parameters.testTimer.count.get();

		if (this->parameters.testTimer.normaliseParameters.get()) {
			auto microstepResolution = this->parameters.motorDriverSettings.microstepResolution.get();
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

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::move(Axis axis
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

		auto period = this->parameters.testTimer.period.get();
		auto count = this->parameters.testTimer.count.get();

		if (this->parameters.testTimer.normaliseParameters.get()) {
			auto microstepResolution = this->parameters.motorDriverSettings.microstepResolution.get();
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

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::move(Axis axis
			, int32_t targetPosition)		
	{
		auto rs485 = this->rs485.lock();
		if (!rs485) {
			ofLogError("No RS485");
			return;
		}

		auto period = this->parameters.testTimer.period.get();
		auto count = this->parameters.testTimer.count.get();

		if (this->parameters.testTimer.normaliseParameters.get()) {
			auto microstepResolution = this->parameters.motorDriverSettings.microstepResolution.get();
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
					// Int only means no motion profile is sent
					msgpack_pack_int32(&packer, targetPosition);
				}
			}
		}

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::serialiseMeasureSettings(msgpack_packer& packer)
	{
		auto timeout_s = this->parameters.motionControl.measureSettings.timeout_s.get();
		auto slowSpeed = this->parameters.motionControl.measureSettings.slowSpeed.get();
		auto backOffDistance = this->parameters.motionControl.measureSettings.backOffDistance.get();
		auto debounceDistance = this->parameters.motionControl.measureSettings.debounceDistance.get();

		// Expecting an array [timeout_s, etc]
		msgpack_pack_array(&packer, 4);
		msgpack_pack_uint8(&packer, timeout_s);
		msgpack_pack_int32(&packer, slowSpeed);
		msgpack_pack_int32(&packer, backOffDistance);
		msgpack_pack_int32(&packer, debounceDistance);
	}

	//---------
	void
		ModuleControl::measureBacklash(Axis axis)
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
					string key = "measureBacklash";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					this->serialiseMeasureSettings(packer);
				}
			}
		}

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::home(Axis axis)
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
					string key = "home";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					this->serialiseMeasureSettings(packer);
				}
			}
		}

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::deinitTimer(Axis axis)
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
					string key = "deinitTimer";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_nil(&packer);
				}
			}
		}

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

	//---------
	void
		ModuleControl::initTimer(Axis axis)
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
					string key = "initTimer";

					msgpack_pack_str(&packer, key.size());
					msgpack_pack_str_body(&packer, key.c_str(), key.size());
				}

				// (1) - Value
				{
					msgpack_pack_nil(&packer);
				}
			}
		}

		auto header = rs485->makeHeader(this->parameters.targetID.get());

		vector<uint8_t> body;
		body.assign((uint8_t*)(messageBuffer.data)
			, (uint8_t*)(messageBuffer.data + messageBuffer.size));

		rs485->transmitHeaderAndBody(header, body);
		msgpack_sbuffer_destroy(&messageBuffer);
	}

}

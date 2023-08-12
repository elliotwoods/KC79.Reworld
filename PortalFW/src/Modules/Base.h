#pragma once

#include <Stream.h>
#include "msgpack.hpp"

namespace Modules {
	class Base : public msgpack::Messaging {
	public:
		virtual void setup() {};
		virtual void update() {};

		virtual void reportStatus(msgpack::Serializer&);
	protected:
		virtual bool processIncomingByKey(const char * key, Stream& stream);
	};
}
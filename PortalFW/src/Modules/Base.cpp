#include "Base.h"

namespace Modules {
	//----------
	void
	Base::reportStatus(msgpack::Serializer& serializer)
	{
		serializer << (uint8_t) 0;
	}

	//----------
	bool
	Base::processIncomingByKey(const char * key, Stream&)
	{
		return false;
	}
}
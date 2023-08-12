#include "Base.h"

namespace Modules {
	//----------
	void
	Base::reportStatus(msgpack::Serializer&)
	{

	}

	//----------
	bool
	Base::processIncomingByKey(const char * key, Stream&)
	{
		return false;
	}
}
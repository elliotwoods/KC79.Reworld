#include "Base.h"

namespace Modules {
	//----------
	const char *
	Base::getName() const
	{
		return this->getTypeName();
	}

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
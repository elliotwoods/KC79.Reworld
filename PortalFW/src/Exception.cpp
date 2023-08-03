#include "Exception.h"

//-----------
const char *
Exception::what() const
{
	return this->message.c_str()
}
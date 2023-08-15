#include "Exception.h"

//-----------
Exception::Exception(const char* message)
: message(message)
, noException(false)
{
	
}

//-----------
Exception
Exception::None()
{
	return Exception();
}

//-----------
Exception
Exception::MessageFormatError()
{
	return Exception("MessageFormatError");
}

//-----------
Exception
Exception::Timeout()
{
	return Exception("Timeout");
}

//-----------
const char *
Exception::what() const
{
	return this->message.c_str();
}

//-----------
Exception::operator bool() const
{
	return !this->noException;
}

//-----------
Exception::Exception()
: noException(true)
{

}
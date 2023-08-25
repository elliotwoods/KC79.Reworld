#include "Exception.h"
#include "Logger.h"

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
Exception
Exception::Escape()
{
	return Exception("Escape");
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
bool
Exception::report() const
{
	if(!this->noException) {
		log(LogLevel::Error, this->message.c_str());
		return true;
	}
	else {
		return false;
	}
}

//-----------
Exception::Exception()
: noException(true)
{

}
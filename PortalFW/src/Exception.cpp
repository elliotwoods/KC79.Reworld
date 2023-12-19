#include "Exception.h"
#include "Logger.h"
#include "Modules/RS485.h"

//-----------
Exception::Exception(const char* message)
: message(message)
, noException(false)
{
	
}

//-----------
Exception::Exception(const Exception& other)
: message(other.message)
, noException(other.noException)
{
	
}

//-----------
Exception &
Exception::operator=(const Exception& other)
{
	this->noException = other.noException;
	this->message = other.message;

	return * this;
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
	// Message format error can occur on message collision
	// In this case the router should just look out for lack of ACK
	Modules::RS485::noACKRequired();

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
Exception
Exception::SwitchNotSeen()
{
	return Exception("Switch not seen");
}

//-----------
Exception
Exception::SwitchSeen()
{
	return Exception("Switch seen");
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
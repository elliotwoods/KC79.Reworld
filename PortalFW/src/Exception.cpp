#include "Exception.h"
#include "Logger.h"
#include "Modules/RS485.h"

//-----------
Exception::Exception(const char* module, const char* message)
: module(module)
, message(message)
, noException(false)
{

}

//-----------
Exception::Exception(const Exception& other)
: module(other.module)
, message(other.message)
, noException(other.noException)
{

}

//-----------
Exception &
Exception::operator=(const Exception& other)
{
	this->noException = other.noException;
	this->module = other.module;
	this->message = other.message;

	return * this;
}

//-----------
void
Exception::setModuleName(const char * module)
{
	this->module = module;
}

//-----------
Exception
Exception::None()
{
	return Exception();
}

//-----------
Exception
Exception::MessageFormatError(const char * module)
{
	// Message format error can occur on message collision
	// In this case the router should just look out for lack of ACK
	Modules::RS485::noACKRequired();

	return Exception(module, "MessageFormatError");
}

//-----------
Exception
Exception::Timeout(const char * module)
{
	return Exception(module, "Timeout");
}

//-----------
Exception
Exception::Escape(const char * module)
{
	return Exception(module, "Escape");
}

//-----------
Exception
Exception::SwitchNotSeen(const char * module)
{
	return Exception(module, "Switch not seen");
}

//-----------
Exception
Exception::SwitchSeen(const char * module)
{
	return Exception(module, "Switch seen");
}


//-----------
const char *
Exception::what() const
{
	return this->message.c_str();
}

//-----------
const std::string&
Exception::getModule() const
{
	return this->module;
}

//-----------
const std::string&
Exception::getMessage() const
{
	return this->message;
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
		log(LogLevel::Error, this->module.c_str(), this->message.c_str());
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
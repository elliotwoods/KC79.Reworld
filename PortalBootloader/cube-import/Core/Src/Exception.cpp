#include "Exception.hpp"

//-----------
Exception::Exception(const char* message)
: message(message)
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
	return Exception("Message format invalid");
}

//-----------
const char *
Exception::what() const
{
	return this->message;
}

//-----------
Exception::operator bool() const
{
	return this->message != nullptr;
}

//-----------
Exception::Exception()
: message(nullptr)
{

}

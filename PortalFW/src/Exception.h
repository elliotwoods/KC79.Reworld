#pragma once

#include <string>

struct Exception {
	Exception(const char *);

	static Exception None();

	static Exception MessageFormatError();
	static Exception Timeout();

	const char * what() const;
	operator bool() const;
private:
	Exception();
	bool noException;
	const std::string message;
};
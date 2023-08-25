#pragma once

#include <string>

struct Exception {
	Exception(const char *);

	static Exception None();

	static Exception MessageFormatError();
	static Exception Timeout();
	static Exception Escape();

	const char * what() const;
	operator bool() const;

	bool report() const;
private:
	Exception();
	bool noException;
	const std::string message;
};
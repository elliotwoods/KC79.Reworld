#pragma once

#include <string>

struct Exception {
	Exception(const char *);
	static Exception None();
	const char * what() const;
	operator bool() const;
private:
	Exception();
	bool noException;
	const std::string message;
};
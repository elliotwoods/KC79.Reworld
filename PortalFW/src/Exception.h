#pragma once

#include <string>

struct Exception {
	Exception(const char *);
	Exception(const Exception&);
	Exception & operator=(const Exception&);
	
	static Exception None();

	static Exception MessageFormatError();
	static Exception Timeout();
	static Exception Escape();
	static Exception SwitchNotSeen();
	static Exception SwitchSeen();

	const char * what() const;
	operator bool() const;

	bool report() const;
private:
	Exception();
	bool noException;
	std::string message;
};
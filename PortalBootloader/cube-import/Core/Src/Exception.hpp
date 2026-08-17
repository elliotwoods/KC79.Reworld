#pragma once

struct Exception {
	Exception(const char *);

	static Exception None();
	static Exception MessageFormatError();

	const char * what() const;
	operator bool() const;
private:
	Exception();
	const char * message;
};

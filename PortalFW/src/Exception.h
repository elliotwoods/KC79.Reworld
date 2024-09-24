#pragma once

#include <string>

struct Exception {
	Exception(const char * module, const char * message);
	Exception(const Exception&);
	Exception & operator=(const Exception&);

	void setModuleName(const char * module);
	
	static Exception None();

	static Exception MessageFormatError(const char * module);
	static Exception Timeout(const char * module);
	static Exception Escape(const char * module);
	static Exception SwitchNotSeen(const char * module);
	static Exception SwitchSeen(const char * module);

	const char * what() const;
	
	const std::string& getModule() const;
	const std::string& getMessage() const;

	operator bool() const;

	bool report() const;
private:
	Exception();
	bool noException;
	std::string module;
	std::string message;
};
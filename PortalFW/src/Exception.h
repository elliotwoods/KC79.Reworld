#include <string>

struct Exception {
	Exception(const char *);
	const char * what() const;
private:
	const std::string message;
}
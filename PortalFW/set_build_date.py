
Import("env")

print("Current CLI targets", COMMAND_LINE_TARGETS)
print("Current Build targets", BUILD_TARGETS)

from datetime import datetime

now = datetime.now()
time_string = now.strftime("%Y-%m-%d_%H.%M")

version_string = "Portal v%s" % time_string;
define_string = '#define PORTAL_VERSION_STRING "' + version_string + '"'
print(define_string)

file = open("src/Version.h", "w")
file.write(define_string)
file.write("\n")

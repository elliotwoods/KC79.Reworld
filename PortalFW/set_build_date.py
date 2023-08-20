from datetime import datetime

now = datetime.now()
time_string = now.strftime("%Y-%m-%d_%H:%M")

version_string = "Portal v%s" % time_string;
print('-D PORTAL_VERSION_STRING="\\"' + version_string + '\\""')
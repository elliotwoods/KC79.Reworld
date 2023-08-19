from datetime import datetime
Import("env")
now = datetime.now()
time_string = now.strftime("%Y0%n0%d-%H:%M")

env.Append(
    CPPDEFINES=[("VERSION_STRING", "PORTALv" % time_string)],
)
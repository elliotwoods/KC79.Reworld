Import("env")
offset = 0x6000 # from 0x8000000
env.Append(
    CPPDEFINES=[("VECT_TAB_OFFSET", "%s" % hex(offset))],
)
# remove old 0-offset, inject new one
linkflags = env["LINKFLAGS"]
linkflags = [x for x in linkflags if not str(x).startswith("-Wl,--defsym=LD_FLASH_OFFSET=")]
linkflags.append("-Wl,--defsym=LD_FLASH_OFFSET=%s" % hex(offset))
env["LINKFLAGS"] = linkflags
Import("env")
offset = 0x6000 # from 0x8000000
# Firmware ends before the final three 2 KiB pages. Those pages are durable provisioning
# identity and alternating settings journals and are never part of an application image.
size = 0x18800 # 0x08006000..0x0801E800 = 98 KiB
env.Append(
    CPPDEFINES=[("VECT_TAB_OFFSET", "%s" % hex(offset))],
)
# remove old 0-offset, inject new one
linkflags = env["LINKFLAGS"]
linkflags = [x for x in linkflags if not str(x).startswith("-Wl,--defsym=LD_FLASH_OFFSET=")]
linkflags = [x for x in linkflags if not str(x).startswith("-Wl,--defsym=LD_FLASH_SIZE=")]
linkflags.append("-Wl,--defsym=LD_FLASH_OFFSET=%s" % hex(offset))
linkflags.append("-Wl,--defsym=LD_FLASH_SIZE=%s" % hex(size))
env["LINKFLAGS"] = linkflags

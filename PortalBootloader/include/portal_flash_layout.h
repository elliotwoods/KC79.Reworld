/* The KC79 Portal flash and RAM map, and the two structures that cross the bootloader/application
 * boundary. This file is the single definition of all of it.
 *
 * Before it existed the same numbers were written out four times -- in the bootloader's
 * `constants.h`, in `PortalFW/set_bank2.py`, in `portal-swd`'s `addr` module, and in
 * `tools/firmware.mjs` -- and nothing checked that they still agreed. They are load-bearing in the
 * way that a wrong one destroys a board's provisioning identity rather than failing a build, so
 * agreement cannot rest on remembering to edit four files.
 *
 * Three of those four readers are not C compilers, so this file is also parsed as *text*:
 *
 *   - `RouterRS/crates/router-proto/src/layout.rs` (include_str! + a test per constant)
 *   - `PortalFlasher/crates/portal-swd/src/lib.rs` (the same)
 *   - `tools/firmware.mjs` and `PortalFW/layout_check.py` (regex)
 *
 * That is why every value below is a bare hexadecimal literal on a `#define` line, with no
 * arithmetic and no reference to another macro. `(24U * 0x400U)` -- which is what `constants.h`
 * used to say -- would have to be evaluated by four parsers instead of read by them. Derived
 * values belong in the reader, not here.
 */
#pragma once

/* ---- Flash -------------------------------------------------------------------------------- */

/* The 128 kB part, in 64 pages of 2 kB. */
#define PORTAL_FLASH_BASE               0x08000000
#define PORTAL_FLASH_END                0x08020000
#define PORTAL_FLASH_PAGE_BYTES         0x800

/* Bootloader v6 occupies pages 0-7. The size is enforced by `PortalBootloader/tools/size_gate.py`
 * and by the linker script's FLASH LENGTH; nothing derives it from the application base, because
 * during the transition the two are deliberately not adjacent. */
#define PORTAL_BOOTLOADER_BYTES         0x4000
/* What v4 and v5 occupied, and therefore where a board that has not yet been updated expects its
 * application to start. Kept because a fielded fleet contains both. */
#define PORTAL_BOOTLOADER_BYTES_LEGACY  0x6000

/* The application, pages 8-60 (108,544 bytes). */
#define PORTAL_APP_BASE                 0x08004000
/* Where v4/v5 boards run their application. A v6 bootloader will still start an image here when
 * the new base is blank, which is what makes replacing a fielded bootloader a survivable
 * single step rather than a flag day. */
#define PORTAL_APP_BASE_LEGACY          0x08006000
/* One past the last application byte: the first of the three durable pages. No firmware image may
 * reach it, no erase may include it. This is the address whose absence from the fielded v4
 * bootloader's erase loop destroys a board's serial number. */
#define PORTAL_APP_END                  0x0801E800

/* The three durable pages: an append-only identity journal and an A/B settings journal. Written
 * over SWD by PortalFlasher and, for settings, by the application itself. Never by the
 * bootloader. */
#define PORTAL_PERSIST_IDENTITY         0x0801E800
#define PORTAL_PERSIST_SETTINGS_A       0x0801F000
#define PORTAL_PERSIST_SETTINGS_B       0x0801F800

/* ---- RAM ---------------------------------------------------------------------------------- */

#define PORTAL_RAM_BASE                 0x20000000
#define PORTAL_RAM_END                  0x20009000

/* The handoff block sits in the top 32 bytes of SRAM and is excluded from both images' linker RAM
 * (`LENGTH = 36K - 32`), so neither one's stack or .bss can reach it. It is never initialised by
 * startup code: that is the point -- it has to survive the reset that carries it. */
#define PORTAL_HANDOFF_ADDR             0x20008FE0
#define PORTAL_HANDOFF_BYTES            0x20
/* "K79H" little-endian. */
#define PORTAL_HANDOFF_MAGIC            0x4839374B
#define PORTAL_HANDOFF_VERSION          0x1

/* ---- Application descriptor --------------------------------------------------------------- */

/* Offset of the descriptor from the application's base address.
 *
 * The G070's vector table is 46 entries (0xB8 bytes), so 0xC0 is the first 16-byte-aligned address
 * past it. It is a *fixed* offset rather than "wherever the linker puts it after the vectors":
 * the bootloader has to find it in an image it did not build, and orphan-section placement is not
 * a contract. `PortalFW/ldscript_app.ld` places it at exactly this offset and asserts the vector
 * table has not grown into it. */
#define PORTAL_APP_DESCRIPTOR_OFFSET    0xC0
#define PORTAL_APP_DESCRIPTOR_BYTES     0x38
#define PORTAL_APP_DESCRIPTOR_MAGIC     "KC79APP1"
#define PORTAL_APP_VERSION_BYTES        0x28

/* ---- Bootloader control plane -------------------------------------------------------------- */

/* Protocol version reported by `{"bl": {"q": "status"}}`. */
#define PORTAL_BL_PROTO_VERSION         0x6
/* Largest firmware-frame payload the bootloader will accept, in bytes. Bounded by the msgpack
 * library's 256-byte COBS decode buffer rather than by anything on the host side. */
#define PORTAL_BL_CHUNK_MAX             0x100
/* Flash is programmed a double-word at a time, so every offset and length is a multiple of this
 * and the received-granule bitmap counts in these units. */
#define PORTAL_FLASH_GRANULE            0x8

/* ---- Device ------------------------------------------------------------------------------- */

/* 96-bit unique id in system memory. Needs no peripheral clock. */
#define PORTAL_UID_BASE                 0x1FFF7590

#ifndef __ASSEMBLER__

#include <stdint.h>

#ifdef __cplusplus
#define PORTAL_STATIC_ASSERT(condition, message) static_assert(condition, message)
#else
#define PORTAL_STATIC_ASSERT(condition, message) _Static_assert(condition, message)
#endif

/* What the application asks the bootloader for when it resets into it.
 *
 * `PORTAL_HANDOFF_REQUEST_STAY` is the difference between a board that can be addressed during an
 * update and one that cannot: without it the bootloader has no idea what its own bus address is
 * (it has no ID daisy-chain of its own) and no reason to stay resident for more than the legacy
 * 3 seconds.
 *
 * `PORTAL_HANDOFF_REQUEST_RUN_NOW` is internal to the bootloader. It jumps to an application by
 * writing this and resetting, rather than by tearing down its own clocks and peripherals in
 * place: after a reset every peripheral is already in the state the application's own init code
 * expects, which removes the entire class of bug where `HAL_RCC_OscConfig` refuses to reconfigure
 * a PLL that is currently driving SYSCLK. */
enum {
	PORTAL_HANDOFF_REQUEST_NONE = 0,
	PORTAL_HANDOFF_REQUEST_STAY = 1,
	PORTAL_HANDOFF_REQUEST_RUN_NOW = 2
};

enum {
	PORTAL_HANDOFF_FLAG_SERIAL_VALID = 1
};

/* 32 bytes at PORTAL_HANDOFF_ADDR, little-endian throughout.
 *
 * Hand-laid out rather than left to the compiler for the same reason the persistent records are:
 * it is read by code built from a different toolchain with different flags, and by host tests in
 * Rust. `crc32c` covers bytes 0..27 and is what distinguishes a real block from whatever the last
 * program left in that RAM. */
typedef struct {
	uint32_t magic;        /* +0x00  PORTAL_HANDOFF_MAGIC */
	uint8_t version;       /* +0x04  PORTAL_HANDOFF_VERSION */
	uint8_t request;       /* +0x05  PORTAL_HANDOFF_REQUEST_* */
	int8_t id;             /* +0x06  RS485 address, <= 0 when unknown */
	uint8_t flags;         /* +0x07  PORTAL_HANDOFF_FLAG_* */
	uint32_t serial;       /* +0x08  provisioning serial, valid per flags */
	uint32_t arg0;         /* +0x0C  RUN_NOW: the base address to start */
	uint32_t reserved[3];  /* +0x10 */
	uint32_t crc32c;       /* +0x1C  CRC-32C over bytes 0..27 */
} portal_handoff_t;

PORTAL_STATIC_ASSERT(sizeof(portal_handoff_t) == PORTAL_HANDOFF_BYTES,
	"the handoff block must be exactly 32 bytes: it is written by one image and read by another");

/* 56 bytes at (application base + PORTAL_APP_DESCRIPTOR_OFFSET).
 *
 * `app_base` is the whole point. An application image linked for 0x08006000 and an application
 * image linked for 0x08004000 are both plausible-looking Cortex-M images with a stack pointer in
 * SRAM and a reset vector inside the application bank; nothing else in the image says which bank
 * it was built for, and starting the wrong one produces a hard fault at some later, unrelated
 * absolute address. This field says it outright, so the bootloader refuses instead of guessing
 * and the host tooling can pick the right build. */
typedef struct {
	char magic[8];         /* +0x00  PORTAL_APP_DESCRIPTOR_MAGIC, not NUL-terminated */
	uint32_t app_base;     /* +0x08  the address this image was linked for */
	uint32_t flags;        /* +0x0C  reserved, 0 */
	char version[PORTAL_APP_VERSION_BYTES]; /* +0x10  PORTAL_VERSION_STRING, NUL-padded */
} portal_app_descriptor_t;

PORTAL_STATIC_ASSERT(sizeof(portal_app_descriptor_t) == PORTAL_APP_DESCRIPTOR_BYTES,
	"the application descriptor must be exactly 56 bytes");

#endif /* __ASSEMBLER__ */

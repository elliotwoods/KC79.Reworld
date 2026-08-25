// The C++ runtime hooks a freestanding image has to answer for itself.
//
// # Why these exist
//
// A statically-constructed object with a destructor makes the compiler emit a registration call so
// the destructor runs at exit. There is no exit here -- this image runs until it resets the part
// or jumps to an application -- and the registration is not free: the default `__cxa_atexit` keeps
// its list on the heap, so one static `COBSRWStream` drags in `malloc`, `_sbrk`, `__malloc_lock`
// and `__register_exitproc`, about 600 bytes of newlib for a destructor that can never run.
//
// (`-fno-use-cxa-atexit` makes this worse rather than better: it substitutes plain `atexit`, which
// reaches the same allocator by a longer route. The size gate catches either.)
//
// Stubbing the registration is the whole fix. Returning 0 tells the compiler the destructor was
// recorded; nothing ever calls it, which is correct.

#include "stm32g0xx.h"

extern "C" {

	/// Static destructor registration. Deliberately does nothing and succeeds.
	///
	/// `__dso_handle` is deliberately *not* defined here: GCC's `crtbegin.o` already provides it,
	/// and a second definition is a link error rather than an override.
	int __cxa_atexit(void (*)(void *), void *, void *)
	{
		return 0;
	}

	/// A pure virtual called through a partially-constructed object. Unreachable here -- the only
	/// virtual class in the image is `msgpack::Stream`, and every instance is fully constructed
	/// before use -- but the symbol has to exist, and spinning is a better answer than a jump
	/// through whatever happened to be at address zero.
	void __cxa_pure_virtual()
	{
		while(true) {
		}
	}

	/// Called if a `noreturn` function returns, and by the newlib pieces that still assume an exit
	/// exists. Resetting is the only sensible response: something has gone wrong in a way this
	/// image cannot describe, and a bootloader that resets is one that can be talked to again.
	void _exit(int)
	{
		NVIC_SystemReset();
		while(true) {
		}
	}

	/// The heap does not exist. `_Min_Heap_Size` is zero in the linker script and nothing here
	/// allocates; this is the definition that makes that a link-time fact rather than a hope.
	void * _sbrk(int)
	{
		return (void *) -1;
	}

} // extern "C"

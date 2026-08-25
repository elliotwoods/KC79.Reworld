/* Linker script for the KC79 Portal application, STM32G070RBTx.
 *
 * A copy of the stm32duino variant script for this part, with three changes. Each one exists
 * because the original silently permits something that destroys a board.
 *
 *   RAM is shortened by 32 bytes.  The top of SRAM holds the handoff block: the note this image
 *                                 leaves for the bootloader saying which RS485 address it answers
 *                                 on and whether an update is expected. It survives a reset only
 *                                 because nothing else uses it, and the stack growing down into it
 *                                 is exactly what would.
 *
 *   FLASH is bounded at 0x0801E800. `board_upload.maximum_size` sets `LD_MAX_SIZE`, and it is set
 *                                 to stop at the first of the three durable pages -- provisioning
 *                                 identity and the A/B settings journals. Before this, the script
 *                                 took `LD_MAX_SIZE` from `upload.maximum_size` (the whole 128 kB
 *                                 part) and `set_bank2.py`'s `LD_FLASH_SIZE` defsym, which this
 *                                 script never referenced, did nothing at all. An application that
 *                                 grew past 98 kB therefore linked cleanly on top of the board's
 *                                 serial number, and the next settings write would have destroyed
 *                                 whatever code was there.
 *
 *   .app_descriptor is placed at a fixed offset. See src/AppDescriptor.cpp for why an image has to
 *                                 state which bank it was built for, and why "somewhere after the
 *                                 vector table" is not good enough.
 *
 * `LD_APP_DESCRIPTOR_OFFSET` is injected by layout_check.py from the shared layout header.
 */

/* Entry Point */
ENTRY(Reset_Handler)

/* Highest address of the user mode stack */
_estack = ORIGIN(RAM) + LENGTH(RAM); /* end of "RAM" Ram type memory */

_Min_Heap_Size = 0x200 ; /* required amount of heap */
_Min_Stack_Size = 0x400 ; /* required amount of stack */

/* Memories definition */
MEMORY
{
  RAM    (xrw)    : ORIGIN = 0x20000000,   LENGTH = LD_MAX_DATA_SIZE
  FLASH    (rx)    : ORIGIN = 0x8000000 + LD_FLASH_OFFSET, LENGTH = LD_MAX_SIZE - LD_FLASH_OFFSET
}

/* Sections */
SECTIONS
{
  /* The startup code into "FLASH" Rom type memory */
  .isr_vector :
  {
    . = ALIGN(4);
    KEEP(*(.isr_vector)) /* Startup code */
    . = ALIGN(4);
  } >FLASH

  /* The bootloader reads this out of an image it did not build, so the offset is a contract
   * rather than a consequence of link order. The assert is the guard: the G070's table is 46
   * entries (0xB8), and a part with more interrupts would run into the descriptor silently. */
  ASSERT(SIZEOF(.isr_vector) <= LD_APP_DESCRIPTOR_OFFSET,
    "the vector table has grown into the application descriptor's fixed offset")

  .app_descriptor ORIGIN(FLASH) + LD_APP_DESCRIPTOR_OFFSET :
  {
    KEEP(*(.app_descriptor))
  } >FLASH

  /* The program code and other data into "FLASH" Rom type memory */
  .text :
  {
    . = ALIGN(4);
    *(.text)           /* .text sections (code) */
    *(.text*)          /* .text* sections (code) */
    *(.glue_7)         /* glue arm to thumb code */
    *(.glue_7t)        /* glue thumb to arm code */
    *(.eh_frame)

    KEEP (*(.init))
    KEEP (*(.fini))

    . = ALIGN(4);
    _etext = .;        /* define a global symbols at end of code */
  } >FLASH

  /* Constant data into "FLASH" Rom type memory */
  .rodata :
  {
    . = ALIGN(4);
    *(.rodata)         /* .rodata sections (constants, strings, etc.) */
    *(.rodata*)        /* .rodata* sections (constants, strings, etc.) */
    . = ALIGN(4);
  } >FLASH

  .ARM.extab   : {
    . = ALIGN(4);
    *(.ARM.extab* .gnu.linkonce.armextab.*)
    . = ALIGN(4);
  } >FLASH

  .ARM : {
    . = ALIGN(4);
    __exidx_start = .;
    *(.ARM.exidx*)
    __exidx_end = .;
    . = ALIGN(4);
  } >FLASH

  .preinit_array     :
  {
    . = ALIGN(4);
    PROVIDE_HIDDEN (__preinit_array_start = .);
    KEEP (*(.preinit_array*))
    PROVIDE_HIDDEN (__preinit_array_end = .);
    . = ALIGN(4);
  } >FLASH

  .init_array :
  {
    . = ALIGN(4);
    PROVIDE_HIDDEN (__init_array_start = .);
    KEEP (*(SORT(.init_array.*)))
    KEEP (*(.init_array*))
    PROVIDE_HIDDEN (__init_array_end = .);
    . = ALIGN(4);
  } >FLASH

  .fini_array :
  {
    . = ALIGN(4);
    PROVIDE_HIDDEN (__fini_array_start = .);
    KEEP (*(SORT(.fini_array.*)))
    KEEP (*(.fini_array*))
    PROVIDE_HIDDEN (__fini_array_end = .);
    . = ALIGN(4);
  } >FLASH

  /* Used by the startup to initialize data */
  _sidata = LOADADDR(.data);

  /* Initialized data sections into "RAM" Ram type memory */
  .data :
  {
    . = ALIGN(4);
    _sdata = .;        /* create a global symbol at data start */
    *(.data)           /* .data sections */
    *(.data*)          /* .data* sections */
    *(.RamFunc)        /* .RamFunc sections */
    *(.RamFunc*)       /* .RamFunc* sections */

    . = ALIGN(4);
    _edata = .;        /* define a global symbol at data end */

  } >RAM AT> FLASH

  /* Uninitialized data section into "RAM" Ram type memory */
  . = ALIGN(4);
  .bss :
  {
    /* This is used by the startup in order to initialize the .bss section */
    _sbss = .;         /* define a global symbol at bss start */
    __bss_start__ = _sbss;
    *(.bss)
    *(.bss*)
    *(COMMON)

    . = ALIGN(4);
    _ebss = .;         /* define a global symbol at bss end */
    __bss_end__ = _ebss;
  } >RAM

  /* User_heap_stack section, used to check that there is enough "RAM" Ram  type memory left */
  ._user_heap_stack :
  {
    . = ALIGN(8);
    PROVIDE ( end = . );
    PROVIDE ( _end = . );
    . = . + _Min_Heap_Size;
    . = . + _Min_Stack_Size;
    . = ALIGN(8);
  } >RAM

  /* Remove information from the compiler libraries */
  /DISCARD/ :
  {
    libc.a ( * )
    libm.a ( * )
    libgcc.a ( * )
  }

  .ARM.attributes 0 : { *(.ARM.attributes) }
}

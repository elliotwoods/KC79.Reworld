/* Linker script for the KC79 Portal RS485 bootloader, STM32G070RBTx.
 *
 * Derived from the STM32CubeIDE original. Four things differ from it, and each one is load-bearing:
 *
 *   FLASH is 16K, not 28K.  The imported script declared 28 kB for a bank the firmware itself
 *                           treated as 24 kB, so an image between the two linked cleanly and
 *                           silently overlapped the application. The bank is now 16 kB and the
 *                           length says so, with `tools/size_gate.py` checking the built image
 *                           against the same number from the other side.
 *
 *   RAM is 36K - 32.        The top 32 bytes are the handoff block, which both this image and the
 *                           application must leave alone -- it carries a board's identity across
 *                           the reset between them. Shortening RAM is how the stack is kept out of
 *                           it; `_estack` lands at 0x20008FE0.
 *
 *   .ram_vectors first.     VTOR's TBLOFF field is bits [31:8], so a relocated vector table has to
 *                           be 256-byte aligned. Putting it at the very start of RAM makes that
 *                           free rather than a hole.
 *
 *   .handoff is NOLOAD.     Startup must neither initialise nor zero it.
 *
 * `.RamFunc` keeps its inherited home inside `.data`, which is what copies the receive interrupt
 * and the flash routines into SRAM at startup -- the property that lets the bootloader keep
 * hearing the bus while it is erasing.
 */

ENTRY(Reset_Handler)

/* Highest address of the user mode stack: the top of RAM, below the handoff block. */
_estack = ORIGIN(RAM) + LENGTH(RAM);

/* No heap. Nothing in this image allocates, and `_sbrk` failing loudly is better than a malloc
 * that succeeds once and then does not. */
_Min_Heap_Size = 0x0;
_Min_Stack_Size = 0x800;

MEMORY
{
  RAM     (xrw) : ORIGIN = 0x20000000, LENGTH = 36K - 32
  HANDOFF (rw)  : ORIGIN = 0x20008FE0, LENGTH = 32
  FLASH   (rx)  : ORIGIN = 0x08000000, LENGTH = 16K
}

SECTIONS
{
  .isr_vector :
  {
    . = ALIGN(4);
    KEEP(*(.isr_vector))
    . = ALIGN(4);
  } >FLASH

  .text :
  {
    . = ALIGN(4);
    *(.text)
    *(.text*)
    *(.glue_7)
    *(.glue_7t)
    *(.eh_frame)

    KEEP (*(.init))
    KEEP (*(.fini))

    . = ALIGN(4);
    _etext = .;
  } >FLASH

  .rodata :
  {
    . = ALIGN(4);
    *(.rodata)
    *(.rodata*)
    . = ALIGN(4);
  } >FLASH

  .ARM.extab   :
  {
    . = ALIGN(4);
    *(.ARM.extab* .gnu.linkonce.armextab.*)
    . = ALIGN(4);
  } >FLASH

  .ARM :
  {
    . = ALIGN(4);
    __exidx_start = .;
    *(.ARM.exidx*)
    __exidx_end = .;
    . = ALIGN(4);
  } >FLASH

  .preinit_array :
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

  _sidata = LOADADDR(.data);

  /* The relocated vector table. First in RAM so its 256-byte alignment costs nothing, and NOLOAD
   * because startup fills it by copying rather than from an image. */
  .ram_vectors (NOLOAD) :
  {
    . = ALIGN(256);
    _sramvec = .;
    KEEP(*(.ram_vectors))
    . = . + 192;
    . = ALIGN(4);
  } >RAM

  .data :
  {
    . = ALIGN(4);
    _sdata = .;
    *(.data)
    *(.data*)
    /* The receive interrupt and the flash erase/program routines. Copied to SRAM at startup by
     * the same loop that copies .data, which is what lets them run while flash is stalled. */
    *(.RamFunc)
    *(.RamFunc*)
    . = ALIGN(4);
    _edata = .;
  } >RAM AT> FLASH

  . = ALIGN(4);
  .bss :
  {
    _sbss = .;
    __bss_start__ = _sbss;
    *(.bss)
    *(.bss*)
    *(COMMON)
    . = ALIGN(4);
    _ebss = .;
    __bss_end__ = _ebss;
  } >RAM

  ._user_heap_stack :
  {
    . = ALIGN(8);
    PROVIDE ( end = . );
    PROVIDE ( _end = . );
    . = . + _Min_Heap_Size;
    . = . + _Min_Stack_Size;
    . = ALIGN(8);
  } >RAM

  /* Never initialised, never zeroed: it has to survive the reset that carries it here. */
  .handoff (NOLOAD) :
  {
    . = ALIGN(4);
    KEEP(*(.handoff))
  } >HANDOFF

  /DISCARD/ :
  {
    libc.a ( * )
    libm.a ( * )
    libgcc.a ( * )
  }

  .ARM.attributes 0 : { *(.ARM.attributes) }
}

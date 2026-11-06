/* nRF52840-like map: app flash at 0x27000 (after a SoftDevice; also keeps
   code clear of address 0, where DWARF for gc-ed functions lands), RAM at 0x20000000. The Unicorn harness loads
   PT_LOAD segments straight to their addresses, so .data needs no copy. */
MEMORY
{
  FLASH : ORIGIN = 0x00027000, LENGTH = 868K
  RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
ENTRY(mo_run)
SECTIONS
{
  .text : { KEEP(*(.text.mo_*)) *(.text .text.*) } > FLASH
  .rodata : { *(.rodata .rodata.*) } > FLASH
  .data : { *(.data .data.*) } > RAM
  .bss (NOLOAD) : { *(.bss .bss.*) *(COMMON) } > RAM
  /DISCARD/ : { *(.ARM.exidx .ARM.exidx.*) *(.ARM.extab .ARM.extab.*) }
}

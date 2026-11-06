/* SPDX-License-Identifier: MIT */
/* An app's slot: m-bootloader's docs/SPEC.md §11.8. */
ENTRY(app_entry)
MEMORY { slot (rwx) : ORIGIN = 0x1c30000, LENGTH = 0x4fd80 }
SECTIONS {
    .text ORIGIN(slot) : {
        KEEP(*(.entry))
        *(.text .text.*) *(.rodata .rodata.*)
        *(.data .data.*)
        . = ALIGN(4);
    } > slot
    __payload_end = .;
    .bss (NOLOAD) : {
        __bss_start = .;
        *(.bss .bss.*) *(COMMON)
        . = ALIGN(8); __bss_end = .;
    } > slot
    .stacks (NOLOAD) : {
        __exception_stack_bottom = .; . += 1024; __exception_stack_top = .;
        __main_stack_bottom = .; . += APP_STACK; __main_stack_top = .;
    } > slot
    __ram_end = .;
    /DISCARD/ : { *(.comment) *(.note*) *(.eh_frame*) *(.llvm_addrsig) }
    ASSERT(__payload_end - ORIGIN(slot) + 8 <= LENGTH(slot), "the file, with its trailer, doesn't fit the slot")
}

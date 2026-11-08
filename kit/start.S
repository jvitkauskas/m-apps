/* SPDX-License-Identifier: MIT */
/* An app's entry (m-bootloader's docs/SPEC.md §11.8). It moves to the app's own stacks, and moves back
 * before returning, keeping every register a caller may rely on. */
.section .entry,"ax",@progbits
.global app_entry
.type app_entry,@function
app_entry:
    goto app_enter
/* The header (kit/app_launch.h), at offset 4, which the entry jumps over. */
.balign 4
    .ascii "MAPP"
    .byte 1, 0, 0, 0
    .long app_entry_launched
.size app_entry,.-app_entry
/* The launcher's way in, which the header names: r1 is its struct app_launch, kept for app_start (kit/app.c). */
.section .text.app_entry_launched,"ax",@progbits
.global app_entry_launched
.type app_entry_launched,@function
app_entry_launched:
    r2 = app_launch_given
    [r2+0] = r1
    goto app_enter
.size app_entry_launched,.-app_entry_launched
.section .text.app_enter,"ax",@progbits
.type app_enter,@function
app_enter:
    cli
    [--sp] = {rets, r15-r4}
    r2 = app_caller_stacks
    r3 = sp
    [r2+0] = r3
    r3 = ssp
    [r2+4] = r3
    r2 = __exception_stack_top
    ssp = r2
    r2 = __main_stack_top
    sp = r2
    call app_start
    cli
    r2 = app_caller_stacks
    r3 = [r2+0]
    sp = r3
    r3 = [r2+4]
    ssp = r3
    {pc, r15-r4} = [sp++]
.size app_enter,.-app_enter
.section .data.app_caller_stacks,"aw",@progbits
.balign 4
app_caller_stacks:
    .long 0, 0
/* In .data, not .bss, which app_start clears: zero unless the launcher's way in set it. */
.section .data.app_launch_given,"aw",@progbits
.balign 4
.global app_launch_given
app_launch_given:
    .long 0
/* Vector 1, the CPU exception, while the guard is on (kit/app.c). */
.section .text.app_crash_entry,"ax",@progbits
.global app_crash_entry
.type app_crash_entry,@function
app_crash_entry:
    goto app_crash
.size app_crash_entry,.-app_crash_entry

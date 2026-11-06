/* SPDX-License-Identifier: MIT */
/* An app's entry (m-bootloader's docs/SPEC.md §11.8). It moves to the app's own stacks, and moves back
 * before returning, keeping every register a caller may rely on. */
.section .entry,"ax",@progbits
.global app_entry
.type app_entry,@function
app_entry:
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
.size app_entry,.-app_entry
.section .data.app_caller_stacks,"aw",@progbits
.balign 4
app_caller_stacks:
    .long 0, 0

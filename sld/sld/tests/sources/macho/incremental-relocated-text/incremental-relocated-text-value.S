.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_relocated_text_value
_incremental_relocated_text_value:
    .long 0x52800540
#ifdef MOVE_RELOCATIONS
    nop
#endif
    adrp x8, _incremental_relocated_text_target@PAGE
    ldr w0, [x8, _incremental_relocated_text_target@PAGEOFF]
#ifndef MOVE_RELOCATIONS
    nop
#endif
    ret
    bl _incremental_relocated_text_helper

#ifdef MOVE_CROSS_INPUT_TARGETS
    nop
#endif
.globl _incremental_cross_input_function
_incremental_cross_input_function:
    mov w0, #42
    ret
#ifndef MOVE_CROSS_INPUT_TARGETS
    nop
#endif

.section __TEXT,__const
.p2align 2
#ifdef RETARGET
.globl _incremental_relocated_text_target
_incremental_relocated_text_target:
    .long 42
    .long 0
#else
    .long 0
    .long 0
#endif

.section __DATA,__data
.p2align 2
#ifdef RETARGET
    .long 0
    .long 0
#else
#ifdef MOVE_TARGET
    .long 0
#endif
.globl _incremental_relocated_text_target
_incremental_relocated_text_target:
    .long 42
#ifndef MOVE_TARGET
    .long 0
#endif
#endif

#ifdef MOVE_CROSS_INPUT_TARGETS
    .long 0
#endif
.globl _incremental_cross_input_data_target
_incremental_cross_input_data_target:
    .long 42
#ifndef MOVE_CROSS_INPUT_TARGETS
    .long 0
#endif

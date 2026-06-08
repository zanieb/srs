.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_relocated_text_value
_incremental_relocated_text_value:
    .long 0x52800540
    adrp x8, _incremental_relocated_text_target@PAGE
    ldr wzr, [x8, _incremental_relocated_text_target@PAGEOFF]
    ret
    bl _incremental_relocated_text_helper

.section __DATA,__data
.p2align 2
.globl _incremental_relocated_text_target
_incremental_relocated_text_target:
    .long 42

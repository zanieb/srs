.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_relocated_text_value
_incremental_relocated_text_value:
    adrp x8, _incremental_relocated_text_target@PAGE
    ldr w0, [x8, _incremental_relocated_text_target@PAGEOFF]
    ret

.section __DATA,__data
.p2align 2
_incremental_relocated_text_target:
    .long 42

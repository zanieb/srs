.section __DATA,__data
.p2align 2
.globl _incremental_added_text_anchor
_incremental_added_text_anchor:
    .long 42

#ifdef ADD_TEXT
.section __TEXT,__text,regular,pure_instructions
.p2align 2
    nop
#endif

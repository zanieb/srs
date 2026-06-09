.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_added_archive_value
_incremental_added_archive_value:
#ifdef ACTIVATE_ADDED_MEMBER
    b _incremental_added_archive_extra
#else
    mov w0, #42
    ret
#endif

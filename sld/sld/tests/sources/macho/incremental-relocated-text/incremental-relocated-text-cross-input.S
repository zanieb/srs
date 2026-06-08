.section __TEXT,__text,regular,pure_instructions
.p2align 2
.globl _incremental_cross_input_branch_value
_incremental_cross_input_branch_value:
    b _incremental_cross_input_function

.globl _incremental_cross_input_page_value
_incremental_cross_input_page_value:
    adrp x8, _incremental_cross_input_data_target@PAGE
    ldr w0, [x8, _incremental_cross_input_data_target@PAGEOFF]
    ret

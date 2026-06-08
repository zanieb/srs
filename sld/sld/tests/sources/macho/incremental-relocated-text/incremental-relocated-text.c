//#Config:incremental-relocated-text
//#Object:incremental-relocated-text-value.s
//#RunEnabled:false
//#DiffEnabled:false
//#TestIncremental:true
//#TestIncrementalUnsignedMachOOutput:true
//#TestIncrementalChanged:true
//#TestIncrementalChangedInput:incremental-relocated-text-value.s.o
//#TestIncrementalChangedSection:__text
//#TestIncrementalChangedExpectPatch:false
//#TestIncrementalChangedFallbackReason:changed bytes outside patchable sections

extern int incremental_relocated_text_value(void);

int main(void) { return incremental_relocated_text_value(); }

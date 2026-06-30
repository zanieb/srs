// Checks that globally enabled target features do not prevent inlining.
//@ only-aarch64
//@ compile-flags: -Cpanic=abort
//@ ignore-backends: gcc

#![crate_type = "lib"]

#[inline]
#[target_feature(enable = "neon")]
unsafe fn neon() {}

// CHECK-LABEL: fn global_feature()
// CHECK:       bb0: {
// CHECK-NEXT:  return;
pub unsafe fn global_feature() {
    // NEON is enabled globally on AArch64, so the explicit attribute is redundant.
    neon();
}

#[inline]
#[target_feature(enable = "sve")]
unsafe fn sve() {}

// CHECK-LABEL: fn local_feature()
// CHECK:       bb0: {
// CHECK-NEXT:  sve()
pub unsafe fn local_feature() {
    sve();
}

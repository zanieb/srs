//@ compile-flags: -O -Zmir-opt-level=4 -Zmir-enable-passes=+LateScalarReplacementOfAggregates
//@ skip-filecheck

#![crate_type = "lib"]

// Destination propagation makes the iterator local stop escaping after the
// first SROA pass. The backend-requested late pass can then split it into its
// pointer and end-pointer fields.

// EMIT_MIR late.has_non_ascii.LateScalarReplacementOfAggregates.diff
pub fn has_non_ascii(bytes: &[u8]) -> bool {
    let mut has_non_ascii = false;
    for byte in bytes {
        has_non_ascii |= !byte.is_ascii();
    }
    has_non_ascii
}

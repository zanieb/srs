fn main() {
    #[cfg(target_arch = "aarch64")]
    assert!(std::arch::is_aarch64_feature_detected!("crc"));

    let input = core::array::from_fn::<u8, 256, _>(|index| index as u8);
    assert_eq!(crc32fast::hash(&input), 0x2905_8c73);
}

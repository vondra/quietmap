//! CPU-only compile backstop for the exported surface-kernel argument macro.

#[test]
fn gpu_bins_use_the_exported_macro_and_the_tuple_stays_twelve_wide() {
    for source in [
        include_str!("../src/gpu_surface.rs"),
        include_str!("../src/e2_full.rs"),
    ] {
        assert!(
            source.contains("noise_gpu::line_kernel_arguments!("),
            "each bin must call the library macro through the external crate path"
        );
        assert!(!source.contains("crate::line_kernel_arguments!("));
    }

    let arguments = noise_gpu::line_kernel_arguments!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11);
    assert_eq!(arguments, (0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11));
}

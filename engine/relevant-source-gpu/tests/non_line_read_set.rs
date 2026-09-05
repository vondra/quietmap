//! Non-line layers never read the line layers' undeclared admin dependency.

#![cfg(feature = "gpu")]

use relevant_source_gpu::cell_preparation::prepare_region;
use relevant_source_gpu::cell_stream::StreamedCell;
use relevant_source_gpu::relevant_source_runner::RelevantSourceRunConfiguration;

#[test]
fn non_line_preparation_requires_structures_but_not_admin() {
    let root = tempfile::tempdir().unwrap();
    let h3r4 = root.path().join("2026/h3r4");
    let cell = 0x84668a9ffffffff;
    std::fs::create_dir_all(h3r4.join(format!("{cell:x}"))).unwrap();
    noise_compute::admin::set_admin_h3r4_directory(&h3r4);
    let configuration = RelevantSourceRunConfiguration {
        prepared_directory: root.path().to_owned(),
        h3r4_directory: h3r4,
        output_directory: root.path().join("output"),
        zoom: 13,
    };
    for layer in [2, 3, 4] {
        let request = StreamedCell {
            region_r4: cell,
            layers: vec![layer],
            tile_window: None,
        };
        let error = prepare_region(&configuration, &request)
            .err()
            .expect("the required structure table is absent");
        assert!(format!("{error:#}").contains("structures.arrow"));
    }
}

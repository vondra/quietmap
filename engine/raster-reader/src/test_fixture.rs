//! Shared z9 file fixture for sampler tests: a data window or a 0-byte absence file.

use crate::channel::Channel;
use grid::raster::RasterWindow;
use grid::Square;
use std::path::Path;

pub fn write_square(
    root: &Path,
    channel: Channel,
    square: Square,
    value: impl Fn(i32, i32) -> i16,
) {
    let window = RasterWindow::for_square(square);
    let mut bytes = Vec::with_capacity(channel.byte_len(window));
    for row in 0..window.rows {
        for column in 0..window.columns {
            let raw = value(
                window.north_node - row as i32,
                window.west_node + column as i32,
            )
            .to_be_bytes();
            bytes.extend_from_slice(&raw[2 - channel.bytes_per_node()..]);
        }
    }
    write_file(root, channel, square, &bytes);
}

pub fn write_absent_square(root: &Path, channel: Channel, square: Square) {
    write_file(root, channel, square, &[]);
}

fn write_file(root: &Path, channel: Channel, square: Square, bytes: &[u8]) {
    let path = channel.path(root, square);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

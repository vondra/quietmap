//! Shared actual z9 publication fixture for sampler tests, without a legacy reader path.

use crate::catalog::{begin_channel, content_digest, record_square};
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
    let path = channel.path(root, square);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &bytes).unwrap();
    let database = begin_channel(root, channel, &"a".repeat(64)).unwrap();
    record_square(&database, channel, square, Some(content_digest(&bytes))).unwrap();
}

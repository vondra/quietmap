//! The smallest PNG writer: 8-bit RGBA, unfiltered rows in one zlib stream.

use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::io::Write;

const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1A, b'\n'];

fn chunk(png: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    png.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc = flate2::Crc::new();
    crc.update(kind);
    crc.update(data);
    png.extend_from_slice(kind);
    png.extend_from_slice(data);
    png.extend_from_slice(&crc.sum().to_be_bytes());
}

/// The PNG of `width` x `height` straight-alpha pixels, row-major from the top.
pub fn encode(width: usize, height: usize, pixels: &[[u8; 4]]) -> Vec<u8> {
    assert_eq!(pixels.len(), width * height, "pixels of the image");
    let mut header = Vec::with_capacity(13);
    header.extend_from_slice(&(width as u32).to_be_bytes());
    header.extend_from_slice(&(height as u32).to_be_bytes());
    // Bit depth 8, colour type 6 (RGBA), deflate, adaptive filtering (each row says none), no
    // interlace.
    header.extend_from_slice(&[8, 6, 0, 0, 0]);
    let mut rows = ZlibEncoder::new(Vec::new(), Compression::fast());
    for row in pixels.chunks_exact(width) {
        rows.write_all(&[0]).expect("in memory");
        rows.write_all(row.as_flattened()).expect("in memory");
    }
    let data = rows.finish().expect("in memory");
    let mut png = Vec::with_capacity(SIGNATURE.len() + 3 * 12 + header.len() + data.len());
    png.extend_from_slice(&SIGNATURE);
    chunk(&mut png, b"IHDR", &header);
    chunk(&mut png, b"IDAT", &data);
    chunk(&mut png, b"IEND", &[]);
    png
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::ZlibDecoder;
    use std::io::Read;

    /// The rows come back from the data chunk, each after its filter byte; the end chunk carries
    /// the CRC every PNG ends with.
    #[test]
    fn rows_round_trip_and_chunks_are_framed() {
        let pixels: Vec<[u8; 4]> = (0..6).map(|i| [i, 2 * i, 3 * i, 255]).collect();
        let png = encode(3, 2, &pixels);
        assert_eq!(png[..8], SIGNATURE);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(png[16..24], [0, 0, 0, 3, 0, 0, 0, 2]);
        assert_eq!(
            png[png.len() - 8..],
            [b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]
        );
        let data_at = 8 + 12 + 13;
        let length = u32::from_be_bytes(png[data_at..data_at + 4].try_into().unwrap()) as usize;
        assert_eq!(&png[data_at + 4..data_at + 8], b"IDAT");
        let mut rows = Vec::new();
        ZlibDecoder::new(&png[data_at + 8..data_at + 8 + length])
            .read_to_end(&mut rows)
            .unwrap();
        let mut expected = vec![0];
        expected.extend(pixels[..3].as_flattened());
        expected.push(0);
        expected.extend(pixels[3..].as_flattened());
        assert_eq!(rows, expected);
    }
}

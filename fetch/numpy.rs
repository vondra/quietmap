//! Decode numeric C-order NPY members of an explicitly selected statistics archive.

use std::io::{Read, Seek};
use zip::ZipArchive;

pub struct Array {
    pub shape: Vec<usize>,
    bytes: Vec<u8>,
    width: usize,
    dtype: String,
}

impl Array {
    pub fn read<R: Read + Seek>(archive: &mut ZipArchive<R>, member: &str) -> Result<Self, String> {
        let mut file = archive.by_name(member).map_err(|error| error.to_string())?;
        let mut prefix = [0; 8];
        file.read_exact(&mut prefix)
            .map_err(|error| error.to_string())?;
        if &prefix[..6] != b"\x93NUMPY" || ![1, 2, 3].contains(&prefix[6]) || prefix[7] != 0 {
            return Err(format!("{member}: unsupported NPY header"));
        }
        let mut length = [0; 4];
        let length_bytes = if prefix[6] == 1 { 2 } else { 4 };
        file.read_exact(&mut length[..length_bytes])
            .map_err(|error| error.to_string())?;
        let length = u32::from_le_bytes(length) as usize;
        if length > 65536 {
            return Err(format!("{member}: oversized NPY header"));
        }
        let mut header = vec![0; length];
        file.read_exact(&mut header)
            .map_err(|error| error.to_string())?;
        let header = std::str::from_utf8(&header).map_err(|error| error.to_string())?;
        let header = header.replace('"', "'");
        let value = |key: &str| {
            header
                .split_once(&format!("'{key}':"))
                .map(|(_, value)| value.trim())
        };
        if !value("fortran_order").is_some_and(|value| value.starts_with("False")) {
            return Err(format!("{member}: expected C-order NPY"));
        }
        let dtype = value("descr")
            .and_then(|value| value.strip_prefix('\''))
            .and_then(|value| value.split('\'').next())
            .ok_or("missing NPY dtype")?;
        let width = match dtype {
            "<u4" => 4,
            "<f8" => 8,
            _ => return Err(format!("{member}: unsupported dtype {dtype}")),
        };
        let shape = value("shape")
            .and_then(|value| value.strip_prefix('('))
            .and_then(|value| value.split(')').next())
            .ok_or("missing NPY shape")?;
        let shape = shape
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|value| value.parse::<usize>().map_err(|error| error.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        let bytes_length = shape
            .iter()
            .try_fold(width, |length: usize, dimension| {
                length.checked_mul(*dimension)
            })
            .ok_or("NPY dimensions overflow")?;
        let remaining = file
            .size()
            .checked_sub((8 + length_bytes + length) as u64)
            .ok_or("truncated NPY")?;
        if bytes_length as u64 != remaining {
            return Err(format!("{member}: NPY length does not match dimensions"));
        }
        let mut bytes = vec![0; bytes_length];
        file.read_exact(&mut bytes)
            .map_err(|error| error.to_string())?;
        // Reading to EOF also verifies the ZIP member CRC.
        if file.read(&mut [0; 1]).map_err(|error| error.to_string())? != 0 {
            return Err(format!("{member}: trailing NPY data"));
        }
        Ok(Self {
            shape,
            bytes,
            width,
            dtype: dtype.into(),
        })
    }

    pub fn require(&self, dtype: &str, shape: &[usize]) -> Result<(), String> {
        if self.dtype != dtype || self.shape != shape {
            return Err(format!(
                "NPY expected {dtype} {shape:?}, got {} {:?}",
                self.dtype, self.shape
            ));
        }
        Ok(())
    }

    pub fn value(&self, index: usize) -> f64 {
        let bytes = &self.bytes[index * self.width..(index + 1) * self.width];
        match self.dtype.as_str() {
            "<u4" => f64::from(u32::from_le_bytes(bytes.try_into().unwrap())),
            "<f8" => f64::from_le_bytes(bytes.try_into().unwrap()),
            _ => unreachable!("validated dtype"),
        }
    }
}

#[cfg(test)]
impl Array {
    pub fn floats(shape: Vec<usize>, values: &[f64]) -> Self {
        Self {
            shape,
            bytes: values
                .iter()
                .flat_map(|value| value.to_le_bytes())
                .collect(),
            width: 8,
            dtype: "<f8".into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numpy_statistics_preserve_shape_dtype_and_exact_values() {
        let mut archive = ZipArchive::new(std::io::Cursor::new(include_bytes!(
            "fixtures/weather-numpy.zip"
        )))
        .unwrap();
        let counts = Array::read(&mut archive, "counts.npy").unwrap();
        counts.require("<u4", &[1, 3]).unwrap();
        assert_eq!(
            (0..3).map(|index| counts.value(index)).collect::<Vec<_>>(),
            [12.0, 4.0, 8.0]
        );
        let means = Array::read(&mut archive, "means.npy").unwrap();
        means.require("<f8", &[1, 3, 8]).unwrap();
        assert_eq!(means.value(23), 23.0);
        assert!(counts.require("<f8", &[1, 3]).is_err());
        assert!(counts.require("<u4", &[3, 1]).is_err());
        assert!(Array::read(&mut archive, "absent.npy").is_err());
    }

    #[test]
    fn numpy_payload_must_pass_the_archive_crc_at_eof() {
        let mut bytes = include_bytes!("fixtures/weather-numpy.zip").to_vec();
        let directory = bytes.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
        // The first central-directory entry is counts.npy; its CRC starts at byte 16.
        assert_eq!(&bytes[directory + 46..directory + 56], b"counts.npy");
        bytes[directory + 16] ^= 1;
        let mut archive = ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        assert!(Array::read(&mut archive, "counts.npy").is_err());
    }
}

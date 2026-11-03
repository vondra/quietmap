//! Land hardness and forest density on a fixed degree lattice, then lossless square windows.
use super::{config::Band, source::Source, warp};
use std::collections::BTreeMap;
use tiles::terrain::{NODES_PER_DEGREE, Window};

const SIDE: usize = NODES_PER_DEGREE as usize + 1;

pub fn degree_window(latitude: i32, longitude: i32) -> Window {
    Window {
        north_node: (latitude + 1) * NODES_PER_DEGREE,
        west_node: longitude * NODES_PER_DEGREE,
        rows: SIDE as u32,
        columns: SIDE as u32,
    }
}

fn global_nearest(source: &Source, window: Window) -> Result<Vec<f64>, String> {
    let (width, height) = source.dataset.raster_size();
    let grid = source.dataset.geo_transform().map_err(|e| e.to_string())?;
    if grid[0] != -180.0 || grid[3] != 90.0 || source.spec.horizontal_crs != "EPSG:4326" {
        return Err("global nearest raster must span the geographic world".into());
    }
    let density = width / 360;
    if density * 360 != width
        || density * 180 != height
        || grid[1] != 1.0 / density as f64
        || grid[5] != -1.0 / density as f64
    {
        return Err("global nearest raster needs an integral cells-per-degree grid".into());
    }
    let rows: Vec<_> = (0..window.rows as i64)
        .map(|r| {
            (((90 * NODES_PER_DEGREE - window.north_node) as i64 + r) * density as i64
                / NODES_PER_DEGREE as i64)
                .min(height as i64 - 1) as usize
        })
        .collect();
    let columns: Vec<_> = (0..window.columns as i64)
        .map(|c| {
            (((window.west_node + 180 * NODES_PER_DEGREE) as i64 + c) * density as i64
                / NODES_PER_DEGREE as i64)
                .rem_euclid(width as i64) as usize
        })
        .collect();
    let first_row = *rows.iter().min().unwrap();
    let last_row = *rows.iter().max().unwrap();
    let mut output = vec![0.0; window.node_count()];
    let mut start = 0;
    while start < columns.len() {
        let end = (start + 1..columns.len())
            .find(|i| columns[*i] < columns[*i - 1])
            .unwrap_or(columns.len());
        let first_column = columns[start];
        let size = (
            columns[end - 1] - first_column + 1,
            last_row - first_row + 1,
        );
        let buffer = source
            .dataset
            .rasterband(1)
            .and_then(|b| {
                b.read_as::<f64>(
                    (first_column as isize, first_row as isize),
                    size,
                    size,
                    None,
                )
            })
            .map_err(|e| e.to_string())?;
        for (r, row) in rows.iter().enumerate() {
            for c in start..end {
                output[r * columns.len() + c] =
                    buffer.data()[(row - first_row) * size.0 + columns[c] - first_column];
            }
        }
        start = end;
    }
    Ok(output)
}

fn sealing(source: &Source, window: Window) -> Result<Vec<u8>, String> {
    let masked = source.mapped("0:0,100:100,101:255,255:255", 255.0)?;
    let step = 1.0 / NODES_PER_DEGREE as f64;
    let mut options: Vec<String> = [
        "-of",
        "VRT",
        "-ot",
        "Byte",
        "-s_srs",
        &source.spec.horizontal_crs,
        "-t_srs",
        "EPSG:4326",
        "-r",
        "bilinear",
        "-srcnodata",
        "255",
        "-dstnodata",
        "255",
        "-te",
    ]
    .map(str::to_string)
    .to_vec();
    options.extend(
        [
            -180.0 - step / 2.0,
            -90.0 - step / 2.0,
            180.0 + step / 2.0,
            90.0 + step / 2.0,
        ]
        .map(|v| v.to_string()),
    );
    options.extend([
        "-ts".into(),
        (360 * NODES_PER_DEGREE + 1).to_string(),
        (180 * NODES_PER_DEGREE + 1).to_string(),
    ]);
    // Global node edges lie half a cell beyond the poles. GDAL probes those
    // unprojectable edges while constructing the VRT; sampled land stays in range.
    unsafe { gdal_sys::CPLPushErrorHandler(Some(polar_padding_error)) };
    let warped = warp::warp(&masked, &options);
    unsafe { gdal_sys::CPLPopErrorHandler() };
    let warped = warped?;
    let offset = (
        (window.west_node + 180 * NODES_PER_DEGREE) as isize,
        (90 * NODES_PER_DEGREE - window.north_node) as isize,
    );
    let size = (window.columns as usize, window.rows as usize);
    let buffer = warped
        .rasterband(1)
        .and_then(|b| b.read_as::<u8>(offset, size, size, None))
        .map_err(|e| e.to_string())?;
    Ok(buffer.data().to_vec())
}

unsafe extern "C" fn polar_padding_error(
    class: gdal_sys::CPLErr::Type,
    number: i32,
    message: *const std::ffi::c_char,
) {
    let text = unsafe { std::ffi::CStr::from_ptr(message) }.to_string_lossy();
    if !text.ends_with(": Invalid latitude") {
        unsafe { gdal_sys::CPLDefaultErrorHandler(class, number, message) };
    }
}

fn degree(
    sources: &[Source],
    latitude: i32,
    longitude: i32,
    year: i32,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let window = degree_window(latitude, longitude);
    let mut hardness = vec![255u8; window.node_count()];
    for source in sources.iter().filter(|s| s.spec.band == Band::Hardness) {
        if !source.intersects(window) {
            continue;
        }
        let source = source.for_window(window);
        let classes = if source.spec.global_nearest {
            global_nearest(source, window)?
        } else {
            warp::read(
                &source.dataset,
                window,
                &source.spec.horizontal_crs,
                (source.spec.nodata, 0.0),
                "near",
                "Byte",
                false,
            )?
        };
        for (result, class) in hardness.iter_mut().zip(classes) {
            if class.is_finite()
                && let Some(value) = source.spec.classes.get(&(class as u16))
            {
                *result = *value;
            }
        }
    }
    if hardness.contains(&255) {
        return Err(format!("unclassified hardness at {latitude}/{longitude}"));
    }
    let base_hardness = hardness.clone();
    for source in sources.iter().filter(|s| s.spec.band == Band::Sealing) {
        if !source.intersects(window) {
            continue;
        }
        let values = sealing(source, window)?;
        for (i, value) in values.into_iter().enumerate() {
            if value <= 100 {
                hardness[i] = if value == 0 && base_hardness[i] == 100 {
                    100
                } else {
                    value
                };
            }
        }
    }
    let forest = super::forest::build(sources, window, year)?;
    Ok((hardness, forest))
}

pub fn build(sources: &[Source], window: Window, year: i32) -> Result<(Vec<u8>, Vec<u8>), String> {
    let mut degrees = BTreeMap::new();
    let mut hardness = vec![100; window.node_count()];
    let mut forest = vec![0; window.node_count()];
    for row in 0..window.rows as usize {
        let north = window.north_node - row as i32;
        let latitude = north.div_euclid(NODES_PER_DEGREE).min(89);
        for column in 0..window.columns as usize {
            let east = window.west_node + column as i32;
            let longitude = east.div_euclid(NODES_PER_DEGREE);
            let wrapped = (longitude + 180).rem_euclid(360) - 180;
            let key = (latitude, wrapped);
            if let std::collections::btree_map::Entry::Vacant(entry) = degrees.entry(key) {
                entry.insert(degree(sources, latitude, wrapped, year)?);
            }
            let values = &degrees[&key];
            let index = ((latitude + 1) * NODES_PER_DEGREE - north) as usize * SIDE
                + (east - longitude * NODES_PER_DEGREE) as usize;
            hardness[row * window.columns as usize + column] = values.0[index];
            forest[row * window.columns as usize + column] = values.1[index];
        }
    }
    Ok((hardness, forest))
}

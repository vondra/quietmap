//! Decode the TIFF, CSV and explicit empty forms of a named GFW vessel-presence report.

use super::ship_raster::Raster;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

pub fn cells(
    path: &Path,
    tiff_member: &str,
    csv_member: &str,
    cell_degrees: f64,
) -> Result<BTreeMap<(i32, i32), f64>, String> {
    let file = std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut cells = BTreeMap::new();
    if file.metadata().map_err(|error| error.to_string())?.len() == 0 {
        return Ok(cells);
    }
    let mut archive = zip::ZipArchive::new(file).map_err(|error| error.to_string())?;
    let add = |cells: &mut BTreeMap<(i32, i32), f64>,
               lon: f64,
               lat: f64,
               hours: f64|
     -> Result<(), String> {
        if !lon.is_finite()
            || !lat.is_finite()
            || !hours.is_finite()
            || !(-180.0..=180.0).contains(&lon)
            || !(-90.0..=90.0).contains(&lat)
            || hours < 0.0
        {
            return Err("invalid GFW location or vessel-hours".into());
        }
        let key = (
            (lon * 1000.0).round_ties_even() as i32,
            (lat * 1000.0).round_ties_even() as i32,
        );
        *cells.entry(key).or_default() += hours;
        Ok(())
    };
    if archive.index_for_name(tiff_member).is_some() {
        let mut bytes = Vec::new();
        archive
            .by_name(tiff_member)
            .map_err(|error| error.to_string())?
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        let raster = Raster::read(bytes)?;
        if raster.geometry.epsg != 4326 {
            return Err(format!(
                "GFW report has CRS {}, expected EPSG:4326",
                raster.geometry.epsg
            ));
        }
        if (raster.geometry.width - cell_degrees).abs() > 1e-12
            || (raster.geometry.height - cell_degrees).abs() > 1e-12
        {
            return Err("GFW raster cell size differs from the manifest".into());
        }
        for index in 0..raster.valid.len() {
            let hours = raster.value(index);
            if raster.valid[index] && hours > 0.0 {
                let (lon, lat) = raster.geometry.centre(index);
                add(&mut cells, lon, lat, hours)?;
            }
        }
    } else {
        let member = archive.by_name(csv_member).map_err(|error| {
            format!(
                "{}: report has neither named TIFF nor CSV: {error}",
                path.display()
            )
        })?;
        let mut reader = csv::Reader::from_reader(member);
        let headers = reader.headers().map_err(|error| error.to_string())?.clone();
        let column = |name| {
            headers
                .iter()
                .position(|field| field == name)
                .ok_or_else(|| format!("missing GFW CSV column {name}"))
        };
        let (longitude, latitude, presence) = (
            column("Lon")?,
            column("Lat")?,
            column("Vessel Presence Hours")?,
        );
        for row in reader.records() {
            let row = row.map_err(|error| error.to_string())?;
            let value = |index| {
                row.get(index)
                    .ok_or("missing GFW CSV value")?
                    .parse::<f64>()
                    .map_err(|_| "invalid GFW CSV number")
            };
            add(
                &mut cells,
                value(longitude)?,
                value(latitude)?,
                value(presence)?,
            )?;
        }
    }
    Ok(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiff_csv_and_empty_reports_preserve_hours_coordinates_and_missingness() {
        let root = std::env::temp_dir().join(format!("qm-ship-reports-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let tiff = root.join("tiff.zip");
        let csv = root.join("csv.zip");
        let empty = root.join("empty.zip");
        std::fs::write(&tiff, include_bytes!("fixtures/ship-presence-tiff.zip")).unwrap();
        std::fs::write(&csv, include_bytes!("fixtures/ship-presence-csv.zip")).unwrap();
        std::fs::write(&empty, []).unwrap();
        let expected = BTreeMap::from([((4000, 60000), 5.0), ((4010, 59990), 7.0)]);
        assert_eq!(
            cells(&tiff, "presence.tif", "presence.csv", 0.01).unwrap(),
            expected
        );
        assert_eq!(
            cells(&csv, "presence.tif", "presence.csv", 0.01).unwrap(),
            expected
        );
        assert!(
            cells(&empty, "presence.tif", "presence.csv", 0.01)
                .unwrap()
                .is_empty()
        );
        assert!(cells(&csv, "wrong.tif", "wrong.csv", 0.01).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}

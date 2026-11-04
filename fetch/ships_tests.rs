//! Publisher fixtures exercise density coverage, production EPSG lookup and strict observation admission.
use super::*;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::io::{Cursor, Write};
use std::path::PathBuf;

pub(crate) struct Fixture {
    pub root: PathBuf,
    pub manifest: serde_json::Value,
    pub square: Square,
}
impl Fixture {
    pub fn new(label: &str) -> Self {
        let root = std::env::temp_dir().join(format!("qm-ships-{label}-{}", std::process::id()));
        let source = root.join("2000");
        std::fs::create_dir_all(source.join("density")).unwrap();
        std::fs::create_dir_all(source.join("presence")).unwrap();
        let geometry = ship_raster::Geometry {
            columns: 8,
            rows: 1,
            west: 3923000.0,
            north: 3200000.0,
            width: 1000.0,
            height: 1000.0,
            epsg: 3035,
        };
        let coverage = Coverage::new(geometry.clone(), vec![false; 8]).unwrap();
        let mut parts = Vec::new();
        for (name, class, last) in [("cargo", "large", 1.0), ("tug", "work", 2.0)] {
            let raster = tiff(
                &geometry,
                1,
                &[
                    last, -9999.0, 0.0, -9999.0, 0.125, -9999.0, -9999.0, -9999.0,
                ],
            );
            let filename = format!("{name}.zip");
            archive(
                &source.join("density").join(&filename),
                "density.tif",
                &raster,
            );
            parts.push(json!({"path":filename,"member":"density.tif","class":class}));
        }
        let mut csv = String::from("Lon,Lat,Vessel Presence Hours\n");
        for index in [6, 2, 4] {
            let (lon, lat) = coverage.centre(index).unwrap();
            csv.push_str(&format!("{lon},{lat},366\n"));
        }
        archive(
            &source.join("presence/large.zip"),
            "presence.csv",
            csv.as_bytes(),
        );
        std::fs::write(source.join("presence/work.zip"), []).unwrap();
        let window = json!({"first_day":"2000-01-01","last_day":"2000-12-31","end_exclusive":"2001-01-01","days":366,"dataset":"test:v1","classes":{"large":["cargo"],"work":["tug"]},"tile_deg":8});
        std::fs::write(
            source.join("presence/window.json"),
            serde_json::to_vec(&window).unwrap(),
        )
        .unwrap();
        let mut manifest = json!({"year":"2000","as_of":"2001-01-01","built":false,"flows":{"start":"2000-01-01","end":"2001-01-01"},"unavailable":[],"inputs":[]});
        let input = |id: &str, kind: &str, conversion: serde_json::Value| json!({"id":id,"kind":kind,"region":"world","publisher":"Test","url":"https://example.org/ships","observed":{"start":"2000-01-01","end":"2000-12-31"},"published":null,"licence":"CC0","public_ok":true,"depends_on":[],"description":"Vessel observations.","conversion":conversion});
        manifest["inputs"] = json!([
            input(
                "density",
                "prepare-ships",
                json!({"format":"density-raster","priority":20,"source_id":42,"epsg":3035,"parts":parts})
            ),
            input(
                "presence",
                "prepare-ships",
                json!({"format":"presence-reports","priority":10,"source_id":43,"window":"window.json","dataset":"test:v1","inventory":{"path":"inventory.json","sha256":""},"tiff_member":"presence.tif","csv_member":"presence.csv","cell_degrees":0.01,"acquisition":{"tile_degrees":8,"bounds":[0,48,8,56],"classes":window["classes"]}})
            ),
            input("ships", "ships", serde_json::Value::Null)
        ]);
        manifest["inputs"][2]["depends_on"] = json!(["density", "presence"]);
        let (lon, lat) = coverage.centre(0).unwrap();
        let centroid = degrees_to_z30(lat, lon);
        let mut fixture = Self {
            root,
            manifest,
            square: Square::of_z30(centroid.0, centroid.1),
        };
        fixture.pin();
        fixture
    }
    pub fn pin(&mut self) {
        let root = self.root.join("2000/presence");
        let object = |name: &str| {
            let bytes = std::fs::read(root.join(name)).unwrap();
            json!({"path":name,"bytes":bytes.len(),"sha256":hash(&bytes)})
        };
        let inventory = json!({"window_sha256":hash(&std::fs::read(root.join("window.json")).unwrap()),"reports":[{"bounds":[0,48,8,56],"large":object("large.zip"),"work":object("work.zip")}]});
        let bytes = serde_json::to_vec(&inventory).unwrap();
        std::fs::write(root.join("inventory.json"), &bytes).unwrap();
        self.manifest["inputs"][1]["conversion"]["inventory"]["sha256"] = json!(hash(&bytes));
    }
    pub fn inputs(&self) -> YearInputs {
        let path = self.root.join("manifest.json");
        std::fs::write(&path, serde_json::to_vec(&self.manifest).unwrap()).unwrap();
        YearInputs::read(&path, &self.root).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.root).unwrap();
    }
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn archive(path: &Path, member: &str, bytes: &[u8]) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file(
        member,
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(bytes).unwrap();
    zip.finish().unwrap();
}
fn tiff(geometry: &ship_raster::Geometry, raster_type: u16, values: &[f32]) -> Vec<u8> {
    use tiff::{
        encoder::{TiffEncoder, colortype},
        tags::Tag,
    };
    let mut output = Cursor::new(Vec::new());
    {
        let mut encoder = TiffEncoder::new(&mut output).unwrap();
        let mut image = encoder
            .new_image::<colortype::Gray32Float>(geometry.columns as u32, geometry.rows as u32)
            .unwrap();
        image
            .encoder()
            .write_tag(
                Tag::ModelPixelScaleTag,
                &[geometry.width, geometry.height, 0.0][..],
            )
            .unwrap();
        image
            .encoder()
            .write_tag(
                Tag::ModelTiepointTag,
                &[0.0, 0.0, 0.0, geometry.west, geometry.north, 0.0][..],
            )
            .unwrap();
        image
            .encoder()
            .write_tag(
                Tag::GeoKeyDirectoryTag,
                &[
                    1u16,
                    1,
                    0,
                    2,
                    1025,
                    0,
                    1,
                    raster_type,
                    if geometry.epsg == 4326 { 2048 } else { 3072 },
                    0,
                    1,
                    geometry.epsg,
                ][..],
            )
            .unwrap();
        image
            .encoder()
            .write_tag(Tag::Unknown(42113), "-9999")
            .unwrap();
        image.write_data(values).unwrap();
    }
    output.into_inner()
}
#[test]
fn density_reader_retains_zero_and_subthreshold_coverage_and_uses_production_epsg() {
    let fixture = Fixture::new("density");
    let inputs = fixture.inputs();
    let input = &inputs.manifest.inputs[0];
    let data = read(
        &specification(input).unwrap(),
        &fixture.root.join("2000/density"),
        &inputs,
        input,
        &fixture.square.with_neighbours().into_iter().collect(),
    )
    .unwrap();
    let coverage = data.coverage.unwrap();
    assert_eq!(
        coverage.mask,
        [true, false, true, false, true, false, false, false]
    );
    assert_eq!(data.observations.len(), 1);
    assert_eq!(data.observations[0].1.cell.hours, [1.0, 2.0, 0.0]);
    let (lon, lat) = coverage.centre(0).unwrap();
    assert_eq!(degrees_to_z30(lat, lon), (549503360, 717930135));
    assert!(coverage.covers(lon, lat));
    let (lon, lat) = coverage.centre(1).unwrap();
    assert!(!coverage.covers(lon, lat));
    for index in [2, 4] {
        let (lon, lat) = coverage.centre(index).unwrap();
        assert!(coverage.covers(lon, lat));
    }
}
#[test]
fn raster_area_point_and_presence_cell_size_are_explicit() {
    let geometry = ship_raster::Geometry {
        columns: 1,
        rows: 1,
        west: 4.0,
        north: 60.0,
        width: 0.01,
        height: 0.01,
        epsg: 4326,
    };
    let area = ship_raster::Raster::read(tiff(&geometry, 1, &[1.0])).unwrap();
    assert_eq!(area.geometry.centre(0), (4.005, 59.995));
    let point = ship_raster::Raster::read(tiff(&geometry, 2, &[1.0])).unwrap();
    assert_eq!(point.geometry.centre(0), (4.0, 60.0));
    assert!(ship_raster::Raster::read(tiff(&geometry, 3, &[1.0])).is_err());
    let fixture = Fixture::new("cell-size");
    let path = fixture.root.join("wrong.zip");
    archive(&path, "presence.tif", &tiff(&geometry, 1, &[1.0]));
    assert!(
        ship_presence::cells(&path, "presence.tif", "presence.csv", 0.02)
            .unwrap_err()
            .contains("cell size")
    );
}
#[test]
fn presence_admission_requires_the_whole_inventory_and_the_observed_leap_window() {
    let mut fixture = Fixture::new("presence");
    let read_fixture = |fixture: &Fixture| {
        let inputs = fixture.inputs();
        let input = &inputs.manifest.inputs[1];
        read(
            &specification(input).unwrap(),
            &fixture.root.join("2000/presence"),
            &inputs,
            input,
            &fixture.square.with_neighbours().into_iter().collect(),
        )
    };
    let result = read_fixture(&fixture).unwrap();
    assert_eq!(result.observations.len(), 3);
    assert_eq!(result.observations[0].1.cell.hours, [30.4375, 0.0, 0.0]);
    fixture.manifest["inputs"][1]["observed"]["start"] = json!("2000-01-02");
    assert!(read_fixture(&fixture).err().unwrap().contains("window"));
    fixture.manifest["inputs"][1]["observed"]["start"] = json!("2000-01-01");
    let inputs = fixture.inputs();
    let input = &inputs.manifest.inputs[1];
    std::fs::remove_file(fixture.root.join("2000/presence/work.zip")).unwrap();
    assert!(
        read(
            &specification(input).unwrap(),
            &fixture.root.join("2000/presence"),
            &inputs,
            input,
            &HashSet::new()
        )
        .err()
        .unwrap()
        .contains("work.zip")
    );
}

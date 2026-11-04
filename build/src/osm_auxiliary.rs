//! Export mapped routes and activities with the same retained osmium/Python converters.
use crate::year::{Kind, YearInputs};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

pub fn build(kind: &str, inputs: &YearInputs, out: &Path, scratch: &Path) -> Result<(), String> {
    let (filters, script, relations_only): (&[&str], &str, bool) = match kind {
        "bus" => (
            &["r/route=bus,trolleybus,coach"],
            include_str!("../../fetch/bus-ways.py"),
            true,
        ),
        "worship" => (
            &[
                "nwr/amenity=place_of_worship",
                "nwr/tower:type=bell_tower,minaret",
                "nwr/building=church,cathedral,chapel,bell_tower,mosque,minaret",
                "nwr/man_made=minaret",
            ],
            include_str!("../../fetch/worship.py"),
            false,
        ),
        "venues" => (
            &[
                "nwr/amenity=bar,pub,nightclub,biergarten,restaurant,cafe,fast_food",
                "nwr/leisure=outdoor_seating",
            ],
            include_str!("../../fetch/venues.py"),
            false,
        ),
        other => return Err(format!("unknown mapped inventory {other}")),
    };
    fs::create_dir(scratch).map_err(|e| e.to_string())?;
    let extract = scratch.join("selected.osm.pbf");
    let mut filter = Command::new("osmium");
    filter
        .arg("tags-filter")
        .args(["-F", "pbf"])
        .arg(inputs.required(Kind::PrepareOsm)?)
        .args(filters)
        .arg("-o")
        .arg(&extract)
        .stdin(Stdio::null());
    if relations_only {
        filter.arg("-R");
    }
    if !filter.status().map_err(|e| e.to_string())?.success() {
        return Err(format!("{kind}: osmium tags-filter failed"));
    }
    let mut export = Command::new("osmium");
    if relations_only {
        export.arg("cat").arg(&extract).args(["-f", "opl"]);
    } else {
        export.arg("export").arg(&extract).args([
            "-f",
            "geojsonseq",
            "--geometry-types=point,polygon",
            "-o",
            "-",
        ]);
    }
    let mut export = export
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    let converted = Command::new("python3")
        .arg("-I")
        .arg("-c")
        .arg(script)
        .arg(out)
        .stdin(Stdio::from(export.stdout.take().unwrap()))
        .status();
    let exported = export.wait().map_err(|e| e.to_string())?;
    if !converted.map_err(|e| e.to_string())?.success() || !exported.success() {
        return Err(format!("{kind}: mapped inventory conversion failed"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_inventories_read_manifest_aliases_without_filename_extensions() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path();
        fs::create_dir(root.join("2000")).unwrap();
        let xml = root.join("mapped.osm");
        fs::write(&xml, r#"<osm version="0.6">
<node id="1" lat="50" lon="14"><tag k="amenity" v="cafe"/></node>
<node id="2" lat="50.01" lon="14.01"><tag k="amenity" v="place_of_worship"/><tag k="religion" v="christian"/></node>
<way id="10"><nd ref="1"/><nd ref="2"/></way>
<relation id="20"><member type="way" ref="10" role=""/><tag k="type" v="route"/><tag k="route" v="bus"/><tag k="interval" v="10"/></relation>
</osm>"#).unwrap();
        assert!(
            Command::new("osmium")
                .arg("cat")
                .arg(&xml)
                .args(["-f", "pbf", "-o"])
                .arg(root.join("2000/osm"))
                .status()
                .unwrap()
                .success()
        );
        let manifest = serde_json::json!({"year":"2000", "built":false, "as_of":"2001-01-01",
            "flows":{"start":"2000-01-01","end":"2001-01-01"}, "unavailable":[], "inputs":[{
                "id":"osm","kind":"prepare-osm","region":"world","publisher":"Test",
                "url":"https://example.org/osm","observed":{"start":"2000","end":"2000"},
                "published":null,"public_ok":true,"licence":"CC0","description":"Mapped activities.","depends_on":[]}]});
        let path = root.join("year.json");
        fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let inputs = YearInputs::read(&path, root).unwrap();
        for kind in ["bus", "worship", "venues"] {
            let out = root.join(kind);
            build(kind, &inputs, &out, &root.join(format!("{kind}-scratch"))).unwrap();
            let rows = fs::read_to_string(out).unwrap();
            assert_eq!(rows.lines().count(), 1, "{kind}: {rows}");
        }
    }
}

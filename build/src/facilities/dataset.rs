//! Completed facility inputs are tied to their manifest and explicitly list every prepared square.

use super::{
    fingerprint::classified_inputs,
    prepare::Receipt,
    records::{self, Facility},
};
use crate::{
    dev4::Square,
    year::{Kind, YearInputs},
};
use std::{collections::BTreeSet, path::PathBuf};

pub struct Dataset {
    root: PathBuf,
    year: String,
    input_hash: String,
    squares: BTreeSet<[u32; 2]>,
}

impl Dataset {
    pub fn open(inputs: &YearInputs) -> Result<Self, String> {
        let root = inputs.required(Kind::Facilities)?;
        let receipt: Receipt = serde_json::from_slice(
            &std::fs::read(root.join("complete.json")).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;
        if receipt.year != inputs.manifest.year
            || receipt.input_sha256 != classified_inputs(inputs).map_err(|e| e.to_string())?
        {
            return Err("facility inputs belong to another year manifest".into());
        }
        if receipt.squares.is_empty() || receipt.squares.iter().any(|s| s[0] >= 512 || s[1] >= 512)
        {
            return Err("invalid facility square inventory".into());
        }
        Ok(Self {
            root: root.into(),
            year: receipt.year,
            input_hash: receipt.input_sha256,
            squares: receipt.squares.into_iter().collect(),
        })
    }
    pub fn rows(&self, square: Square, role: u8) -> Result<Vec<Facility>, String> {
        if !self.squares.contains(&[square.x, square.y]) {
            return Err(format!("facility square not prepared: {square:?}"));
        }
        records::read(
            &self
                .root
                .join("z9")
                .join(square.x.to_string())
                .join(square.y.to_string())
                .join("facilities.arrow"),
            &self.year,
            &self.input_hash,
            Some(role),
        )
        .map_err(|e| e.to_string())
    }

    pub fn require_source_context(&self, squares: &[Square]) -> Result<(), String> {
        for target in squares {
            for square in target
                .with_neighbours()
                .into_iter()
                .flat_map(Square::with_neighbours)
            {
                if !self.squares.contains(&[square.x, square.y]) {
                    return Err(format!(
                        "facilities for sources need a two-square halo: {square:?} is not prepared"
                    ));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facilities::prepare::Receipt;
    use serde_json::json;

    #[test]
    fn source_outlines_require_two_rings_including_wrapped_longitude() {
        let target = Square { x: 0, y: 0 };
        let mut dataset = Dataset {
            root: PathBuf::new(),
            year: String::new(),
            input_hash: String::new(),
            squares: target
                .with_neighbours()
                .iter()
                .map(|s| [s.x, s.y])
                .collect(),
        };
        assert!(dataset.require_source_context(&[target]).is_err());
        dataset.squares = target
            .with_neighbours()
            .into_iter()
            .flat_map(Square::with_neighbours)
            .map(|s| [s.x, s.y])
            .collect();
        assert!(dataset.require_source_context(&[target]).is_ok());
        dataset.squares.remove(&[510, 0]);
        assert!(dataset.require_source_context(&[target]).is_err());
    }

    #[test]
    fn missing_unprepared_and_foreign_manifest_inputs_are_errors_not_empty_squares() {
        let temporary = tempfile::tempdir().unwrap();
        let manifest_path = temporary.path().join("manifest.json");
        let mut manifest = json!({"year":"2000","built":false,"as_of":"2001-01-01",
            "flows":{"start":"2000-01-01","end":"2001-01-01"},"unavailable":[],
            "inputs":[{"id":"facilities","kind":"facilities","region":"world","publisher":"test",
                "url":"https://example.org","observed":{"start":"2000","end":"2000"},
                "published":null,"licence":"CC0","public_ok":true,"depends_on":[],"description":"test"}]});
        for (id, kind) in [
            ("model", "facility-model"),
            ("observations", "facility-observations"),
        ] {
            let mut input = manifest["inputs"][0].clone();
            input["id"] = id.into();
            input["kind"] = kind.into();
            manifest["inputs"].as_array_mut().unwrap().push(input);
        }
        std::fs::write(&manifest_path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        let root = temporary.path().join("2000/facilities");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.parent().unwrap().join("model"), "{}").unwrap();
        std::fs::write(
            root.parent().unwrap().join("observations"),
            "normalized fixture",
        )
        .unwrap();
        let inputs = YearInputs::read(&manifest_path, temporary.path()).unwrap();
        assert!(Dataset::open(&inputs).is_err());
        let hash = classified_inputs(&inputs).unwrap();
        let receipt = Receipt {
            input_sha256: hash.clone(),
            year: "2000".into(),
            squares: vec![[1, 2]],
        };
        std::fs::write(
            root.join("complete.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        let dataset = Dataset::open(&inputs).unwrap();
        assert!(
            dataset
                .rows(Square { x: 1, y: 2 }, records::INDUSTRY)
                .is_err()
        );
        assert!(
            dataset
                .rows(Square { x: 1, y: 3 }, records::INDUSTRY)
                .is_err()
        );
        records::write(&root.join("z9/1/2/facilities.arrow"), &[], "2000", &hash).unwrap();
        assert!(
            dataset
                .rows(Square { x: 1, y: 2 }, records::INDUSTRY)
                .unwrap()
                .is_empty()
        );
        let file = root.join("z9/1/2/facilities.arrow");
        assert!(records::read(&file, "2000", "different inputs", None).is_err());
        std::fs::write(&manifest_path, format!("{}\n", manifest)).unwrap();
        assert!(Dataset::open(&inputs).is_ok());
        std::fs::write(root.parent().unwrap().join("model"), "{\"changed\":true}").unwrap();
        assert!(Dataset::open(&inputs).is_err());
        std::fs::write(root.parent().unwrap().join("model"), "{}").unwrap();
        std::fs::write(
            root.parent().unwrap().join("observations"),
            "changed observations",
        )
        .unwrap();
        assert!(Dataset::open(&inputs).is_err());
    }
}

//! Convert exactly the declared observation dependencies and reject unstable export-row identities.

use super::{InputSpec, Kind, YearInputs};
use anyhow::{Result, ensure};
use std::collections::BTreeSet;

pub fn declared_sources<'a>(
    inputs: &'a YearInputs,
    recipes: &[InputSpec],
) -> Result<BTreeSet<&'a str>> {
    let declarations: Vec<_> = inputs
        .manifest
        .inputs
        .iter()
        .filter(|input| input.kind == Kind::FacilityObservations)
        .collect();
    let [declaration] = declarations.as_slice() else {
        anyhow::bail!("declare exactly one facility observations input");
    };
    ensure!(
        inputs
            .manifest
            .admitted(&declaration.id)
            .map_err(anyhow::Error::msg)?,
        "facility observation dependencies are not admitted"
    );
    let selected: BTreeSet<_> = inputs
        .manifest
        .inputs
        .iter()
        .filter(|input| {
            input.kind == Kind::PrepareIndustrialSites && declaration.depends_on.contains(&input.id)
        })
        .map(|input| input.id.as_str())
        .collect();
    ensure!(
        selected == recipes.iter().map(|recipe| recipe.input.as_str()).collect(),
        "facility recipes must match the observation input's declared register dependencies"
    );
    // ESRI export/republication regenerates these row numbers; they cannot link a facility across snapshots.
    for recipe in recipes {
        ensure!(
            !recipe.identities.iter().any(|name| matches!(
                name.to_ascii_lowercase().as_str(),
                "objectid" | "objectid_1" | "object_id" | "fid" | "fid_1" | "fid_12" | "oid"
            )),
            "{}: export row numbers are not facility identities",
            recipe.input
        );
    }
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn inputs(temporary: &tempfile::TempDir) -> YearInputs {
        let entries: Vec<_> = [
            ("register", "prepare-industrial-sites", vec![]),
            ("not-selected", "prepare-industrial-sites", vec![]),
            ("observations", "facility-observations", vec!["register"]),
        ]
        .into_iter()
        .map(|(id, kind, dependencies)| {
            json!({"id":id,"kind":kind,
                "region":"world","publisher":"test","url":"https://example.org",
                "observed":{"start":"2000","end":"2000"},"published":null,
                "licence":"CC0","public_ok":true,"depends_on":dependencies,"description":"test"})
        })
        .collect();
        let path = temporary.path().join("manifest.json");
        std::fs::write(
            &path,
            json!({"year":"2000","built":false,"as_of":"2001-01-01",
            "flows":{"start":"2000-01-01","end":"2001-01-01"},"inputs":entries,"unavailable":[]})
            .to_string(),
        )
        .unwrap();
        YearInputs::read(&path, temporary.path()).unwrap()
    }

    #[test]
    fn undeclared_registers_cannot_be_consumed_or_escape_the_fingerprint() {
        let temporary = tempfile::tempdir().unwrap();
        let mut inputs = inputs(&temporary);
        let mut recipes = vec![InputSpec {
            input: "register".into(),
            ..Default::default()
        }];
        assert_eq!(
            declared_sources(&inputs, &recipes).unwrap(),
            BTreeSet::from(["register"])
        );
        recipes.push(InputSpec {
            input: "not-selected".into(),
            ..Default::default()
        });
        assert!(declared_sources(&inputs, &recipes).is_err());
        inputs.manifest.inputs[2]
            .depends_on
            .push("not-selected".into());
        assert_eq!(declared_sources(&inputs, &recipes).unwrap().len(), 2);
        recipes.pop();
        assert!(declared_sources(&inputs, &recipes).is_err());
    }

    #[test]
    fn a_republished_row_counter_is_not_a_persistent_facility_key() {
        let temporary = tempfile::tempdir().unwrap();
        let inputs = inputs(&temporary);
        let mut recipe = InputSpec {
            input: "register".into(),
            identities: vec!["OBJECTID".into()],
            ..Default::default()
        };
        assert!(declared_sources(&inputs, &[recipe.clone()]).is_err());
        recipe.identities = vec!["publisher_key".into()];
        assert!(declared_sources(&inputs, &[recipe.clone()]).is_ok());
        recipe.identities.clear();
        assert!(declared_sources(&inputs, &[recipe]).is_ok());
    }
}

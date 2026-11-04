//! Decode, shuffle and retain aircraft tracks from the manifest window.
use crate::{aircraft, boxes, cli::*, year};
use std::path::Path;

pub(crate) fn run(
    command: &str,
    options: &Arguments,
    inputs: &year::YearInputs,
    out: &Path,
) -> Result<(), String> {
    match command {
        "aircraft-segments" => {
            if let Some(threads) = options.optional("threads") {
                let threads = threads
                    .parse()
                    .map_err(|_| format!("bad --threads {threads}"))?;
                rayon::ThreadPoolBuilder::new()
                    .num_threads(threads)
                    .build_global()
                    .map_err(|error| error.to_string())?;
            }
            let window = inputs.manifest.window();
            let days = aircraft::Days::listed(&window.baseline_days, &window.increment_days)?;
            aircraft::run(aircraft::Run {
                days,
                primary: inputs.required(crate::year::Kind::Primary)?,
                secondary: if window.increment_days.is_empty() {
                    None
                } else {
                    Some(inputs.required(crate::year::Kind::Secondary)?)
                },
                rasters: inputs.required(crate::year::Kind::Rasters)?,
                geoid: inputs.required(crate::year::Kind::Geoid)?,
                boxes: options.optional("boxes"),
                out,
                national: national_models(inputs)?,
                raw_heights: options
                    .optional("scratch")
                    .map(|path| {
                        crate::rasters::ground::GroundHeights::open(inputs, Path::new(path))
                    })
                    .transpose()?,
            })?;
            require_days(out, &window)
        }
        "aircraft-shuffle" => {
            let window = inputs.manifest.window();
            let mut squares = parse_squares(options.get("squares")?)?;
            // `--halo`: with the squares the events of these read (within their reach).
            if options.optional("halo").is_some() {
                squares = boxes::events::halo(&squares);
            }
            let scope = squares.into_iter().collect();
            let left = boxes::shuffle::shuffle(
                inputs.required(crate::year::Kind::Segments)?,
                &window,
                &scope,
                out,
            )?;
            if left == 0 {
                Ok(())
            } else {
                Err(format!(
                    "aircraft shuffle: {left} requested days not written"
                ))
            }
        }
        "aircraft-boxes" | "aircraft-events" => {
            crate::commands_airspace::run(command, options, inputs, out)
        }
        "aircraft-tracks" => aircraft::tracks::build(
            &inputs
                .required(crate::year::Kind::Segments)?
                .join("segments"),
            &inputs.manifest.window(),
            out,
        ),
        other => Err(format!("unknown aircraft command {other}")),
    }
}

fn require_days(root: &Path, window: &boxes::Window) -> Result<(), String> {
    let admission: aircraft::Admission = serde_json::from_slice(
        &std::fs::read(root.join("admission.json")).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    for (kind, requested, admitted) in [
        ("baseline", &window.baseline_days, &admission.baseline_days),
        (
            "increment",
            &window.increment_days,
            &admission.increment_days,
        ),
    ] {
        let requested = requested
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if requested != *admitted {
            return Err(format!(
                "aircraft {kind} admission differs from the manifest: rejected {:?}, unexpected {:?}; correct the manifest window before building",
                requested.difference(admitted).collect::<Vec<_>>(),
                admitted.difference(&requested).collect::<Vec<_>>()
            ));
        }
    }
    for day in &window.baseline_days {
        for kind in ["segments", "flights"] {
            let path = root.join(kind).join(format!("{day}.arrow"));
            if !path.is_file() {
                return Err(format!(
                    "requested aircraft day is incomplete: {}",
                    path.display()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_missing_provider_receipt_cannot_complete_an_aircraft_stage() {
        let root = tempfile::tempdir().unwrap();
        let window = crate::boxes::Window {
            baseline_days: vec!["2000-01-01".into()],
            increment_days: vec!["2000-01-01".into()],
        };
        assert!(super::require_days(root.path(), &window).is_err());
        for kind in ["segments", "flights"] {
            std::fs::create_dir(root.path().join(kind)).unwrap();
            std::fs::write(
                root.path().join(kind).join("2000-01-01.arrow"),
                b"written day",
            )
            .unwrap();
        }
        let mut admission = serde_json::json!({
            "baseline_days":["2000-01-01"], "increment_days":[],
            "primary":{}, "secondary":{}
        });
        let path = root.path().join("admission.json");
        std::fs::write(&path, serde_json::to_vec(&admission).unwrap()).unwrap();
        assert!(super::require_days(root.path(), &window).is_err());
        admission["increment_days"] = serde_json::json!(["2000-01-01"]);
        admission["baseline_days"] = serde_json::json!([]);
        std::fs::write(&path, serde_json::to_vec(&admission).unwrap()).unwrap();
        assert!(super::require_days(root.path(), &window).is_err());
        admission["baseline_days"] = serde_json::json!(["2000-01-01"]);
        std::fs::write(&path, serde_json::to_vec(&admission).unwrap()).unwrap();
        super::require_days(root.path(), &window).unwrap();
    }
}

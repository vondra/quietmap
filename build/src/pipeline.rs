//! Build one year's selected squares in dependency order; independent work survives a missing input.
use crate::{
    cli::{Arguments, parse_squares},
    dev4::Square,
    pipeline_files::{link, publish, write_json},
    pipeline_plan::{context, steps},
    pipeline_storage::Run,
    year::{Kind, Manifest},
};
use anyhow::{Result, ensure};
use serde_json::json;
use std::{collections::BTreeSet, fs, path::Path};

fn bind(run: &Run, manifest: &Manifest, kind: Kind, path: &Path) -> Result<()> {
    let inputs: Vec<_> = manifest
        .inputs
        .iter()
        .filter(|input| input.kind == kind)
        .collect();
    ensure!(
        inputs.len() == 1,
        "year runner needs exactly one declaration of {kind}"
    );
    let alias = run.inputs.join(&manifest.year).join(&inputs[0].id);
    link(path, &alias)?;
    if kind == Kind::OsmGeography {
        for family in ["railway-ways", "train-routes"] {
            link(
                &crate::osm::year_sibling_path(path, family).map_err(anyhow::Error::msg)?,
                &crate::osm::year_sibling_path(&alias, family).map_err(anyhow::Error::msg)?,
            )?;
        }
    }
    Ok(())
}

fn composed(run: &Run, squares: &[Square], output: &Path) -> Result<()> {
    for &square in squares {
        let relative = format!("z9/{}/{}", square.x, square.y);
        let target = output.join(&relative);
        fs::create_dir_all(&target)?;
        for (stage, families) in [
            (
                "osm",
                &["square-country-city.bin", "transport_nodes.arrow"][..],
            ),
            ("structures", &["structures.arrow"]),
            ("roads", &["roads.arrow"]),
            ("prepare-railways", &["railways.arrow"]),
        ] {
            for family in families {
                let source = run
                    .stage_root(stage)
                    .join("output")
                    .join(&relative)
                    .join(family);
                if source.exists() {
                    link(&source, &target.join(family))?;
                }
            }
        }
    }
    Ok(())
}

pub(crate) fn build(options: &Arguments) -> Result<()> {
    options
        .require_only(&[
            "year",
            "sources",
            "out",
            "scratch",
            "squares",
            "boxes",
            "threads",
            "memory-gb",
            "road-library-path",
        ])
        .map_err(anyhow::Error::msg)?;
    let manifest_path = Path::new(options.get("year").map_err(anyhow::Error::msg)?);
    let manifest = Manifest::read(manifest_path).map_err(anyhow::Error::msg)?;
    let squares = context(
        &parse_squares(options.get("squares").map_err(anyhow::Error::msg)?)
            .map_err(anyhow::Error::msg)?,
        0,
    );
    ensure!(
        !squares.is_empty() && squares.iter().all(|s| s.x < crate::dev4::SQUARES_PER_AXIS && s.y < crate::dev4::SQUARES_PER_AXIS),
        "invalid year selection"
    );
    let selection = squares
        .iter()
        .map(|s| format!("{}:{}\n", s.x, s.y))
        .collect::<String>();
    if squares.len() == crate::dev4::SQUARES_PER_AXIS.pow(2) as usize {
        let inputs = crate::year::YearInputs::read(
            manifest_path,
            Path::new(options.get("sources").map_err(anyhow::Error::msg)?),
        )
        .map_err(anyhow::Error::msg)?;
        let produced = crate::pipeline_plan::produced_kinds();
        crate::source_tables::check_tables(&inputs, &produced).map_err(anyhow::Error::msg)?;
        crate::pipeline_plan::check_publisher_inputs(&inputs)?;
        inputs
            .check_published_tables(&produced)
            .map_err(anyhow::Error::msg)?;
    }
    let run = Run::open(
        manifest_path,
        Path::new(options.get("sources").map_err(anyhow::Error::msg)?),
        Path::new(options.get("out").map_err(anyhow::Error::msg)?),
        Path::new(options.get("scratch").map_err(anyhow::Error::msg)?),
        &selection,
        &serde_json::to_value(options.identity())?,
    )?;
    let contexts: Vec<_> = (0..=3).map(|radius| context(&squares, radius)).collect();
    for (radius, squares) in contexts.iter().enumerate() {
        fs::write(
            run.root.join(format!("squares-{radius}")),
            squares
                .iter()
                .map(|s| format!("{}:{}\n", s.x, s.y))
                .collect::<String>(),
        )?;
    }
    let mut complete = BTreeSet::new();
    let mut failures = Vec::new();
    for step in steps() {
        run.verify_inputs()?;
        if !step.dependencies.iter().all(|name| complete.contains(name)) {
            failures.push(format!("{}: prerequisite did not complete", step.name));
            continue;
        }
        let output = if step.native {
            run.out.clone()
        } else {
            run.stage_root(step.name).join("output")
        };
        let result = (|| -> Result<()> {
            let mut extra = Vec::new();
            if let Some(radius) = step.radius {
                extra.push((
                    "squares",
                    format!("@{}", run.root.join(format!("squares-{radius}")).display()),
                ));
            }
            if matches!(
                step.name,
                "osm" | "rail-movements" | "aircraft-segments" | "bus" | "worship" | "venues"
            ) {
                extra.push((
                    "scratch",
                    run.stage_root(step.name)
                        .join("scratch")
                        .display()
                        .to_string(),
                ));
            }
            if step.name == "aircraft-shuffle" {
                extra.push(("halo", "yes".into()));
            }
            if step.name == "aircraft-boxes" {
                extra.push(("kind", "aircraft,aircraft-far".into()));
            }
            if matches!(step.name, "aircraft-boxes" | "aircraft-events") {
                if let Some(value) = options.optional("memory-gb") {
                    extra.push(("memory-gb", value.into()));
                }
                extra.push((
                    "progress",
                    run.stage_root(step.name)
                        .join("progress")
                        .display()
                        .to_string(),
                ));
            }
            if step.name == "aircraft-segments" {
                for key in ["threads", "boxes"] {
                    if let Some(value) = options.optional(key) {
                        extra.push((key, value.into()));
                    }
                }
            }
            if step.name == "prepared" {
                composed(&run, &contexts[1], &output)?;
            } else {
                run.execute(
                    step.name,
                    &output,
                    &extra,
                    if step.name == "road-observations" {
                        options.optional("road-library-path")
                    } else {
                        None
                    },
                )?;
            }
            if let Some(kind) = step.binding {
                bind(&run, &manifest, kind, &output)?;
                if crate::source_tables::MODEL_KINDS.contains(&kind) {
                    let table = format!("{kind}.json");
                    publish(
                        &output,
                        &run.root.join("tables").join(&manifest.year).join(&table),
                    )?;
                    publish(&output, &run.out.join("tables").join(&table))?;
                }
            }
            match step.name {
                "weather" => publish(&output, &run.out.join("weather"))?,
                "source-outlines" => publish(&output, &run.out.join("source-outlines.sqlite"))?,
                "aircraft-tracks" => publish(
                    &output.join("aircraft-tracks"),
                    &run.out.join("aircraft-tracks"),
                )?,
                "airport-traffic" if output.join("rolls").exists() => {
                    publish(&output.join("rolls"), &run.out)?
                }
                _ => (),
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                complete.insert(step.name);
            }
            Err(error) => {
                run.verify_inputs()?;
                eprintln!("year: {}: {error:#}", step.name);
                failures.push(format!("{}: {error:#}", step.name));
            }
        }
    }
    if failures.is_empty()
        && let Err(error) = finalize(&run, &squares, options.optional("boxes"))
    {
        failures.push(format!("finalize: {error:#}"));
    }
    write_json(
        &run.root.join("status.json"),
        &json!({
            "complete": complete, "failures": failures, "finalized": failures.is_empty()
        }),
    )?;
    ensure!(
        failures.is_empty(),
        "year incomplete: {}",
        failures.join("; ")
    );
    Ok(())
}

fn finalize(run: &Run, squares: &[Square], boxes: Option<&str>) -> Result<()> {
    run.verify_inputs()?;
    if squares.len() == crate::dev4::SQUARES_PER_AXIS.pow(2) as usize && boxes.is_none() {
        run.execute("complete", &run.out, &[("note", "world".into())], None)
    } else {
        let inputs = crate::year::YearInputs::read(&run.manifest, &run.inputs)
            .map_err(anyhow::Error::msg)?;
        crate::source_tables::SourceTables::read(&inputs).map_err(anyhow::Error::msg)?;
        inputs
            .check_published_tables(&Default::default())
            .map_err(anyhow::Error::msg)?;
        record_partial(run, squares, boxes)
    }
}

pub(crate) fn record_partial(run: &Run, squares: &[Square], boxes: Option<&str>) -> Result<()> {
    fs::copy(&run.manifest, run.out.join("year.json"))?;
    write_json(
        &run.out.join("partial.json"),
        &json!({
            "layout": tiles::LAYOUT,
            "manifest_sha256": crate::pipeline_files::digest(&run.manifest)?,
            "squares": squares.iter().map(|s| [s.x, s.y]).collect::<Vec<_>>(),
            "aircraft_boxes": boxes,
            "aircraft_scope": if boxes.is_some() { "diagnostic traces touching the supplied boxes; not complete aircraft coverage of the selected squares" } else { "all admitted flights" },
        }),
    )
}

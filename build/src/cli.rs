//! Parse build arguments and the shared spatial, terrain and aircraft options.
use crate::{
    boxes,
    dev4::{self, Square},
    terrain, year,
};
use std::path::{Path, PathBuf};

pub(crate) struct Arguments {
    values: Vec<(String, String)>,
}

impl Arguments {
    pub(crate) fn parse(arguments: &[String]) -> Result<Self, String> {
        let mut values = Vec::new();
        for pair in arguments.chunks(2) {
            match pair {
                [key, value] if key.starts_with("--") => {
                    let key = &key[2..];
                    if ![
                        "year",
                        "sources",
                        "out",
                        "squares",
                        "kind",
                        "threads",
                        "boxes",
                        "halo",
                        "memory-gb",
                        "progress",
                        "level-step-db",
                        "pieces",
                        "points",
                        "only",
                        "diagnose",
                        "note",
                        "scratch",
                        "road-library-path",
                    ]
                    .contains(&key)
                    {
                        return Err(format!(
                            "unknown option --{key}; input files and flow dates belong in --year"
                        ));
                    }
                    if values.iter().any(|(name, _)| name == key) {
                        return Err(format!("repeated option --{key}"));
                    }
                    values.push((key.to_string(), value.clone()))
                }
                _ => return Err(format!("expected --key value pairs, got {pair:?}")),
            }
        }
        Ok(Arguments { values })
    }

    pub(crate) fn get(&self, key: &str) -> Result<&str, String> {
        self.optional(key).ok_or_else(|| format!("missing --{key}"))
    }

    pub(crate) fn optional(&self, key: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    }

    pub(crate) fn identity(&self) -> std::collections::BTreeMap<&str, &str> {
        self.values
            .iter()
            .filter(|(key, _)| !matches!(key.as_str(), "threads" | "memory-gb"))
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    pub(crate) fn require_only(&self, accepted: &[&str]) -> Result<(), String> {
        for (key, _) in &self.values {
            if !accepted.contains(&key.as_str()) {
                return Err(format!("--{key} is not supported by the year runner"));
            }
        }
        Ok(())
    }

    /// The terrain input, the year root a builder reads the ground from: the tree it builds (`--out`,
    /// not complete yet) or a complete one of this layout; one of another layout would read as
    /// all ocean.
    pub(crate) fn terrain<'a>(
        &self,
        inputs: &'a year::YearInputs,
        out: &Path,
    ) -> Result<&'a Path, String> {
        let terrain = inputs.required(crate::year::Kind::Terrain)?;
        if terrain != out
            && terrain
                .canonicalize()
                .ok()
                .zip(out.canonicalize().ok())
                .is_none_or(|(left, right)| left != right)
        {
            if let Some(selection) = self.optional("squares") {
                crate::terrain_completion::check(
                    inputs,
                    terrain,
                    &crate::pipeline_plan::context(&parse_squares(selection)?, 1),
                )?;
            } else {
                inputs.check_complete(terrain)?;
            }
        }
        Ok(terrain)
    }
}

/// The squares of `--squares`: `X:Y,X:Y,..`, `all` (every z9 square of the world) or `@FILE`
/// (the same pairs, by commas or lines: a world's list is too long for one argument).
pub(crate) fn parse_squares(text: &str) -> Result<Vec<Square>, String> {
    if text == "all" {
        return Ok((0..dev4::SQUARES_PER_AXIS)
            .flat_map(|x| (0..dev4::SQUARES_PER_AXIS).map(move |y| Square { x, y }))
            .collect());
    }
    if let Some(path) = text.strip_prefix('@') {
        let listed = std::fs::read_to_string(path).map_err(|error| format!("{path}: {error}"))?;
        let pairs: Vec<&str> = listed
            .split([',', '\n'])
            .map(str::trim)
            .filter(|pair| !pair.is_empty())
            .collect();
        return parse_squares(&pairs.join(","));
    }
    text.split(',')
        .map(|pair| {
            let (x, y) = pair
                .split_once(':')
                .ok_or_else(|| format!("square {pair:?} is not X:Y"))?;
            Ok(Square {
                x: x.parse().map_err(|_| format!("bad x in {pair:?}"))?,
                y: y.parse().map_err(|_| format!("bad y in {pair:?}"))?,
            })
        })
        .collect()
}

/// Every national terrain model admitted by the manifest, in its declared precedence.
pub(crate) fn national_models(
    inputs: &year::YearInputs,
) -> Result<Vec<terrain::national::NationalHeights>, String> {
    inputs
        .national_dem_paths()?
        .iter()
        .map(|path| terrain::national::NationalHeights::read(path))
        .collect()
}

/// The box rules of `--kind` (a list, `aircraft` by default), with `--level-step-db` and
/// `--pieces` laid over each.
pub(crate) fn box_rules(options: &Arguments) -> Result<Vec<boxes::BoxRule>, String> {
    options
        .optional("kind")
        .unwrap_or("aircraft")
        .split(',')
        .map(|kind| {
            let mut rule = match kind {
                "aircraft" => boxes::BoxRule::default(),
                "aircraft-far" => boxes::BoxRule::far(),
                other => return Err(format!("bad --kind {other}")),
            };
            if let Some(step) = options.optional("level-step-db") {
                rule.level_step_db = step
                    .parse()
                    .map_err(|_| format!("bad --level-step-db {step}"))?;
            }
            if let Some(pieces) = options.optional("pieces") {
                rule.pieces = pieces
                    .parse()
                    .map_err(|_| format!("bad --pieces {pieces}"))?;
            }
            Ok(rule)
        })
        .collect()
}

/// `--memory-gb`: the memory the squares built at once may take by their estimates (GB, 32 by
/// default; the estimates are bounds: the heaviest squares took about half of theirs, the rest a
/// third or less, 2026-10-16).
pub(crate) fn memory_budget(options: &Arguments) -> Result<u64, String> {
    let gigabytes: u64 = options
        .optional("memory-gb")
        .unwrap_or("32")
        .parse()
        .map_err(|_| "bad --memory-gb".to_string())?;
    Ok(gigabytes << 30)
}

/// `--progress DIR`: a marker per square and kind built, so that a rerun resumes.
pub(crate) fn progress_markers(options: &Arguments) -> Result<Option<PathBuf>, String> {
    let progress = options.optional("progress").map(PathBuf::from);
    if let Some(progress) = &progress {
        std::fs::create_dir_all(progress).map_err(|error| error.to_string())?;
    }
    Ok(progress)
}

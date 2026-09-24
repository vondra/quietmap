//! The Python publisher authority binds primary-provider inputs and completed work receipts.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Publisher receipts of a primary archive root with a `catalog.sqlite`.
pub struct SourceCache {
    root: PathBuf,
    work: PathBuf,
}

impl SourceCache {
    pub fn new(root: &Path, work: &Path) -> Self {
        Self {
            root: root.into(),
            work: work.into(),
        }
    }

    /// The publisher authority of `root` when it holds a catalog; a plain
    /// archive directory is read as it is.
    pub fn open(root: &Path, work: &Path) -> Result<Option<Self>> {
        Ok(root
            .join("catalog.sqlite")
            .try_exists()?
            .then(|| Self::new(root, work)))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn validate(
        &self,
        days: Option<&[String]>,
        stage: Option<&str>,
    ) -> Result<BTreeMap<String, Vec<PathBuf>>> {
        self.invoke(days, stage.map(|s| (s, "check")))
    }

    pub fn begin(&self, day: &str, stage: &str) -> Result<BTreeMap<String, Vec<PathBuf>>> {
        self.invoke(Some(&[day.to_owned()]), Some((stage, "begin")))
    }

    pub fn complete(&self, day: &str, stage: &str) -> Result<()> {
        self.invoke(Some(&[day.to_owned()]), Some((stage, "complete")))?;
        Ok(())
    }

    fn invoke(
        &self,
        days: Option<&[String]>,
        receipt: Option<(&str, &str)>,
    ) -> Result<BTreeMap<String, Vec<PathBuf>>> {
        let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut command = Command::new("python3");
        command
            .arg(project.join("scripts/download-adsblol.py"))
            .arg("validate")
            .arg("--source-root")
            .arg(&self.root);
        if let Some(days) = days {
            command.arg("--days").arg(days.join(","));
        }
        if let Some((stage, action)) = receipt {
            command
                .arg("--work-dir")
                .arg(&self.work)
                .arg("--stage")
                .arg(stage)
                .arg("--action")
                .arg(action);
        }
        let output = command
            .output()
            .context("run existing publisher/source-cache validator")?;
        anyhow::ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        eprint!("{}", String::from_utf8_lossy(&output.stderr));
        let fields = output
            .stdout
            .strip_suffix(&[0])
            .context("empty source validator response")?
            .split(|&byte| byte == 0)
            .collect::<Vec<_>>();
        anyhow::ensure!(fields.len() % 2 == 0, "invalid source path transport");
        let mut selected = BTreeMap::<String, Vec<PathBuf>>::new();
        for pair in fields.as_chunks::<2>().0 {
            let day = std::str::from_utf8(pair[0])?;
            aircraft_extract::period::parse_date_id(day)?;
            let path = PathBuf::from(std::str::from_utf8(pair[1])?);
            selected.entry(day.to_owned()).or_default().push(path);
        }
        Ok(selected)
    }
}

/// Re-validate the primary publisher receipts a sealed shuffle recorded for
/// its baseline days, without opening the day intermediates.
pub fn validate_shuffled_sources(shuffled: &Path, primary_root: &Path) -> Result<()> {
    use aircraft_extract::shuffle::completion::{sampling_days, source_receipts};
    let receipts = source_receipts(shuffled)?;
    let Some(_) = SourceCache::open(primary_root, Path::new("."))? else {
        anyhow::ensure!(
            receipts.is_empty(),
            "shuffled publisher receipts require their catalog-bound primary source"
        );
        return Ok(());
    };
    let (expected, _) = sampling_days(shuffled)?;
    let mut covered = std::collections::BTreeSet::new();
    for (_, receipt, _) in &receipts {
        let db = rusqlite::Connection::open_with_flags(
            receipt,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        let days = db
            .prepare("SELECT DISTINCT day FROM sources ORDER BY day")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let days: Vec<_> = days.into_iter().filter(|d| expected.contains(d)).collect();
        anyhow::ensure!(!days.is_empty(), "shuffle has an unrelated source receipt");
        for day in &days {
            anyhow::ensure!(
                covered.insert(day.clone()),
                "duplicate shuffled source receipt for {day}"
            );
        }
        SourceCache::new(
            primary_root,
            receipt.parent().context("missing source receipt parent")?,
        )
        .validate(Some(&days), Some("sources"))?;
    }
    anyhow::ensure!(
        covered == expected,
        "missing source receipts for shuffled sampling window"
    );
    Ok(())
}

#[cfg(test)]
#[path = "source_cache_tests.rs"]
mod tests;
